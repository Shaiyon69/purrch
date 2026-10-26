//! Connected Bluetooth devices from BlueZ over the system bus.

use crate::linux::{proxy, system};
use std::collections::{BTreeSet, HashMap};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// None when BlueZ isn't running (no adapter, or no Bluetooth stack at all).
pub fn connected() -> Option<BTreeSet<String>> {
    let p = proxy(system()?, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager").ok()?;
    let objects: Objects = p.call("GetManagedObjects", &()).ok()?;
    let names = objects
        .values()
        .filter_map(|ifaces| ifaces.get("org.bluez.Device1"))
        .filter(|d| d.get("Connected").is_some_and(|v| v.downcast_ref::<bool>().unwrap_or(false)))
        .filter_map(|d| d.get("Alias").or(d.get("Name"))?.downcast_ref::<&str>().ok().map(String::from))
        .filter(|n| !n.is_empty())
        .collect();
    Some(names)
}
