//! Synced lyrics from LRCLIB (https://lrclib.net), a free, open lyrics database.

use std::time::Duration;
use windows::Data::Json::{JsonArray, JsonValueType};
use windows::Foundation::Uri;
use windows::Web::Http::HttpClient;
use windows::core::{HSTRING, h};

const USER_AGENT: &str = "Purrch/0.1 (open-source notch app for Windows)";

pub type Lines = Vec<(Duration, String)>;

/// Players often append "(Remastered 2011)", "[Live]" or " - Radio Edit", which breaks lookups.
pub fn clean_title(title: &str) -> &str {
    [" (", " [", " - "]
        .iter()
        .filter_map(|sep| title.find(sep))
        .min()
        .map_or(title, |i| &title[..i])
        .trim()
}

pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Parses LRC text: "[01:02.50] words". A line may carry several time tags.
pub fn parse_lrc(text: &str) -> Lines {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(end) = inner.find(']') else { break };
            if let Some(t) = parse_stamp(&inner[..end]) {
                stamps.push(t);
            }
            rest = inner[end + 1..].trim_start();
        }
        for t in stamps {
            lines.push((t, rest.to_string()));
        }
    }
    lines.sort_by_key(|(t, _)| *t);
    lines
}

fn parse_stamp(s: &str) -> Option<Duration> {
    let (m, sec) = s.split_once(':')?;
    let m: u64 = m.parse().ok()?;
    let sec: f64 = sec.parse().ok()?;
    (sec < 60.0).then(|| Duration::from_secs(m * 60) + Duration::from_secs_f64(sec))
}

/// Index of the line being sung at `pos`.
pub fn current(lines: &Lines, pos: Duration) -> Option<usize> {
    lines.iter().rposition(|(t, _)| *t <= pos)
}

/// Vertical centre of each line when the lines are stacked, given their heights.
pub fn centers(heights: &[f32]) -> Vec<f32> {
    let mut top = 0.0;
    heights
        .iter()
        .map(|h| {
            let c = top + h / 2.0;
            top += h;
            c
        })
        .collect()
}

/// Opacity at `f` (0 = top, 1 = bottom) in a lyric box whose top and bottom 22% fade out.
pub fn edge_fade(f: f32) -> f32 {
    (f.min(1.0 - f) / 0.22).clamp(0.0, 1.0)
}

/// Out of the search results, pick the synced lyrics whose length is closest to the track's.
pub fn pick(results: &[(f64, Option<String>)], length: Duration) -> Option<Lines> {
    let len = length.as_secs_f64();
    results
        .iter()
        .filter_map(|(d, synced)| synced.as_ref().map(|s| (d, s)))
        .min_by(|a, b| (a.0 - len).abs().total_cmp(&(b.0 - len).abs()))
        .map(|(_, s)| parse_lrc(s))
        .filter(|l| !l.is_empty())
}

fn search(query: &str) -> windows::core::Result<Vec<(f64, Option<String>)>> {
    let client = HttpClient::new()?;
    client.DefaultRequestHeaders()?.UserAgent()?.TryParseAdd(&HSTRING::from(USER_AGENT))?;
    let uri = Uri::CreateUri(&HSTRING::from(format!("https://lrclib.net/api/search?{query}")))?;
    let body = client.GetStringAsync(&uri)?.get()?;
    let items = JsonArray::Parse(&body)?;
    let mut out = Vec::new();
    for i in 0..items.Size()? {
        let item = items.GetObjectAt(i)?;
        let duration = item.GetNamedNumberOrDefault(h!("duration"), 0.0)?;
        let synced = item.GetNamedValue(h!("syncedLyrics")).ok().and_then(|v| {
            (v.ValueType().ok()? == JsonValueType::String).then(|| v.GetString().ok().map(|s| s.to_string())).flatten()
        });
        out.push((duration, synced));
    }
    Ok(out)
}

/// Blocking network call; run it off the UI thread.
pub fn fetch(title: &str, artist: &str, length: Duration) -> Option<Lines> {
    let title = clean_title(title);
    let exact = format!("track_name={}&artist_name={}", encode(title), encode(artist));
    if let Some(lines) = search(&exact).ok().and_then(|r| pick(&r, length)) {
        return Some(lines);
    }
    let loose = format!("q={}", encode(&format!("{title} {artist}")));
    search(&loose).ok().and_then(|r| pick(&r, length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Song (Remastered 2011)"), "Song");
        assert_eq!(clean_title("Song - Radio Edit [Live]"), "Song");
        assert_eq!(clean_title("Plain"), "Plain");
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(encode("AC/DC & Co é"), "AC%2FDC%20%26%20Co%20%C3%A9");
    }

    #[test]
    fn parses_lrc_with_multiple_tags() {
        let l = parse_lrc("[ar:Someone]\n[00:01.50]Hello\n[00:03.00][00:10.00]Chorus\nno tag");
        let times: Vec<f64> = l.iter().map(|(t, _)| t.as_secs_f64()).collect();
        assert_eq!(times, [1.5, 3.0, 10.0]);
        assert_eq!(l[1].1, "Chorus");
        assert_eq!(current(&l, Duration::from_secs(5)), Some(1));
        assert_eq!(current(&l, Duration::from_secs(0)), None);
    }

    #[test]
    fn stacks_and_fades_lines() {
        assert_eq!(centers(&[20.0, 40.0, 10.0]), [10.0, 40.0, 65.0]);
        assert_eq!(edge_fade(0.5), 1.0);
        assert_eq!(edge_fade(0.0), 0.0);
        assert_eq!(edge_fade(1.0), 0.0);
        assert!((edge_fade(0.11) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn picks_closest_synced_result() {
        let results = [
            (300.0, Some("[00:01.00]long".to_string())),
            (181.0, Some("[00:01.00]right".to_string())),
            (180.0, None),
        ];
        let l = pick(&results, Duration::from_secs(180)).unwrap();
        assert_eq!(l[0].1, "right");
        assert_eq!(pick(&[(180.0, None)], Duration::from_secs(180)), None);
    }
}
