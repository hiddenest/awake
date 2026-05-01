use super::{
    activity_within_window, age_from_epoch_secs, age_from_system_time, gui_app_running, home_dir,
    matching_files, process_command_lines, process_infos, sql_quote, sqlite_single_line,
    unfinished_activity_within_window, ProcessInfo, SessionPollResult, SQLITE_FIELD_SEPARATOR,
};

const CODEX_GUI_APP_NAME: &str = "Codex";
const CODEX_PROCESS_NAME: &str = "codex";

const NON_INTERACTIVE_SUBCOMMANDS: [&str; 11] = [
    "app-server",
    "mcp-server",
    "completion",
    "sandbox",
    "debug",
    "features",
    "help",
    "login",
    "logout",
    "app",
    "cloud",
];

struct CodexThread {
    created_at: u64,
    updated_at: u64,
    unfinished_turn: bool,
    process_kind: CodexProcessKind,
    source: String,
    cwd: String,
    title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodexProcessKind {
    AppServer,
    InteractiveCli,
}

struct CodexProcessFiles {
    db_paths: Vec<std::path::PathBuf>,
    rollout_paths: Vec<std::path::PathBuf>,
}

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CODEX_GUI_APP_NAME);
    let cli_present = codex_cli_session_running();
    let fallback_db_paths = matching_files(&home_dir().join(".codex"), "state_*.sqlite");
    if fallback_db_paths.is_empty() {
        return SessionPollResult {
            active: false,
            detail: format!(
                "idle — no Codex state database found ({})",
                runtime_presence_detail(gui_present, cli_present)
            ),
            last_activity_age_secs: None,
            worked_for_secs: None,
        };
    }

    let matched_thread = process_infos(CODEX_PROCESS_NAME)
        .into_iter()
        .filter_map(|process| codex_process_kind(&process.command).map(|kind| (process, kind)))
        .flat_map(|(process, kind)| current_threads_for_process(&process, kind, &fallback_db_paths))
        .max_by_key(|thread| thread.updated_at);

    match matched_thread {
        None => {
            return SessionPollResult {
                active: false,
                detail: format!(
                    "idle — no Codex thread matched a running process ({})",
                    runtime_presence_detail(gui_present, cli_present)
                ),
                last_activity_age_secs: None,
                worked_for_secs: None,
            }
        }
        Some(thread) => {
            let age_secs = match age_from_epoch_secs(Some(thread.updated_at)) {
                Some(age_secs) => age_secs,
                None => {
                    return SessionPollResult {
                        active: false,
                        detail: "idle — Codex thread timestamp unreadable".to_string(),
                        last_activity_age_secs: None,
                        worked_for_secs: None,
                    }
                }
            };
            let worked_for_secs = age_from_epoch_secs(Some(thread.created_at));

            if codex_thread_is_active(&thread, age_secs, gui_present, cli_present) {
                return SessionPollResult {
                    active: true,
                    detail: format!(
                        "active {} {} session in {} present (last update {}s ago{}) ({})",
                        thread.process_kind.detail_label(),
                        thread.source,
                        thread.cwd,
                        age_secs,
                        if thread.unfinished_turn {
                            ", unfinished turn"
                        } else {
                            ""
                        },
                        thread.title
                    ),
                    last_activity_age_secs: Some(age_secs),
                    worked_for_secs,
                };
            }

            SessionPollResult {
                active: false,
                detail: format!(
                    "idle — process-matched {} {} thread in {} updated {}s ago ({})",
                    thread.process_kind.detail_label(),
                    thread.source,
                    thread.cwd,
                    age_secs,
                    runtime_presence_detail(gui_present, cli_present)
                ),
                last_activity_age_secs: Some(age_secs),
                worked_for_secs,
            }
        }
    }
}

pub(super) fn debug_report() -> String {
    use std::fmt::Write as _;

    let mut report = String::new();
    let gui_present = gui_app_running(CODEX_GUI_APP_NAME);
    let cli_present = codex_cli_session_running();
    let fallback_db_paths = matching_files(&home_dir().join(".codex"), "state_*.sqlite");
    let poll = poll_session();

    let _ = writeln!(report, "[awake] Codex internals:");
    let _ = writeln!(report, "  poll.active: {}", poll.active);
    let _ = writeln!(report, "  poll.detail: {}", poll.detail);
    let _ = writeln!(
        report,
        "  poll.last_activity_age_secs: {}",
        optional_secs(poll.last_activity_age_secs)
    );
    let _ = writeln!(
        report,
        "  poll.worked_for_secs: {}",
        optional_secs(poll.worked_for_secs)
    );
    let _ = writeln!(
        report,
        "  runtime: gui_present={}, cli_present={} ({})",
        gui_present,
        cli_present,
        runtime_presence_detail(gui_present, cli_present)
    );

    let _ = writeln!(
        report,
        "  fallback state DBs ({}):",
        fallback_db_paths.len()
    );
    if fallback_db_paths.is_empty() {
        let _ = writeln!(report, "    none");
    } else {
        for path in &fallback_db_paths {
            let _ = writeln!(report, "    {}", path_with_age(path));
        }
    }

    let processes = process_infos(CODEX_PROCESS_NAME);
    let _ = writeln!(report, "  codex processes ({}):", processes.len());
    if processes.is_empty() {
        let _ = writeln!(report, "    none");
        return report;
    }

    for process in processes {
        let process_kind = codex_process_kind(&process.command);
        let _ = writeln!(
            report,
            "    pid={} kind={} cwd={}",
            process.pid,
            process_kind
                .map(CodexProcessKind::detail_label)
                .unwrap_or("ignored"),
            process
                .cwd
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
        let _ = writeln!(report, "      command: {}", process.command);

        let CodexProcessFiles {
            db_paths,
            rollout_paths,
        } = codex_process_files(&process);
        let _ = writeln!(report, "      lsof state DBs ({}):", db_paths.len());
        if db_paths.is_empty() {
            let _ = writeln!(report, "        none");
        } else {
            for path in &db_paths {
                let _ = writeln!(report, "        {}", path_with_age(path));
            }
        }

        let _ = writeln!(report, "      writable rollouts ({}):", rollout_paths.len());
        if rollout_paths.is_empty() {
            let _ = writeln!(report, "        none");
            continue;
        }

        let candidate_db_paths = if db_paths.is_empty() {
            fallback_db_paths.clone()
        } else {
            db_paths
        };

        for rollout_path in &rollout_paths {
            let unfinished_turn = rollout_has_unfinished_turn(rollout_path);
            let _ = writeln!(
                report,
                "        {} unfinished_turn={}",
                path_with_age(rollout_path),
                unfinished_turn
            );

            let Some(kind) = process_kind else {
                let _ = writeln!(
                    report,
                    "          thread lookup skipped for ignored process"
                );
                continue;
            };

            if candidate_db_paths.is_empty() {
                let _ = writeln!(report, "          no DB available for thread lookup");
                continue;
            }

            for db_path in &candidate_db_paths {
                match thread_for_rollout_path(db_path, rollout_path, kind) {
                    Some(thread) => {
                        let age_secs = age_from_epoch_secs(Some(thread.updated_at));
                        let active = age_secs
                            .map(|age| {
                                codex_thread_is_active(&thread, age, gui_present, cli_present)
                            })
                            .unwrap_or(false);
                        let _ = writeln!(
                            report,
                            "          thread db={} source={} cwd={} updated={} created={} active={} title={}",
                            db_path.display(),
                            thread.source,
                            thread.cwd,
                            optional_secs(age_secs),
                            optional_secs(age_from_epoch_secs(Some(thread.created_at))),
                            active,
                            thread.title
                        );
                    }
                    None => {
                        let _ = writeln!(
                            report,
                            "          no thread match in db={}",
                            db_path.display()
                        );
                    }
                }
            }
        }
    }

    report
}

fn optional_secs(value: Option<u64>) -> String {
    value
        .map(|secs| format!("{}s", secs))
        .unwrap_or_else(|| "unknown".to_string())
}

fn path_with_age(path: &std::path::Path) -> String {
    let age = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| age_from_system_time(Some(modified)))
        .map(|secs| format!("modified {}s ago", secs))
        .unwrap_or_else(|| "modified unknown".to_string());
    format!("{} ({})", path.display(), age)
}

fn current_threads_for_process(
    process: &ProcessInfo,
    process_kind: CodexProcessKind,
    fallback_db_paths: &[std::path::PathBuf],
) -> Vec<CodexThread> {
    let CodexProcessFiles {
        db_paths,
        rollout_paths,
    } = codex_process_files(process);
    if rollout_paths.is_empty() {
        return Vec::new();
    }

    let db_paths = if db_paths.is_empty() {
        fallback_db_paths.to_vec()
    } else {
        db_paths
    };

    db_paths
        .into_iter()
        .flat_map(|db_path| {
            rollout_paths.iter().filter_map(move |rollout_path| {
                thread_for_rollout_path(&db_path, rollout_path, process_kind)
            })
        })
        .collect()
}

fn codex_process_files(process: &ProcessInfo) -> CodexProcessFiles {
    let output = match crate::command_output("lsof", &["-p", &process.pid.to_string()]) {
        Ok(output) if output.status.success() => output,
        _ => {
            return CodexProcessFiles {
                db_paths: Vec::new(),
                rollout_paths: Vec::new(),
            }
        }
    };
    parse_codex_process_files(&String::from_utf8_lossy(&output.stdout))
}

fn parse_codex_process_files(output: &str) -> CodexProcessFiles {
    let mut db_paths = Vec::new();
    let mut rollout_paths = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }

        let fd = parts[3];
        let Some(path) = parts.last() else {
            continue;
        };

        if path.contains("/.codex/sessions/")
            && path.contains("rollout-")
            && path.ends_with(".jsonl")
            && fd.ends_with('w')
        {
            rollout_paths.push(std::path::PathBuf::from(path));
            continue;
        }

        if path.contains("/.codex/state_") && path.ends_with(".sqlite") {
            db_paths.push(std::path::PathBuf::from(path));
        }
    }

    db_paths.sort();
    db_paths.dedup();
    rollout_paths.sort();
    rollout_paths.dedup();

    CodexProcessFiles {
        db_paths,
        rollout_paths,
    }
}

fn thread_for_rollout_path(
    db_path: &std::path::Path,
    rollout_path: &std::path::Path,
    process_kind: CodexProcessKind,
) -> Option<CodexThread> {
    let query = format!(
        "select created_at,updated_at,source,cwd,title from threads where archived = 0 and rollout_path = '{}' limit 1;",
        sql_quote(&rollout_path.display().to_string())
    );
    let row = sqlite_single_line(db_path, &query)?;
    let mut parts = row.split(SQLITE_FIELD_SEPARATOR);
    Some(CodexThread {
        created_at: parts.next()?.parse().ok()?,
        updated_at: parts.next()?.parse().ok()?,
        unfinished_turn: rollout_has_unfinished_turn(rollout_path),
        process_kind,
        source: parts.next()?.to_string(),
        cwd: parts.next()?.to_string(),
        title: parts.next()?.to_string(),
    })
}

fn codex_thread_is_active(
    thread: &CodexThread,
    age_secs: u64,
    gui_present: bool,
    cli_present: bool,
) -> bool {
    match thread.process_kind {
        CodexProcessKind::AppServer => {
            gui_present && thread.unfinished_turn && activity_within_window(age_secs)
        }
        CodexProcessKind::InteractiveCli => {
            cli_present
                && (activity_within_window(age_secs)
                    || (thread.unfinished_turn && unfinished_activity_within_window(age_secs)))
        }
    }
}

fn rollout_has_unfinished_turn(path: &std::path::Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    rollout_content_has_unfinished_turn(&content)
}

fn rollout_content_has_unfinished_turn(content: &str) -> bool {
    let mut latest_turn_id: Option<String> = None;
    let mut latest_turn_completed = false;

    for line in content.lines() {
        match top_level_rollout_event_type(line) {
            Some("turn_context") => {
                latest_turn_id = extract_json_string(line, "turn_id");
                latest_turn_completed = false;
            }
            Some("task_complete") => {
                if latest_turn_id.as_deref() == extract_json_string(line, "turn_id").as_deref() {
                    latest_turn_completed = true;
                }
            }
            _ => {}
        }
    }

    latest_turn_id.is_some() && !latest_turn_completed
}

fn top_level_rollout_event_type(line: &str) -> Option<&str> {
    let after_timestamp = line.strip_prefix("{\"timestamp\":\"")?;
    let type_key = after_timestamp.find("\"type\":\"")?;
    let value = &after_timestamp[type_key + "\"type\":\"".len()..];
    let end = value.find('"')?;
    Some(&value[..end])
}

fn extract_json_string(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":\"", key);
    let start = line.find(&needle)? + needle.len();
    let value = &line[start..];
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

fn runtime_presence_detail(gui_present: bool, cli_present: bool) -> &'static str {
    match (gui_present, cli_present) {
        (true, true) => "GUI app + CLI session present",
        (true, false) => "GUI app present",
        (false, true) => "CLI session present",
        (false, false) => "no Codex GUI or CLI session",
    }
}

fn codex_cli_session_running() -> bool {
    process_command_lines(CODEX_PROCESS_NAME)
        .into_iter()
        .any(|line| is_interactive_codex_invocation(&line))
}

fn codex_process_kind(command_line: &str) -> Option<CodexProcessKind> {
    if is_codex_app_server_invocation(command_line) {
        return Some(CodexProcessKind::AppServer);
    }

    if is_interactive_codex_invocation(command_line) {
        return Some(CodexProcessKind::InteractiveCli);
    }

    None
}

impl CodexProcessKind {
    fn detail_label(self) -> &'static str {
        match self {
            CodexProcessKind::AppServer => "app-server",
            CodexProcessKind::InteractiveCli => "CLI",
        }
    }
}

fn is_codex_app_server_invocation(command_line: &str) -> bool {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    matches!(tokens.get(1), Some(&"app-server"))
}

fn is_interactive_codex_invocation(command_line: &str) -> bool {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }

    let mut idx = 1;
    while idx < tokens.len() {
        let token = tokens[idx];

        if matches!(
            token,
            "-m" | "--model"
                | "--provider"
                | "-a"
                | "--approval-policy"
                | "-c"
                | "--config"
                | "--cd"
                | "--notify"
                | "--session"
        ) {
            idx += 2;
            continue;
        }

        if token.starts_with("--") && token.contains('=') {
            idx += 1;
            continue;
        }

        if token.starts_with('-') {
            idx += 1;
            continue;
        }

        if NON_INTERACTIVE_SUBCOMMANDS.contains(&token) {
            return false;
        }

        return true;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_thread(process_kind: CodexProcessKind, unfinished_turn: bool) -> CodexThread {
        CodexThread {
            created_at: 1,
            updated_at: 2,
            unfinished_turn,
            process_kind,
            source: "test".to_string(),
            cwd: "/tmp/project".to_string(),
            title: "test thread".to_string(),
        }
    }

    #[test]
    fn parse_codex_process_files_keeps_live_rollouts_and_state_dbs() {
        let output = "\
codex 123 user 15u REG 1,16 1 /Users/test/.codex/state_5.sqlite\n\
codex 123 user 16u REG 1,16 1 /Users/test/.codex/state_v2.sqlite\n\
codex 123 user 46w REG 1,16 1 /Users/test/.codex/sessions/2026/04/10/rollout-a.jsonl\n\
codex 123 user 47r REG 1,16 1 /Users/test/.codex/sessions/2026/04/10/rollout-b.jsonl\n\
codex 123 user 48w REG 1,16 1 /Users/test/.codex/archived_sessions/rollout-c.jsonl\n";
        let files = parse_codex_process_files(output);
        assert_eq!(
            files.db_paths,
            vec![
                std::path::PathBuf::from("/Users/test/.codex/state_5.sqlite"),
                std::path::PathBuf::from("/Users/test/.codex/state_v2.sqlite"),
            ]
        );
        assert_eq!(
            files.rollout_paths,
            vec![std::path::PathBuf::from(
                "/Users/test/.codex/sessions/2026/04/10/rollout-a.jsonl"
            )]
        );
    }

    #[test]
    fn bare_codex_is_interactive() {
        assert!(is_interactive_codex_invocation("codex"));
    }

    #[test]
    fn codex_with_prompt_is_interactive() {
        assert!(is_interactive_codex_invocation(
            "codex fix the login bug in auth.rs"
        ));
    }

    #[test]
    fn codex_with_flags_and_prompt_is_interactive() {
        assert!(is_interactive_codex_invocation(
            "codex --model o4-mini fix the login bug"
        ));
    }

    #[test]
    fn codex_exec_is_interactive() {
        assert!(is_interactive_codex_invocation("codex exec add tests"));
    }

    #[test]
    fn codex_review_is_interactive() {
        assert!(is_interactive_codex_invocation("codex review"));
    }

    #[test]
    fn codex_resume_is_interactive() {
        assert!(is_interactive_codex_invocation("codex resume --last"));
    }

    #[test]
    fn codex_fork_is_interactive() {
        assert!(is_interactive_codex_invocation("codex fork --last"));
    }

    #[test]
    fn codex_app_server_is_not_interactive() {
        assert!(!is_interactive_codex_invocation(
            "codex app-server --analytics-default-enabled"
        ));
    }

    #[test]
    fn codex_app_server_is_detected_separately() {
        assert!(is_codex_app_server_invocation(
            "codex app-server --analytics-default-enabled"
        ));
    }

    #[test]
    fn codex_process_kind_detects_app_server_before_interactive_cli() {
        assert_eq!(
            codex_process_kind("codex app-server --analytics-default-enabled"),
            Some(CodexProcessKind::AppServer)
        );
    }

    #[test]
    fn codex_process_kind_detects_interactive_cli() {
        assert_eq!(
            codex_process_kind("codex --model o4-mini fix the login bug"),
            Some(CodexProcessKind::InteractiveCli)
        );
    }

    #[test]
    fn codex_process_kind_rejects_non_interactive_subcommands() {
        assert_eq!(codex_process_kind("codex mcp-server"), None);
    }

    #[test]
    fn codex_mcp_server_is_not_interactive() {
        assert!(!is_interactive_codex_invocation("codex mcp-server"));
    }

    #[test]
    fn codex_completion_is_not_interactive() {
        assert!(!is_interactive_codex_invocation("codex completion bash"));
    }

    #[test]
    fn codex_help_is_not_interactive() {
        assert!(!is_interactive_codex_invocation("codex help"));
    }

    #[test]
    fn codex_login_is_not_interactive() {
        assert!(!is_interactive_codex_invocation("codex login"));
    }

    #[test]
    fn codex_with_only_flags_is_interactive() {
        assert!(is_interactive_codex_invocation(
            "codex --model o4-mini --full-auto"
        ));
    }

    #[test]
    fn codex_sandbox_is_not_interactive() {
        assert!(!is_interactive_codex_invocation("codex sandbox ls"));
    }

    #[test]
    fn codex_absolute_path_app_server_is_not_interactive() {
        assert!(!is_interactive_codex_invocation(
            "/Applications/Codex.app/Contents/Resources/codex app-server --analytics-default-enabled"
        ));
    }

    #[test]
    fn rollout_content_with_open_turn_is_unfinished() {
        let content = r#"{"timestamp":"2026-04-24T13:00:00.000Z","type":"turn_context","payload":{"turn_id":"turn-a"}}
{"timestamp":"2026-04-24T13:00:01.000Z","type":"response_item","payload":{"type":"function_call","call_id":"call-a"}}"#;

        assert!(rollout_content_has_unfinished_turn(content));
    }

    #[test]
    fn rollout_content_with_completed_latest_turn_is_finished() {
        let content = r#"{"timestamp":"2026-04-24T13:00:00.000Z","type":"turn_context","payload":{"turn_id":"turn-a"}}
{"timestamp":"2026-04-24T13:00:01.000Z","type":"task_complete","turn_id":"turn-a","payload":{}}
{"timestamp":"2026-04-24T13:00:02.000Z","type":"turn_context","payload":{"turn_id":"turn-b"}}
{"timestamp":"2026-04-24T13:00:03.000Z","type":"task_complete","turn_id":"turn-b","payload":{}}"#;

        assert!(!rollout_content_has_unfinished_turn(content));
    }

    #[test]
    fn rollout_content_ignores_nested_task_complete_text() {
        let content = r#"{"timestamp":"2026-04-24T13:00:00.000Z","type":"turn_context","payload":{"turn_id":"turn-a"}}
{"timestamp":"2026-04-24T13:00:01.000Z","type":"event_msg","payload":{"aggregated_output":"{\"type\":\"task_complete\",\"turn_id\":\"turn-a\"}"}}"#;

        assert!(rollout_content_has_unfinished_turn(content));
    }

    #[test]
    fn app_server_thread_requires_unfinished_turn_even_when_recent() {
        let thread = codex_thread(CodexProcessKind::AppServer, false);

        assert!(!codex_thread_is_active(&thread, 0, true, false));
    }

    #[test]
    fn app_server_thread_is_active_for_unfinished_recent_turn() {
        let thread = codex_thread(CodexProcessKind::AppServer, true);

        assert!(codex_thread_is_active(&thread, 0, true, false));
    }

    #[test]
    fn app_server_thread_requires_gui_presence() {
        let thread = codex_thread(CodexProcessKind::AppServer, true);

        assert!(!codex_thread_is_active(&thread, 0, false, false));
    }

    #[test]
    fn app_server_thread_rejects_unfinished_turn_after_fresh_window() {
        let thread = codex_thread(CodexProcessKind::AppServer, true);

        assert!(!codex_thread_is_active(
            &thread,
            super::super::ACTIVE_SESSION_WINDOW_SECS + 1,
            true,
            false
        ));
    }

    #[test]
    fn cli_thread_accepts_quiet_unfinished_turn() {
        let thread = codex_thread(CodexProcessKind::InteractiveCli, true);

        assert!(codex_thread_is_active(
            &thread,
            super::super::ACTIVE_SESSION_WINDOW_SECS + 1,
            false,
            true
        ));
    }

    #[test]
    fn cli_thread_accepts_recent_finished_activity() {
        let thread = codex_thread(CodexProcessKind::InteractiveCli, false);

        assert!(codex_thread_is_active(&thread, 0, false, true));
    }
}
