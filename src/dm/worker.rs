//! The DM worker: links via the keyring, keeps the session fresh and saved,
//! listens on the user's private realtime topic, and sends replies.

use super::{DmEvent, LinkEntry, Note, SavedSession, decrypt, preview, reply_row, wanted};
use serde::de::DeserializeOwned;
use serde_json::Value;
use shaiyon_crypto::Jwk;
use shaiyon_keys::{E2eeKeys, KeyVault, OsStore, SecretStore};
use shaiyon_supabase::{Client, Error, Session, User, realtime};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

const LINK: &str = "purrch_link";
const SESSION: &str = "purrch_session";
const POLL: Duration = Duration::from_secs(10);
const CMD_POLL: Duration = Duration::from_millis(500);
/// Refresh this many seconds before the access token expires.
const REFRESH_MARGIN_SECS: i64 = 60;

pub enum Cmd {
    Reply { room: String, text: String, peer_public: Jwk },
    Unlink,
}

pub enum Out {
    Dm(DmEvent),
    Note(Note),
}

pub struct Dm {
    pub out: Arc<Mutex<Vec<Out>>>,
    pub cmd: Sender<Cmd>,
    pub linked: Arc<AtomicBool>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum State {
    Unlinked,
    Link(LinkEntry),
    Session(SavedSession),
}

fn read<T: DeserializeOwned, S: SecretStore>(store: &S, name: &str) -> Option<T> {
    serde_json::from_slice(&store.get(name).ok()??).ok()
}

/// A `purrch_link` wins: it is newer than any saved session.
pub(crate) fn next_state<S: SecretStore>(store: &S) -> State {
    if let Some(link) = read(store, LINK) {
        return State::Link(link);
    }
    read(store, SESSION).map_or(State::Unlinked, State::Session)
}

pub(crate) fn save_session<S: SecretStore>(store: &S, s: &SavedSession) -> Result<(), String> {
    let bytes = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    store.set(SESSION, &bytes).map_err(|e| e.to_string())
}

fn forget<S: SecretStore>(store: &S) {
    let _ = store.delete(LINK);
    let _ = store.delete(SESSION);
}

/// The old refresh token is already spent, so an unsaved new one would strand
/// the next start: forget the link instead.
pub(crate) fn on_refreshed<S: SecretStore>(store: &S, s: &SavedSession) -> Result<(), Note> {
    save_session(store, s).map_err(|_| {
        forget(store);
        Note::Unlinked
    })
}

pub fn start(notify: impl Fn() + Send + 'static) -> Dm {
    let out = Arc::new(Mutex::new(Vec::new()));
    let linked = Arc::new(AtomicBool::new(false));
    let (cmd, cmds) = std::sync::mpsc::channel();
    let worker = Worker { out: out.clone(), linked: linked.clone(), notify: Box::new(notify), cmds };
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(worker.run());
    });
    Dm { out, cmd, linked }
}

/// Why a session step stopped.
enum Fail {
    /// The session is forgotten; the UI was told.
    Gone,
    /// Transient (network, server); try again later.
    Retry,
}

struct Worker {
    out: Arc<Mutex<Vec<Out>>>,
    linked: Arc<AtomicBool>,
    notify: Box<dyn Fn() + Send>,
    cmds: Receiver<Cmd>,
}

impl Worker {
    fn push(&self, item: Out) {
        if let Ok(mut q) = self.out.lock() {
            q.push(item);
        }
        (self.notify)();
    }

    fn note(&self, note: Note) {
        self.push(Out::Note(note));
    }

    fn unlink(&self) {
        forget(&OsStore);
        self.linked.store(false, Ordering::Relaxed);
        self.note(Note::Unlinked);
    }

    async fn run(self) {
        loop {
            let wait = match next_state(&OsStore) {
                State::Unlinked => {
                    self.linked.store(false, Ordering::Relaxed);
                    self.drain_unlinked();
                    true
                }
                State::Link(link) => self.link(link).await,
                State::Session(saved) => self.session(saved).await,
            };
            if wait {
                tokio::time::sleep(POLL).await;
            }
        }
    }

    /// Commands queued while unlinked must not run against a later session.
    fn drain_unlinked(&self) {
        while let Ok(cmd) = self.cmds.try_recv() {
            if let Cmd::Reply { room, text, .. } = cmd {
                self.note(Note::SendFailed { error: "Messapp is not linked".into(), room, text });
            }
        }
    }

    /// Returns whether to wait before the next state check.
    async fn link(&self, link: LinkEntry) -> bool {
        let client = Client::new(&link.url, &link.anon_key);
        match client.verify_otp(&link.token_hash).await {
            Ok(s) => {
                // Ruling C: the server's user id wins over the link's uid.
                let saved = SavedSession { uid: s.user.id, refresh_token: s.refresh_token, url: link.url, anon_key: link.anon_key };
                let saved_ok = save_session(&OsStore, &saved).is_ok();
                let _ = OsStore.delete(LINK);
                self.note(if saved_ok { Note::Linked } else { Note::LinkFailed });
                !saved_ok
            }
            Err(Error::Api { .. }) => {
                let _ = OsStore.delete(LINK);
                self.note(Note::LinkFailed);
                true
            }
            Err(_) => true,
        }
    }

    /// Saves a refresh token the client rotated (explicitly or inside any call)
    /// and hands a new access token to the realtime socket.
    async fn sync(&self, client: &Client, saved: &mut SavedSession, tokens: &watch::Sender<String>) -> Result<(), Fail> {
        let Some(s) = client.session().await else { return Ok(()) };
        if s.refresh_token != saved.refresh_token {
            saved.refresh_token = s.refresh_token;
            if let Err(note) = on_refreshed(&OsStore, saved) {
                self.linked.store(false, Ordering::Relaxed);
                self.note(note);
                return Err(Fail::Gone);
            }
        }
        tokens.send_if_modified(|t| {
            let changed = *t != s.access_token;
            if changed {
                *t = s.access_token;
            }
            changed
        });
        Ok(())
    }

    async fn refresh(&self, client: &Client, saved: &mut SavedSession, tokens: &watch::Sender<String>) -> Result<(), Fail> {
        match client.refresh().await {
            Ok(_) => self.sync(client, saved, tokens).await,
            Err(Error::Api { status: 400 | 401, .. }) => {
                self.unlink();
                Err(Fail::Gone)
            }
            Err(_) => Err(Fail::Retry),
        }
    }

    /// Returns whether to wait before the next state check.
    async fn session(&self, mut saved: SavedSession) -> bool {
        let (url, anon_key, uid) = (saved.url.clone(), saved.anon_key.clone(), saved.uid.clone());
        let client = Client::new(&url, &anon_key);
        // The access token is never stored, so `expires_at: 0` makes the first step a refresh.
        let user = User { id: uid.clone(), email: None };
        client.set_session(Some(Session { access_token: String::new(), refresh_token: saved.refresh_token.clone(), expires_at: 0, user })).await;
        self.linked.store(true, Ordering::Relaxed);
        let (tokens, token_rx) = watch::channel(String::new());
        if self.refresh(&client, &mut saved, &tokens).await.is_err() {
            return true;
        }
        let keys = KeyVault::new(OsStore).load(&uid).unwrap_or_default();

        let topic = format!("user:{uid}");
        let (events_tx, mut events) = mpsc::channel(32);
        let listener = realtime::listen(&url, &anon_key, &topic, token_rx, events_tx);
        tokio::pin!(listener);
        let mut cmd_tick = tokio::time::interval(CMD_POLL);
        let mut state_tick = tokio::time::interval_at(Instant::now() + POLL, POLL);
        let mut retry_at: Option<Instant> = None;

        loop {
            let refresh_at = retry_at.unwrap_or(match client.session().await {
                Some(s) => Instant::now() + Duration::from_secs((s.expires_at - REFRESH_MARGIN_SECS - now()).max(0) as u64),
                None => Instant::now(),
            });
            let step = tokio::select! {
                _ = &mut listener => return true,
                Some((event, payload)) = events.recv() => match event.as_str() {
                    // Ruling A: an expired token is the usual cause of a refused join.
                    "join_error" => self.refresh(&client, &mut saved, &tokens).await,
                    "dm_activity" => match wanted(&payload, &uid) {
                        Some((id, room)) => self.fetch(&client, &mut saved, &tokens, &keys, &id, room).await,
                        None => Ok(()),
                    },
                    _ => Ok(()),
                },
                _ = tokio::time::sleep_until(refresh_at) => self.refresh(&client, &mut saved, &tokens).await,
                _ = cmd_tick.tick() => match self.cmds.try_recv() {
                    Ok(Cmd::Reply { room, text, peer_public }) => self.reply(&client, &mut saved, &tokens, &keys, room, text, &peer_public).await,
                    Ok(Cmd::Unlink) => {
                        let _ = client.sign_out().await;
                        self.unlink();
                        Err(Fail::Gone)
                    }
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => Ok(()),
                },
                // A newer link (or an outside delete) replaces this session.
                _ = state_tick.tick() => {
                    if !matches!(next_state(&OsStore), State::Session(_)) {
                        return false;
                    }
                    Ok(())
                }
            };
            match step {
                Ok(()) => {
                    if retry_at.is_some_and(|t| t <= Instant::now()) {
                        retry_at = None;
                    }
                }
                Err(Fail::Gone) => return true,
                Err(Fail::Retry) => retry_at = Some(Instant::now() + POLL),
            }
        }
    }

    async fn fetch(&self, client: &Client, saved: &mut SavedSession, tokens: &watch::Sender<String>, keys: &E2eeKeys, id: &str, room: String) -> Result<(), Fail> {
        let rows = client.select::<Value>("messages", &message_query(id)).await;
        self.sync(client, saved, tokens).await?;
        // A failed or empty fetch drops this one alert; nothing to retry.
        let Some(row) = rows.ok().and_then(|r| r.into_iter().next()) else { return Ok(()) };
        self.push(Out::Dm(dm_event(&row, keys, room)));
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // session context plus the reply's own three fields
    async fn reply(&self, client: &Client, saved: &mut SavedSession, tokens: &watch::Sender<String>, keys: &E2eeKeys, room: String, text: String, peer_public: &Jwk) -> Result<(), Fail> {
        let sent = match keys.private.as_ref() {
            None => Err("no private key".to_string()),
            Some(own) => match reply_row(&saved.uid, &room, own, peer_public, &text) {
                Err(e) => Err(e),
                Ok(row) => client.insert::<Value>("messages", &[row]).await.map(drop).map_err(|e| e.to_string()),
            },
        };
        self.sync(client, saved, tokens).await?;
        self.note(match sent {
            Ok(()) => Note::Sent,
            Err(error) => Note::SendFailed { error, room, text },
        });
        Ok(())
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// `messages` reaches `profiles` through three foreign keys, so the embed must name one.
fn message_query(id: &str) -> String {
    format!("id=eq.{id}&select=content,is_encrypted,profile_id,dm_room_id,profiles!fk_messages_profile(username,public_key)")
}

fn dm_event(row: &Value, keys: &E2eeKeys, room: String) -> DmEvent {
    let p = &row["profiles"];
    // `public_key` is a JWK serialized as a JSON string.
    let peer_public: Option<Jwk> = p["public_key"].as_str().and_then(|s| serde_json::from_str(s).ok());
    let content = row["content"].as_str().unwrap_or_default();
    let text = if row["is_encrypted"] == true {
        decrypt(content, keys.private.as_ref(), &keys.legacy, peer_public.as_ref())
    } else {
        Some(content.to_string())
    };
    DmEvent {
        room,
        sender: p["username"].as_str().unwrap_or_default().into(),
        preview: text.map(|t| preview(&t)),
        peer_public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiyon_keys::MemoryStore;
    use serde_json::json;

    fn link() -> LinkEntry {
        LinkEntry { uid: "u1".into(), token_hash: "h".into(), url: "https://x.supabase.co".into(), anon_key: "k".into() }
    }

    fn session() -> SavedSession {
        SavedSession {
            uid: "0b6f3c1e-9a7d-4e2b-8c5f-1d2e3f4a5b6c".into(),
            refresh_token: "r".repeat(64),
            url: "https://abcdefghijklmnopqrst.supabase.co".into(),
            // Supabase anon keys are JWTs of about 210 chars; leave headroom.
            anon_key: "a".repeat(400),
        }
    }

    #[test]
    fn state_unlinked_when_empty() {
        assert_eq!(next_state(&MemoryStore::default()), State::Unlinked);
    }

    #[test]
    fn newer_link_replaces_session() {
        let store = MemoryStore::default();
        save_session(&store, &session()).unwrap();
        store.set(LINK, &serde_json::to_vec(&link()).unwrap()).unwrap();
        assert_eq!(next_state(&store), State::Link(link()));
    }

    #[test]
    fn session_round_trips() {
        let store = MemoryStore::default();
        save_session(&store, &session()).unwrap();
        assert_eq!(next_state(&store), State::Session(session()));
        assert!(store.get(SESSION).unwrap().unwrap().len() < 2560);
    }

    struct FailingSet(Mutex<Vec<String>>);

    impl SecretStore for FailingSet {
        fn get(&self, _: &str) -> shaiyon_keys::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &[u8]) -> shaiyon_keys::Result<()> {
            Err(shaiyon_keys::Error::Store("full".into()))
        }
        fn delete(&self, name: &str) -> shaiyon_keys::Result<()> {
            self.0.lock().unwrap().push(name.into());
            Ok(())
        }
    }

    #[test]
    fn refresh_save_failure_unlinks() {
        let store = FailingSet(Mutex::new(Vec::new()));
        assert!(save_session(&store, &session()).is_err());
        store.0.lock().unwrap().clear();
        assert_eq!(on_refreshed(&store, &session()), Err(Note::Unlinked));
        assert_eq!(*store.0.lock().unwrap(), [LINK, SESSION]);
    }

    #[test]
    fn message_query_names_the_sender_fk() {
        assert_eq!(
            message_query("m1"),
            "id=eq.m1&select=content,is_encrypted,profile_id,dm_room_id,profiles!fk_messages_profile(username,public_key)"
        );
    }

    #[test]
    fn dm_event_reads_row() {
        let (alice, bob) = (shaiyon_crypto::generate_keypair(), shaiyon_crypto::generate_keypair());
        let sent = reply_row("a", "r1", &alice, &bob.public(), "hey").unwrap();
        let profiles = json!({"username": "alice_1", "public_key": serde_json::to_string(&alice.public()).unwrap()});
        let row = json!({"content": sent["content"], "is_encrypted": true, "profiles": profiles});
        let keys = E2eeKeys { private: Some(bob), public: None, legacy: vec![] };
        let ev = dm_event(&row, &keys, "r1".into());
        assert_eq!(ev.sender, "alice_1");
        assert_eq!(ev.preview.as_deref(), Some("hey"));
        assert_eq!(ev.peer_public, Some(alice.public()));

        let row = json!({"content": "x", "is_encrypted": true, "profiles": {"username": "b", "public_key": "not a jwk"}});
        let ev = dm_event(&row, &E2eeKeys::default(), "r1".into());
        assert_eq!((ev.preview, ev.peer_public), (None, None));
    }
}
