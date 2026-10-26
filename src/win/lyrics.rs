//! LRCLIB search over WinRT's HTTP client and JSON parser.

use super::{Offline, Results, USER_AGENT};
use windows::Data::Json::{JsonArray, JsonValueType};
use windows::Foundation::Uri;
use windows::Web::Http::HttpClient;
use windows::core::{HSTRING, h};

pub fn search(query: &str) -> Result<Results, Offline> {
    let get = || -> windows::core::Result<HSTRING> {
        let client = HttpClient::new()?;
        client.DefaultRequestHeaders()?.UserAgent()?.TryParseAdd(&HSTRING::from(USER_AGENT))?;
        let uri = Uri::CreateUri(&HSTRING::from(format!("https://lrclib.net/api/search?{query}")))?;
        client.GetStringAsync(&uri)?.get()
    };
    parse_results(&get().map_err(|_| Offline)?.to_string_lossy())
}

/// (duration, synced lyrics) per search result. Either field can be null, so neither may fail
/// the whole list: one entry with `"duration": null` used to hide every other match.
pub fn parse_results(body: &str) -> Result<Results, Offline> {
    let items = JsonArray::Parse(&HSTRING::from(body)).map_err(|_| Offline)?;
    let mut out = Vec::new();
    for i in 0..items.Size().map_err(|_| Offline)? {
        let Ok(item) = items.GetObjectAt(i) else { continue };
        let duration = item
            .GetNamedValue(h!("duration"))
            .ok()
            .and_then(|v| (v.ValueType().ok()? == JsonValueType::Number).then(|| v.GetNumber().ok()).flatten())
            .unwrap_or(0.0);
        let synced = item.GetNamedValue(h!("syncedLyrics")).ok().and_then(|v| {
            (v.ValueType().ok()? == JsonValueType::String).then(|| v.GetString().ok().map(|s| s.to_string())).flatten()
        });
        out.push((duration, synced));
    }
    Ok(out)
}
