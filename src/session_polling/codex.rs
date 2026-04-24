use super::{
    activity_within_window, age_from_epoch_secs, gui_app_running, home_dir, matching_files,
    process_command_lines, process_infos, sql_quote, sqlite_single_line, ProcessInfo,
    SessionPollResult, SQLITE_FIELD_SEPARATOR,
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
    source: String,
    cwd: String,
    title: String,
}

struct CodexProcessFiles {
    db_paths: Vec<std::path::PathBuf>,
    rollout_paths: Vec<std::path::PathBuf>,
}

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CODEX_GUI_APP_NAME);
    let cli_present = codex_cli_session_running();
    let runtime_present = gui_present || cli_present;
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
        .filter(|process| {
            is_codex_app_server_invocation(&process.command)
                || is_interactive_codex_invocation(&process.command)
        })
        .flat_map(|process| current_threads_for_process(&process, &fallback_db_paths))
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

            if runtime_present && (activity_within_window(age_secs) || thread.unfinished_turn) {
                return SessionPollResult {
                    active: true,
                    detail: format!(
                        "active {} session in {} present (last update {}s ago{}) ({})",
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
                    "idle — process-matched {} thread in {} updated {}s ago ({})",
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

fn current_threads_for_process(
    process: &ProcessInfo,
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
            rollout_paths
                .iter()
                .filter_map(move |rollout_path| thread_for_rollout_path(&db_path, rollout_path))
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
        source: parts.next()?.to_string(),
        cwd: parts.next()?.to_string(),
        title: parts.next()?.to_string(),
    })
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
}
