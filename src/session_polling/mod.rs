mod claude_code;
mod codex;
mod opencode;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const TARGETS: [&str; 3] = ["claude-code", "codex", "opencode"];
pub(crate) const ACTIVE_SESSION_WINDOW_SECS: u64 = crate::POLL_INTERVAL_SECS * 3;
pub(crate) const SQLITE_FIELD_SEPARATOR: char = '\u{1f}';

pub(crate) struct SessionPollResult {
    pub(crate) active: bool,
    pub(crate) detail: String,
    pub(crate) last_activity_age_secs: Option<u64>,
    pub(crate) worked_for_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessInfo {
    pub(crate) pid: u32,
    pub(crate) command: String,
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClaudeIdeLock {
    workspace_folders: Vec<PathBuf>,
}

pub(crate) fn poll_target_session(name: &str) -> SessionPollResult {
    match name {
        "claude-code" => claude_code::poll_session(),
        "codex" => codex::poll_session(),
        "opencode" => opencode::poll_session(),
        _ => SessionPollResult {
            active: false,
            detail: "unsupported target".to_string(),
            last_activity_age_secs: None,
            worked_for_secs: None,
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

fn claude_ide_lock_active() -> bool {
    !claude_ide_locks().is_empty()
}

fn claude_ide_locks() -> Vec<ClaudeIdeLock> {
    let dir = home_dir().join(".claude/ide");
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut locks = Vec::new();

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
                let workspace_folders = extract_json_string_array(&content, "workspaceFolders")
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                locks.push(ClaudeIdeLock { workspace_folders });
            }
        }
    }

    locks
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

fn extract_json_string_array(content: &str, key: &str) -> Option<Vec<String>> {
    let needle = format!("\"{}\"", key);
    let start = content.find(&needle)?;
    let after_key = &content[start + needle.len()..];
    let colon = after_key.find(':')?;
    let value = after_key[colon + 1..].trim_start();
    if !value.starts_with('[') {
        return None;
    }

    let mut values = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut current = String::new();

    for ch in value[1..].chars() {
        if !in_string {
            match ch {
                '"' => {
                    in_string = true;
                    current.clear();
                }
                ']' => break,
                _ => {}
            }
            continue;
        }

        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => {
                values.push(current.clone());
                in_string = false;
            }
            _ => current.push(ch),
        }
    }

    Some(values)
}

fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

fn matching_files(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let (prefix, suffix) = match pattern.split_once('*') {
        Some((p, s)) => (p, s),
        None => return Vec::new(),
    };
    let mut matches = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries {
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
        matches.push((path, modified));
    }
    matches.sort_by(|a, b| b.1.cmp(&a.1));
    matches.into_iter().map(|(path, _)| path).collect()
}

#[cfg(test)]
fn newest_jsonl_file_age_secs(dir: &Path) -> Option<(PathBuf, u64)> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for entry in fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
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
    let uri = format!("file:{}?mode=ro", path.display());
    let separator = SQLITE_FIELD_SEPARATOR.to_string();
    let output = crate::command_output("sqlite3", &["-separator", &separator, &uri, query]).ok()?;
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
    process_infos(process_name)
        .into_iter()
        .map(|process| process.command)
        .collect()
}

fn process_infos(process_name: &str) -> Vec<ProcessInfo> {
    let output = match crate::command_output("ps", &["axww", "-o", "pid=,command="]) {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_pid_command_line)
        .filter(|(_, command)| matches_process_name(command, process_name))
        .map(|(pid, command)| ProcessInfo {
            pid,
            command: command.to_string(),
            cwd: process_cwd(pid),
        })
        .collect()
}

fn process_cwd(pid: u32) -> Option<PathBuf> {
    let output =
        crate::command_output("lsof", &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).ok()?;
    if !output.status.success() {
        return None;
    }
    parse_lsof_cwd(&String::from_utf8_lossy(&output.stdout))
}

fn parse_lsof_cwd(output: &str) -> Option<PathBuf> {
    let mut saw_cwd = false;
    for line in output.lines() {
        if line == "fcwd" {
            saw_cwd = true;
            continue;
        }
        if saw_cwd && line.starts_with('n') {
            return Some(PathBuf::from(&line[1..]));
        }
    }
    None
}

fn parse_pid_command_line(line: &str) -> Option<(u32, &str)> {
    let trimmed = line.trim();
    let split_at = trimmed.find(char::is_whitespace)?;
    let (pid, command) = trimmed.split_at(split_at);
    Some((pid.parse().ok()?, command.trim_start()))
}

fn matches_process_name(command_line: &str, process_name: &str) -> bool {
    let executable = command_line.split_whitespace().next().unwrap_or("");
    executable == process_name || executable.rsplit('/').next() == Some(process_name)
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

fn age_from_system_time(value: Option<SystemTime>) -> Option<u64> {
    let value = value?;
    Some(SystemTime::now().duration_since(value).ok()?.as_secs())
}

fn sql_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn sanitize_claude_project_path(workspace: &Path) -> String {
    let raw = workspace.to_string_lossy();
    let mut sanitized = String::with_capacity(raw.len() + 1);
    for ch in raw.chars() {
        match ch {
            '/' | '\\' | ':' => sanitized.push('-'),
            _ => sanitized.push(ch),
        }
    }
    if !sanitized.starts_with('-') {
        sanitized.insert(0, '-');
    }
    sanitized
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
    fn newest_jsonl_file_age_secs_ignores_non_jsonl_files() {
        let dir = env::temp_dir().join(format!("awake-jsonl-age-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let older = dir.join("older.txt");
        let newer = dir.join("newer.jsonl");
        fs::write(&older, "a").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&newer, "b").unwrap();

        let (path, age_secs) = newest_jsonl_file_age_secs(&dir).unwrap();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("newer.jsonl")
        );
        assert!(age_secs <= 1);

        let _ = fs::remove_file(older);
        let _ = fs::remove_file(newer);
        let _ = fs::remove_dir(dir);
    }

    #[test]
    fn matching_files_returns_newest_first() {
        let dir = env::temp_dir().join(format!("awake-matching-files-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let older = dir.join("state_1.sqlite");
        let newer = dir.join("state_2.sqlite");
        fs::write(&older, "a").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&newer, "b").unwrap();

        let matches = matching_files(&dir, "state_*.sqlite");
        assert_eq!(matches, vec![newer.clone(), older.clone()]);

        let _ = fs::remove_file(older);
        let _ = fs::remove_file(newer);
        let _ = fs::remove_dir(dir);
    }

    #[test]
    fn extract_json_string_array_reads_workspace_folders() {
        let content = r#"{"workspaceFolders":["/tmp/one","/tmp/two"],"pid":123}"#;
        assert_eq!(
            extract_json_string_array(content, "workspaceFolders"),
            Some(vec!["/tmp/one".to_string(), "/tmp/two".to_string()])
        );
    }

    #[test]
    fn parse_lsof_cwd_reads_named_path() {
        let output = "p123\nfcwd\nn/Users/test/project\n";
        assert_eq!(
            parse_lsof_cwd(output),
            Some(PathBuf::from("/Users/test/project"))
        );
    }

    #[test]
    fn sanitize_claude_project_path_matches_expected_layout() {
        assert_eq!(
            sanitize_claude_project_path(Path::new("/Users/test/workspace")),
            "-Users-test-workspace"
        );
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
