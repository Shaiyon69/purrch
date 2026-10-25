//! Says hi when Bluetooth headphones, mice, etc. connect or drop off.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_BLUETOOTH: u32 = WM_APP + 3;

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

fn connected() -> windows::core::Result<BTreeSet<String>> {
    let status = BluetoothConnectionStatus::Connected;
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromConnectionStatus(status)?,
        BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(status)?,
    ];
    let mut names = BTreeSet::new();
    for selector in selectors {
        let list = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.get()?;
        for i in 0..list.Size()? {
            let name = list.GetAt(i)?.Name()?.to_string();
            if !name.is_empty() {
                names.insert(name);
            }
        }
    }
    Ok(names)
}

/// Polls every few seconds; changes queue up in the returned list for the UI to drain.
pub fn start(hwnd: HWND) -> Arc<Mutex<Vec<Change>>> {
    let queue = Arc::new(Mutex::new(Vec::new()));
    let worker_queue = queue.clone();
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        // Devices already connected at startup are not news.
        let mut known = connected().unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let Ok(now) = connected() else { continue };
            let changes = diff(&known, &now);
            known = now;
            if changes.is_empty() {
                continue;
            }
            if let Ok(mut q) = worker_queue.lock() {
                q.extend(changes);
            }
            unsafe {
                let _ = PostMessageW(HWND(hwnd as _), WM_BLUETOOTH, WPARAM(0), LPARAM(0));
            }
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
