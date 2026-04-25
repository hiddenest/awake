use super::{
    activity_within_window, age_from_epoch_millis, gui_app_running, home_dir, process_infos,
    sql_quote, sqlite_single_line, ProcessInfo, SessionPollResult, SQLITE_FIELD_SEPARATOR,
};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const OPENCODE_GUI_APP_NAME: &str = "OpenCode";
const OPENCODE_PROCESS_NAME: &str = "opencode";

const NON_INTERACTIVE_COMMANDS: [&str; 12] = [
    "completion",
    "acp",
    "mcp",
    "debug",
    "providers",
    "agent",
    "upgrade",
    "uninstall",
    "serve",
    "web",
    "models",
    "stats",
];

struct OpenCodeActivity {
    id: String,
    title: String,
    directory: String,
    created_at: u64,
    updated_at: u64,
    assistant_pending: bool,
    tool_running: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiveSessionStatus {
    Busy,
    Retry,
    Idle,
}

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(OPENCODE_GUI_APP_NAME);
    let processes = process_infos(OPENCODE_PROCESS_NAME);
    let cli_present = processes
        .iter()
        .any(|process| is_interactive_opencode_invocation(&process.command));
    let runtime_present = gui_present || cli_present;
    let db_path = home_dir().join(".local/share/opencode/opencode.db");

    let matched_activity = processes
        .into_iter()
        .filter_map(|process| opencode_session_directory(&process))
        .filter_map(|directory| root_activity_for_directory(&db_path, &directory))
        .max_by_key(|activity| activity.updated_at);

    match matched_activity {
        Some(activity) => {
            let age_secs = match age_from_epoch_millis(Some(activity.updated_at)) {
                Some(age_secs) => age_secs,
                None => {
                    return SessionPollResult {
                        active: false,
                        detail: "idle — OpenCode activity timestamp unreadable".to_string(),
                        last_activity_age_secs: None,
                        worked_for_secs: None,
                    }
                }
            };
            let worked_for_secs = age_from_epoch_millis(Some(activity.created_at));
            let live_status = discover_live_status(&activity.id);
            let log_age_secs = newest_opencode_log_age_secs();
            let pending = activity.assistant_pending || activity.tool_running;

            if runtime_present
                && matches!(
                    live_status,
                    Some(LiveSessionStatus::Busy | LiveSessionStatus::Retry)
                )
            {
                return SessionPollResult {
                    active: true,
                    detail: format!(
                        "active OpenCode {} session in {} present via live status {} (last update {}s ago{}) ({})",
                        runtime_detail(gui_present, cli_present),
                        activity.directory,
                        live_status_detail(live_status),
                        age_secs,
                        log_detail_suffix(log_age_secs),
                        activity.title
                    ),
                    last_activity_age_secs: Some(age_secs),
                    worked_for_secs,
                };
            }

            if runtime_present && pending {
                return SessionPollResult {
                    active: true,
                    detail: format!(
                        "pending OpenCode {} session in {} present (assistant pending: {}, tool running: {}, last update {}s ago{}) ({})",
                        runtime_detail(gui_present, cli_present),
                        activity.directory,
                        activity.assistant_pending,
                        activity.tool_running,
                        age_secs,
                        log_detail_suffix(log_age_secs),
                        activity.title
                    ),
                    last_activity_age_secs: Some(age_secs),
                    worked_for_secs,
                };
            }

            if runtime_present && activity_within_window(age_secs) {
                return SessionPollResult {
                    active: true,
                    detail: format!(
                        "active OpenCode {} session in {} present (last update {}s ago{}) ({})",
                        runtime_detail(gui_present, cli_present),
                        activity.directory,
                        age_secs,
                        log_detail_suffix(log_age_secs),
                        activity.title
                    ),
                    last_activity_age_secs: Some(age_secs),
                    worked_for_secs,
                };
            }

            SessionPollResult {
                active: false,
                detail: format!(
                    "idle — process-matched OpenCode activity in {} updated {}s ago ({}, live status: {}{})",
                    activity.directory,
                    age_secs,
                    runtime_presence_detail(gui_present, cli_present),
                    live_status_detail(live_status),
                    log_detail_suffix(log_age_secs)
                ),
                last_activity_age_secs: Some(age_secs),
                worked_for_secs,
            }
        }
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no OpenCode session matched a running process ({})",
                runtime_presence_detail(gui_present, cli_present)
            ),
            last_activity_age_secs: None,
            worked_for_secs: None,
        },
    }
}

fn root_activity_for_directory(
    db_path: &std::path::Path,
    directory: &std::path::Path,
) -> Option<OpenCodeActivity> {
    let normalized_directory = normalize_directory(directory);
    let query = format!(
        "select s.id,s.title,s.directory,s.time_created,max(s.time_updated,ifnull((select max(m.time_updated) from message m where m.session_id = s.id),0),ifnull((select max(p.time_updated) from part p where p.session_id = s.id),0),ifnull((select max(t.time_updated) from todo t where t.session_id = s.id),0)),exists(select 1 from message m2 where m2.session_id = s.id and json_extract(m2.data,'$.role') = 'assistant' and json_type(m2.data,'$.time.completed') is null),exists(select 1 from part p2 where p2.session_id = s.id and json_extract(p2.data,'$.type') = 'tool' and json_extract(p2.data,'$.state.status') in ('pending','running')) from session s where s.time_archived is null and s.parent_id is null and s.directory = '{}' order by 5 desc limit 1;",
        sql_quote(&normalized_directory.display().to_string())
    );
    let row = sqlite_single_line(db_path, &query)?;
    parse_activity_row(&row)
}

fn parse_activity_row(row: &str) -> Option<OpenCodeActivity> {
    let mut parts = row.split(SQLITE_FIELD_SEPARATOR);
    let id = parts.next()?.to_string();
    Some(OpenCodeActivity {
        id,
        title: parts.next()?.to_string(),
        directory: parts.next()?.to_string(),
        created_at: parts.next()?.parse::<u64>().ok()?,
        updated_at: parts.next()?.parse::<u64>().ok()?,
        assistant_pending: parse_sqlite_bool(parts.next()?)?,
        tool_running: parse_sqlite_bool(parts.next()?)?,
    })
}

fn parse_sqlite_bool(value: &str) -> Option<bool> {
    match value {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn runtime_detail(gui_present: bool, cli_present: bool) -> &'static str {
    match (gui_present, cli_present) {
        (true, true) => "GUI/CLI",
        (true, false) => "GUI",
        (false, true) => "CLI",
        (false, false) => "runtime",
    }
}

fn runtime_presence_detail(gui_present: bool, cli_present: bool) -> &'static str {
    match (gui_present, cli_present) {
        (true, true) => "GUI + CLI session present",
        (true, false) => "GUI process present",
        (false, true) => "CLI session present",
        (false, false) => "no OpenCode GUI or CLI session",
    }
}

fn discover_live_status(session_id: &str) -> Option<LiveSessionStatus> {
    candidate_server_urls().into_iter().find_map(|base_url| {
        let url = format!("{}/session/status", base_url.trim_end_matches('/'));
        let body = http_get(&url)?;
        live_status_from_json(&body, session_id)
    })
}

fn candidate_server_urls() -> Vec<String> {
    let mut urls = vec![
        "http://127.0.0.1:4096".to_string(),
        "http://localhost:4096".to_string(),
    ];

    for pid in process_infos(OPENCODE_PROCESS_NAME)
        .into_iter()
        .map(|process| process.pid)
    {
        for url in listening_server_urls(pid) {
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    }

    urls
}

fn listening_server_urls(pid: u32) -> Vec<String> {
    let output = match crate::command_output(
        "lsof",
        &[
            "-Pan",
            "-a",
            "-p",
            &pid.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
        ],
    ) {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(parse_lsof_listen_url)
        .collect()
}

fn parse_lsof_listen_url(line: &str) -> Option<String> {
    let listen_at = line.find("(LISTEN)")?;
    let prefix = &line[..listen_at];
    let address = prefix.split_whitespace().last()?;
    let host_port = address.trim();
    let host_port = host_port.rsplit("->").next().unwrap_or(host_port);
    let (host, port) = host_port.rsplit_once(':')?;
    let host = match host {
        "*" => "127.0.0.1",
        "localhost" | "127.0.0.1" | "[::1]" | "::1" => "127.0.0.1",
        _ => return None,
    };
    Some(format!("http://{}:{}", host, port))
}

fn http_get(url: &str) -> Option<String> {
    let output = crate::command_output("curl", &["-fsS", "--max-time", "1", url]).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn live_status_from_json(body: &str, session_id: &str) -> Option<LiveSessionStatus> {
    let needle = format!("\"{}\":{{", session_id);
    let start = body.find(&needle)? + needle.len();
    let tail = &body[start..];
    let end = tail.find('}').unwrap_or(tail.len());
    let object = &tail[..end];
    if object.contains("\"type\":\"busy\"") {
        return Some(LiveSessionStatus::Busy);
    }
    if object.contains("\"type\":\"retry\"") {
        return Some(LiveSessionStatus::Retry);
    }
    if object.contains("\"type\":\"idle\"") {
        return Some(LiveSessionStatus::Idle);
    }
    None
}

fn live_status_detail(status: Option<LiveSessionStatus>) -> &'static str {
    match status {
        Some(LiveSessionStatus::Busy) => "busy",
        Some(LiveSessionStatus::Retry) => "retry",
        Some(LiveSessionStatus::Idle) => "idle",
        None => "unavailable",
    }
}

fn newest_opencode_log_age_secs() -> Option<u64> {
    let log_dir = home_dir().join(".local/share/opencode/log");
    let entries = fs::read_dir(log_dir).ok()?;
    let newest = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("log") {
                return None;
            }
            let modified = metadata.modified().ok()?;
            Some((path, modified))
        })
        .max_by_key(|(_, modified)| *modified)?;
    let age_secs = SystemTime::now().duration_since(newest.1).ok()?.as_secs();
    Some(age_secs)
}

fn log_detail_suffix(log_age_secs: Option<u64>) -> String {
    match log_age_secs {
        Some(age_secs) => format!(", log heartbeat {}s ago", age_secs),
        None => String::new(),
    }
}

fn opencode_session_directory(process: &ProcessInfo) -> Option<std::path::PathBuf> {
    let tokens: Vec<&str> = process.command.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let mut idx = 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        match token {
            "--log-level" | "--port" | "--hostname" | "--mdns-domain" | "--attach"
            | "--password" | "-p" | "--dir" | "--model" | "--agent" | "--format" | "--title"
            | "--variant" | "-s" | "--session" | "-c" | "--command" | "-m" | "-f" | "--file" => {
                idx += 2;
                continue;
            }
            _ if token.starts_with("--") && token.contains('=') => {
                idx += 1;
                continue;
            }
            "--print-logs" | "--mdns" | "--fork" | "--share" | "--thinking" | "--continue"
            | "-h" | "--help" | "-v" | "--version" | "--pure" => {
                idx += 1;
                continue;
            }
            "run" | "attach" | "pr" => {
                return process.cwd.as_deref().map(normalize_directory);
            }
            token
                if NON_INTERACTIVE_COMMANDS.contains(&token)
                    || token == "session"
                    || token == "db"
                    || token == "github"
                    || token == "import"
                    || token == "export"
                    || token == "plugin" =>
            {
                return None;
            }
            _ if token.starts_with('-') => {
                idx += 1;
            }
            _ => {
                let path = PathBuf::from(token);
                if path.is_absolute() {
                    return Some(normalize_directory(&path));
                }
                return process
                    .cwd
                    .as_ref()
                    .map(|cwd| normalize_directory(&cwd.join(path)));
            }
        }
    }

    process.cwd.as_deref().map(normalize_directory)
}

fn is_interactive_opencode_invocation(command_line: &str) -> bool {
    opencode_session_directory(&ProcessInfo {
        pid: 0,
        command: command_line.to_string(),
        cwd: Some(std::path::PathBuf::from(".")),
    })
    .is_some()
}

fn normalize_directory(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_lexical_path(path))
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Normal(_) | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_process(command: &str, cwd: &str) -> ProcessInfo {
        ProcessInfo {
            pid: 1,
            command: command.to_string(),
            cwd: Some(std::path::PathBuf::from(cwd)),
        }
    }

    #[test]
    fn parse_activity_row_reads_title_directory_and_timestamp() {
        let row = format!(
            "ses_123{sep}Build WXT migration{sep}/tmp/project{sep}1775710000000{sep}1775714298852{sep}1{sep}0",
            sep = SQLITE_FIELD_SEPARATOR
        );
        let activity = parse_activity_row(&row).unwrap();
        assert_eq!(activity.id, "ses_123");
        assert_eq!(activity.title, "Build WXT migration");
        assert_eq!(activity.directory, "/tmp/project");
        assert_eq!(activity.created_at, 1_775_710_000_000);
        assert_eq!(activity.updated_at, 1_775_714_298_852);
        assert!(activity.assistant_pending);
        assert!(!activity.tool_running);
    }

    #[test]
    fn parse_activity_row_rejects_missing_timestamp() {
        assert!(parse_activity_row("ses_123|title|/tmp/project").is_none());
    }

    #[test]
    fn parse_activity_row_handles_pipe_in_title() {
        let row = format!(
            "ses_123{sep}title | with pipe{sep}/tmp/project{sep}1775710000000{sep}1775714298852{sep}0{sep}1",
            sep = SQLITE_FIELD_SEPARATOR
        );
        let activity = parse_activity_row(&row).unwrap();
        assert_eq!(activity.title, "title | with pipe");
        assert!(!activity.assistant_pending);
        assert!(activity.tool_running);
    }

    #[test]
    fn live_status_from_json_matches_session_id() {
        let body = r#"{"ses_busy":{"type":"busy"},"ses_idle":{"type":"idle"},"ses_retry":{"type":"retry","attempt":2}}"#;
        assert_eq!(
            live_status_from_json(body, "ses_busy"),
            Some(LiveSessionStatus::Busy)
        );
        assert_eq!(
            live_status_from_json(body, "ses_idle"),
            Some(LiveSessionStatus::Idle)
        );
        assert_eq!(
            live_status_from_json(body, "ses_retry"),
            Some(LiveSessionStatus::Retry)
        );
    }

    #[test]
    fn parse_lsof_listen_url_accepts_loopback() {
        let line = "opencode 99950 hiddenest   30u  IPv4 0x123 0t0 TCP 127.0.0.1:4096 (LISTEN)";
        assert_eq!(
            parse_lsof_listen_url(line),
            Some("http://127.0.0.1:4096".to_string())
        );
    }

    #[test]
    fn bare_opencode_is_interactive() {
        assert_eq!(
            opencode_session_directory(&test_process("opencode", "/tmp/project")),
            Some(std::path::PathBuf::from("/tmp/project"))
        );
    }

    #[test]
    fn opencode_project_argument_overrides_process_cwd() {
        assert_eq!(
            opencode_session_directory(&test_process("opencode worktree", "/tmp/project")),
            Some(std::path::PathBuf::from("/tmp/project/worktree"))
        );
    }

    #[test]
    fn opencode_dot_project_is_normalized() {
        assert_eq!(
            opencode_session_directory(&test_process("opencode .", "/tmp/project")),
            Some(std::path::PathBuf::from("/tmp/project"))
        );
    }

    #[test]
    fn opencode_run_is_interactive() {
        assert!(is_interactive_opencode_invocation("opencode run fix tests"));
    }

    #[test]
    fn opencode_session_list_is_not_interactive() {
        assert!(!is_interactive_opencode_invocation("opencode session list"));
    }

    #[test]
    fn opencode_server_subcommand_is_not_interactive() {
        assert!(!is_interactive_opencode_invocation(
            "opencode --port 1234 serve"
        ));
    }

    #[test]
    fn opencode_continue_flag_without_subcommand_stays_interactive() {
        assert!(is_interactive_opencode_invocation("opencode --continue"));
    }
}
