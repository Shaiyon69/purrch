//! Connected Bluetooth devices, Classic and LE, through WinRT device enumeration.

use std::collections::BTreeSet;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;

pub fn connected() -> Option<BTreeSet<String>> {
    list().ok()
}

fn list() -> windows::core::Result<BTreeSet<String>> {
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
