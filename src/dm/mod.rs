//! Pure DM logic: no I/O. The network worker and UI build on this.

pub mod worker;

use serde::{Deserialize, Serialize};
use shaiyon_crypto::{Envelope, Jwk};
use std::fmt;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkEntry {
    pub uid: String,
    pub token_hash: String,
    pub url: String,
    pub anon_key: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    pub uid: String,
    pub refresh_token: String,
    pub url: String,
    pub anon_key: String,
}

// Hand-written so tokens never reach a log line.
impl fmt::Debug for LinkEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkEntry").field("uid", &self.uid).field("url", &self.url).finish()
    }
}

impl fmt::Debug for SavedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedSession").field("uid", &self.uid).field("url", &self.url).finish()
    }
}

/// `preview == None` means the message could not be decrypted, so replying is disabled.
#[derive(Debug, Clone, PartialEq)]
pub struct DmEvent {
    pub room: String,
    pub sender: String,
    pub preview: Option<String>,
    pub peer_public: Option<Jwk>,
}

/// Toast status for the UI. `SendFailed` carries the text back so the UI can restore it.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    Linked,
    LinkFailed,
    Unlinked,
    Sent,
    SendFailed { error: String, room: String, text: String },
}

const PREVIEW_CHARS: usize = 80;

pub fn preview(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return "Sent an attachment".into();
    }
    match t.char_indices().nth(PREVIEW_CHARS) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.into(),
    }
}

/// `(message id, room)` for an INSERT in a DM room from someone else.
pub fn wanted(payload: &serde_json::Value, me: &str) -> Option<(String, String)> {
    if payload["type"] != "INSERT" || payload["profile_id"].as_str()? == me {
        return None;
    }
    Some((payload["id"].as_str()?.into(), payload["dm_room_id"].as_str()?.into()))
}

pub fn reply_row(me: &str, room: &str, own: &Jwk, peer_public: &Jwk, text: &str) -> Result<serde_json::Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty reply".into());
    }
    if own.d.is_none() {
        return Err("no private key".into());
    }
    let key = shaiyon_crypto::derive_shared(own, peer_public).map_err(|e| e.to_string())?;
    let mut env = shaiyon_crypto::encrypt_text(&key, text);
    env.spub = Some(own.public());
    env.tpub = Some(peer_public.public());
    let content = serde_json::to_string(&env).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "profile_id": me,
        "dm_room_id": room,
        "content": content,
        "is_encrypted": true,
        "origin": "purrch",
    }))
}

pub fn decrypt(content: &str, own: Option<&Jwk>, legacy: &[Jwk], sender_public: Option<&Jwk>) -> Option<String> {
    let env: Envelope = serde_json::from_str(content).ok()?;
    let historical = shaiyon_crypto::historical_keys(&env, false, own, legacy);
    if let Some(t) = shaiyon_crypto::decrypt_text_any(&historical, &env) {
        return Some(t);
    }
    let keys = shaiyon_crypto::build_shared_keys(own, sender_public, legacy);
    shaiyon_crypto::decrypt_text_any(keys.iter(), &env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preview_cuts_multibyte_on_char_boundary() {
        let p = preview(&"é👋".repeat(100));
        assert_eq!(p.chars().count(), 81);
        assert!(p.ends_with('…'));
        assert_eq!(preview("  hi "), "hi");
        assert_eq!(preview(""), "Sent an attachment");
    }

    #[test]
    fn wanted_filters_events() {
        let ev = |t: &str, room: serde_json::Value, who: &str| {
            json!({"type": t, "id": "m1", "profile_id": who, "created_at": "2026-11-03T00:00:00Z",
                "dm_room_id": room, "channel_id": null, "thread_root_id": null})
        };
        assert_eq!(wanted(&ev("INSERT", json!("r1"), "me"), "me"), None);
        assert_eq!(wanted(&ev("UPDATE", json!("r1"), "bob"), "me"), None);
        assert_eq!(wanted(&ev("INSERT", json!(null), "bob"), "me"), None);
        assert_eq!(wanted(&ev("INSERT", json!("r1"), "bob"), "me"), Some(("m1".into(), "r1".into())));
    }

    #[test]
    fn reply_round_trips() {
        let (alice, bob) = (shaiyon_crypto::generate_keypair(), shaiyon_crypto::generate_keypair());
        let row = reply_row("a", "r1", &alice, &bob.public(), "yo").unwrap();
        assert_eq!(row["origin"], "purrch");
        assert_eq!(row["is_encrypted"], true);
        let content = row["content"].as_str().unwrap();
        assert_eq!(decrypt(content, Some(&bob), &[], Some(&alice.public())).as_deref(), Some("yo"));
    }

    #[test]
    fn reply_refuses_without_private_key() {
        let (alice, bob) = (shaiyon_crypto::generate_keypair(), shaiyon_crypto::generate_keypair());
        assert!(reply_row("a", "r1", &alice.public(), &bob.public(), "yo").is_err());
        assert!(reply_row("a", "r1", &alice, &bob.public(), "  ").is_err());
    }

    #[test]
    fn debug_hides_secrets() {
        let link = LinkEntry { uid: "u1".into(), token_hash: "SECRET-HASH".into(), url: "https://x".into(), anon_key: "SECRET-ANON".into() };
        let saved = SavedSession { uid: "u1".into(), refresh_token: "SECRET-REFRESH".into(), url: "https://x".into(), anon_key: "SECRET-ANON".into() };
        for out in [format!("{link:?}"), format!("{saved:?}")] {
            assert!(out.contains("u1") && out.contains("https://x"));
            assert!(!out.contains("SECRET"), "{out}");
        }
    }

    #[test]
    fn decrypt_garbage_is_none() {
        assert_eq!(decrypt("not json", None, &[], None), None);
    }
}
