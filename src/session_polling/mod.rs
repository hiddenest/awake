mod claude_code;
mod codex;
mod opencode;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const TARGETS: [&str; 3] = ["claude-code", "codex", "opencode"];
pub(crate) const ACTIVE_SESSION_WINDOW_SECS: u64 = crate::POLL_INTERVAL_SECS * 3;

pub(crate) struct SessionPollResult {
    pub(crate) active: bool,
    pub(crate) detail: String,
}

pub(crate) fn poll_target_session(name: &str) -> SessionPollResult {
    match name {
        "claude-code" => claude_code::poll_session(),
        "codex" => codex::poll_session(),
        "opencode" => opencode::poll_session(),
        _ => SessionPollResult {
            active: false,
            detail: "unsupported target".to_string(),
        },
    }
}

fn claude_runtime_detail(gui_present: bool, ide_lock_present: bool) -> String {
    match (gui_present, ide_lock_present) {
        (true, true) => "GUI app + IDE lock present".to_string(),
        (true, false) => "GUI app present".to_string(),
        (false, true) => "IDE lock present".to_string(),
        (false, false) => "no Claude GUI or IDE session".to_string(),
    }
}

fn gui_runtime_detail(gui_present: bool, app_name: &str) -> String {
    if gui_present {
        format!("{} GUI app present", app_name)
    } else {
        format!("no {} GUI app", app_name)
    }
}

fn claude_ide_lock_active() -> bool {
    let dir = home_dir().join(".claude/ide");
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("lock") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        if let Some(pid) = extract_json_number(&content, "pid") {
            if crate::process_alive(pid as u32) {
                return true;
            }
        }
    }
    false
}

fn extract_json_number(content: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\"", key);
    let start = content.find(&needle)?;
    let after_key = &content[start + needle.len()..];
    let colon = after_key.find(':')?;
    let value = after_key[colon + 1..].trim_start();
    let digits: String = value.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

fn newest_matching_file(dir: &Path, pattern: &str) -> Option<PathBuf> {
    let (prefix, suffix) = match pattern.split_once('*') {
        Some((p, s)) => (p, s),
        None => return None,
    };
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for entry in fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(prefix) || !name.ends_with(suffix) {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        match &newest {
            Some((_, current)) if modified <= *current => {}
            _ => newest = Some((path, modified)),
        }
    }
    newest.map(|(path, _)| path)
}

fn newest_file_age_secs(dir: &Path) -> Option<(PathBuf, u64)> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for entry in fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        match &newest {
            Some((_, current)) if modified <= *current => {}
            _ => newest = Some((path, modified)),
        }
    }

    let (path, modified) = newest?;
    let age_secs = SystemTime::now().duration_since(modified).ok()?.as_secs();
    Some((path, age_secs))
}

fn sqlite_single_line(path: &Path, query: &str) -> Option<String> {
    let uri = format!("file:{}?mode=ro&immutable=1", path.display());
    let output = crate::command_output("sqlite3", &[&uri, query]).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
}

fn gui_app_running(app_name: &str) -> bool {
    let output = match crate::command_output("lsappinfo", &["list"]) {
        Ok(output) if output.status.success() => output,
        _ => return false,
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.contains(&format!("\"{}\"", app_name)))
}

fn process_command_lines(process_name: &str) -> Vec<String> {
    let output = match crate::command_output("ps", &["axww", "-o", "command="]) {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let executable = line.split_whitespace().next().unwrap_or("");
            executable == process_name || executable.rsplit('/').next() == Some(process_name)
        })
        .map(|line| line.to_string())
        .collect()
}

fn activity_within_window(age_secs: u64) -> bool {
    age_secs <= ACTIVE_SESSION_WINDOW_SECS
}

fn age_from_epoch_secs(value: Option<u64>) -> Option<u64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let value = value?;
    Some(now.saturating_sub(value))
}

fn age_from_epoch_millis(value: Option<u64>) -> Option<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let value = value?;
    Some(now.saturating_sub(value) / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn age_from_epoch_secs_computes_recent_delta() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(age_from_epoch_secs(Some(now.saturating_sub(4))), Some(4));
    }

    #[test]
    fn age_from_epoch_millis_computes_recent_delta() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(
            age_from_epoch_millis(Some(now.saturating_sub(4_000))),
            Some(4)
        );
    }

    #[test]
    fn newest_file_age_secs_returns_none_for_missing_dir() {
        assert!(newest_file_age_secs(Path::new("/definitely/missing-awake-dir")).is_none());
    }

    #[test]
    fn newest_file_age_secs_prefers_latest_file() {
        let dir = env::temp_dir().join(format!("awake-file-age-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let older = dir.join("older.txt");
        let newer = dir.join("newer.txt");
        fs::write(&older, "a").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&newer, "b").unwrap();

        let (path, age_secs) = newest_file_age_secs(&dir).unwrap();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("newer.txt")
        );
        assert!(age_secs <= 1);

        let _ = fs::remove_file(older);
        let _ = fs::remove_file(newer);
        let _ = fs::remove_dir(dir);
    }

    #[test]
    fn activity_within_window_accepts_recent_updates() {
        assert!(activity_within_window(ACTIVE_SESSION_WINDOW_SECS));
    }

    #[test]
    fn activity_within_window_rejects_stale_updates() {
        assert!(!activity_within_window(ACTIVE_SESSION_WINDOW_SECS + 1));
    }
}
