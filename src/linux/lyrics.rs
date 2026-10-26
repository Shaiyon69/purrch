//! LRCLIB search through `curl`, which nearly every distribution ships.

use super::{Offline, Results, USER_AGENT};
use std::process::Command;

pub fn search(query: &str) -> Result<Results, Offline> {
    let url = format!("https://lrclib.net/api/search?{query}");
    let out = Command::new("curl").args(["-fsS", "--max-time", "15", "-A", USER_AGENT, &url]).output().map_err(|_| Offline)?;
    if !out.status.success() {
        return Err(Offline);
    }
    parse_results(&String::from_utf8_lossy(&out.stdout))
}

/// (duration, synced lyrics) per search result. Either field can be null, so neither may fail
/// the whole list.
pub fn parse_results(body: &str) -> Result<Results, Offline> {
    let items: serde_json::Value = serde_json::from_str(body).map_err(|_| Offline)?;
    let items = items.as_array().ok_or(Offline)?;
    Ok(items.iter().map(|i| (i["duration"].as_f64().unwrap_or(0.0), i["syncedLyrics"].as_str().map(String::from))).collect())
}
