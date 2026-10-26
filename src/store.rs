use std::collections::HashMap;
use std::path::PathBuf;
use std::{fs, io};

#[cfg(windows)]
pub fn dir() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Purrch")
}

/// `$XDG_CONFIG_HOME/purrch`, usually `~/.config/purrch`.
#[cfg(target_os = "linux")]
pub fn dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("purrch")
}

/// An XDG base directory, falling back to `~/<fallback>` when the variable is unset.
#[cfg(target_os = "linux")]
pub fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
}

pub fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn load(name: &str) -> HashMap<String, String> {
    fs::read_to_string(dir().join(name)).map(|t| parse(&t)).unwrap_or_default()
}

pub fn get<T: std::str::FromStr>(map: &HashMap<String, String>, key: &str, default: T) -> T {
    map.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Write to a temp file first so a crash mid-write never truncates the real file.
pub fn save(name: &str, contents: &str) -> io::Result<()> {
    let dir = dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_values_and_skips_junk() {
        let m = parse("a=1\nnot a pair\n b = two \n");
        assert_eq!(m.get("a").map(String::as_str), Some("1"));
        assert_eq!(m.get("b").map(String::as_str), Some("two"));
        assert_eq!(m.len(), 2);
        assert_eq!(get(&m, "a", 0u32), 1);
        assert_eq!(get(&m, "missing", 7u32), 7);
    }
}
