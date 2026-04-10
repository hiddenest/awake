use super::{
    activity_within_window, age_from_epoch_secs, gui_app_running, home_dir, newest_matching_file,
    process_command_lines, sqlite_single_line, SessionPollResult,
};

const CODEX_GUI_APP_NAME: &str = "Codex";
const CODEX_CLI_PROCESS_NAME: &str = "codex";

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

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CODEX_GUI_APP_NAME);
    let cli_present = codex_cli_session_running();
    let runtime_present = gui_present || cli_present;
    let db_path = match newest_matching_file(&home_dir().join(".codex"), "state_*.sqlite") {
        Some(path) => path,
        None => {
            return SessionPollResult {
                active: false,
                detail: format!(
                    "idle — no Codex state database found ({})",
                    runtime_presence_detail(gui_present, cli_present)
                ),
            }
        }
    };
    let query = "select id,updated_at,source,cwd,title,archived from threads where archived = 0 order by updated_at desc limit 1;";

    match sqlite_single_line(&db_path, query) {
        Some(row) => {
            let parts: Vec<&str> = row.split('|').collect();
            if parts.len() < 6 {
                return SessionPollResult {
                    active: false,
                    detail: "idle — Codex thread state unreadable".to_string(),
                };
            }

            let updated_at = parts[1].parse::<u64>().ok();
            let source = parts[2];
            let cwd = parts[3];
            let title = parts[4];

            if let Some(age_secs) = age_from_epoch_secs(updated_at) {
                if runtime_present && activity_within_window(age_secs) {
                    return SessionPollResult {
                        active: true,
                        detail: format!(
                            "active {} session in {} present (last update {}s ago) ({})",
                            source, cwd, age_secs, title
                        ),
                    };
                }

                return SessionPollResult {
                    active: false,
                    detail: format!(
                        "idle — latest {} thread in {} updated {}s ago ({})",
                        source,
                        cwd,
                        age_secs,
                        runtime_presence_detail(gui_present, cli_present)
                    ),
                };
            }

            SessionPollResult {
                active: false,
                detail: "idle — Codex thread timestamp unreadable".to_string(),
            }
        }
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no Codex thread state found ({})",
                runtime_presence_detail(gui_present, cli_present)
            ),
        },
    }
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
    process_command_lines(CODEX_CLI_PROCESS_NAME)
        .into_iter()
        .any(|line| is_interactive_codex_invocation(&line))
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
}
