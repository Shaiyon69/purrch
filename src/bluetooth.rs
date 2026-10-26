//! Says hi when Bluetooth headphones, mice, etc. connect or drop off.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(windows)]
#[path = "win/bluetooth.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux/bluetooth.rs"]
mod imp;

#[derive(Debug, PartialEq)]
pub enum Change {
    Connected(String),
    Disconnected(String),
}

pub fn diff(old: &BTreeSet<String>, new: &BTreeSet<String>) -> Vec<Change> {
    let added = new.difference(old).cloned().map(Change::Connected);
    let removed = old.difference(new).cloned().map(Change::Disconnected);
    added.chain(removed).collect()
}

/// Polls every few seconds; changes queue up in the returned list for the UI to drain.
pub fn start(notify: impl Fn() + Send + 'static) -> Arc<Mutex<Vec<Change>>> {
    let queue = Arc::new(Mutex::new(Vec::new()));
    let worker_queue = queue.clone();
    std::thread::spawn(move || {
        // Devices already connected at startup are not news.
        let mut known = imp::connected().unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let Some(now) = imp::connected() else { continue };
            let changes = diff(&known, &now);
            known = now;
            if changes.is_empty() {
                continue;
            }
            if let Ok(mut q) = worker_queue.lock() {
                q.extend(changes);
            }
            notify();
        }
    });
    queue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_arrivals_and_departures() {
        let old: BTreeSet<String> = ["Mouse".into(), "Buds".into()].into();
        let new: BTreeSet<String> = ["Mouse".into(), "Speaker".into()].into();
        assert_eq!(diff(&old, &new), [Change::Connected("Speaker".into()), Change::Disconnected("Buds".into())]);
        assert!(diff(&new, &new).is_empty());
    }
}
