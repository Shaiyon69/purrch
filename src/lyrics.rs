//! Synced lyrics from LRCLIB (https://lrclib.net), a free, open lyrics database.

use std::time::Duration;

const USER_AGENT: &str = "Purrch/0.1 (open-source notch app)";

pub type Lines = Vec<(Duration, String)>;

/// LRCLIB could not be reached (or answered with something that isn't a result list).
#[derive(Debug)]
pub struct Offline;

/// (duration, synced lyrics) per search result.
type Results = Vec<(f64, Option<String>)>;

fn search(query: &str) -> Result<Results, Offline> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| Offline)?;
    let res = client.get(format!("https://lrclib.net/api/search?{query}")).send().map_err(|_| Offline)?;
    parse_results(&res.error_for_status().map_err(|_| Offline)?.text().map_err(|_| Offline)?)
}

/// (duration, synced lyrics) per search result. Either field can be null, so neither may fail
/// the whole list: one entry with `"duration": null` used to hide every other match.
fn parse_results(body: &str) -> Result<Results, Offline> {
    let items: serde_json::Value = serde_json::from_str(body).map_err(|_| Offline)?;
    let items = items.as_array().ok_or(Offline)?;
    Ok(items.iter().map(|i| (i["duration"].as_f64().unwrap_or(0.0), i["syncedLyrics"].as_str().map(String::from))).collect())
}

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

/// YouTube channel names: "Artist - Topic", "ArtistVEVO".
pub fn clean_artist(artist: &str) -> &str {
    let a = artist.strip_suffix(" - Topic").unwrap_or(artist);
    a.strip_suffix("VEVO").unwrap_or(a).trim()
}

/// Drops "(Official Video)" and "[Lyrics]" style extras but keeps a "Artist - Song" dash.
pub fn strip_brackets(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for ch in title.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The searches to try, best first. Browsers often title videos "Artist - Song".
pub fn queries(title: &str, artist: &str) -> Vec<String> {
    let channel = clean_artist(artist) != artist;
    let artist = clean_artist(artist);
    let bare = strip_brackets(title);
    let mut out = vec![format!("track_name={}&artist_name={}", encode(clean_title(title)), encode(artist))];
    if let Some((by, song)) = bare.split_once(" - ") {
        let split = format!("track_name={}&artist_name={}", encode(song.trim()), encode(by.trim()));
        // A channel name, or the artist again before the dash, means "Artist - Song": the plain
        // title query would search for the artist's name as a song and can match the wrong one.
        if channel || by.trim().eq_ignore_ascii_case(artist) {
            out.insert(0, split);
        } else {
            out.push(split);
        }
    }
    out.push(format!("q={}", encode(&format!("{bare} {artist}"))));
    out
}

/// Blocking network calls, run side by side; call it off the UI thread.
/// Err only when every search failed to reach LRCLIB, so the caller can retry.
pub fn fetch(title: &str, artist: &str, length: Duration) -> Result<Option<Lines>, Offline> {
    let results: Vec<_> = std::thread::scope(|s| {
        let jobs: Vec<_> = queries(title, artist).into_iter().map(|q| s.spawn(move || search(&q))).collect();
        jobs.into_iter().map(|j| j.join().unwrap_or(Err(Offline))).collect()
    });
    if let Some(lines) = results.iter().flatten().find_map(|r| pick(r, length)) {
        return Ok(Some(lines));
    }
    if results.iter().all(Result::is_err) {
        return Err(Offline);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hits lrclib.net; run with --ignored"]
    fn live_lookup() {
        assert!(fetch("Hello", "Adele", Duration::from_secs(295)).unwrap().is_some());
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Song (Remastered 2011)"), "Song");
        assert_eq!(clean_title("Song - Radio Edit [Live]"), "Song");
        assert_eq!(clean_title("Plain"), "Plain");
    }

    #[test]
    fn null_fields_keep_the_other_results() {
        let body = r#"[{"duration":null,"syncedLyrics":"[00:01.00]a"},{"duration":263.0,"syncedLyrics":null},{"duration":263.0,"syncedLyrics":"[00:01.00]b"}]"#;
        let r = parse_results(body).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].0, 0.0);
        assert_eq!(pick(&r, Duration::from_secs(262)).unwrap()[0].1, "b");
    }

    #[test]
    fn splits_browser_titles() {
        assert_eq!(clean_artist("Adele - Topic"), "Adele");
        assert_eq!(clean_artist("AdeleVEVO"), "Adele");
        assert_eq!(strip_brackets("Adele - Hello (Official Video) [4K]"), "Adele - Hello");
        let q = queries("Adele - Hello (Official Video)", "AdeleVEVO");
        assert_eq!(q[0], "track_name=Hello&artist_name=Adele", "channel uploads search the split first");
        assert_eq!(q[2], "q=Adele%20-%20Hello%20Adele");
        assert_eq!(queries("Song - Radio Edit", "Band")[0], "track_name=Song&artist_name=Band", "a real artist keeps its title first");
        assert_eq!(queries("Hello", "Adele").len(), 2, "no dash, no split query");
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
