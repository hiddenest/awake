use super::{
    activity_within_window, age_from_system_time, claude_ide_lock_active, claude_ide_locks,
    claude_runtime_detail, gui_app_running, home_dir, process_infos, sanitize_claude_project_path,
    ProcessInfo, SessionPollResult,
};

const CLAUDE_GUI_APP_NAME: &str = "Claude";
const CLAUDE_PROCESS_NAME: &str = "claude";

const NON_INTERACTIVE_COMMANDS: [&str; 7] = [
    "agents",
    "auth",
    "auto-mode",
    "doctor",
    "install",
    "mcp",
    "plugin",
];

struct ClaudeSessionActivity {
    workspace: std::path::PathBuf,
    last_activity_age_secs: u64,
    worked_for_secs: Option<u64>,
}

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CLAUDE_GUI_APP_NAME);
    let ide_lock_present = claude_ide_lock_active();
    let cli_workspaces = process_infos(CLAUDE_PROCESS_NAME)
        .into_iter()
        .filter_map(|process| claude_session_workspace_from_process(&process));

    let mut workspaces: Vec<std::path::PathBuf> = claude_ide_locks()
        .into_iter()
        .flat_map(|lock| lock.workspace_folders)
        .chain(cli_workspaces)
        .collect();
    workspaces.sort();
    workspaces.dedup();

    let freshest = workspaces
        .iter()
        .filter_map(|workspace| session_activity_for_workspace(workspace))
        .min_by_key(|activity| activity.last_activity_age_secs);

    match freshest {
        Some(activity) if activity_within_window(activity.last_activity_age_secs) => {
            SessionPollResult {
                active: true,
                detail: format!(
                    "active Claude session in {} present (last update {}s ago)",
                    activity.workspace.display(),
                    activity.last_activity_age_secs
                ),
                last_activity_age_secs: Some(activity.last_activity_age_secs),
                worked_for_secs: activity.worked_for_secs,
            }
        }
        Some(activity) => SessionPollResult {
            active: false,
            detail: format!(
                "idle — last Claude session in {} updated {}s ago ({})",
                activity.workspace.display(),
                activity.last_activity_age_secs,
                claude_runtime_detail(gui_present, ide_lock_present)
            ),
            last_activity_age_secs: Some(activity.last_activity_age_secs),
            worked_for_secs: activity.worked_for_secs,
        },
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no Claude project session matched a live workspace ({})",
                claude_runtime_detail(gui_present, ide_lock_present)
            ),
            last_activity_age_secs: None,
            worked_for_secs: None,
        },
    }
}

fn session_activity_for_workspace(workspace: &std::path::Path) -> Option<ClaudeSessionActivity> {
    let project_dir = home_dir()
        .join(".claude/projects")
        .join(sanitize_claude_project_path(workspace));
    let session_file = newest_project_session_file(&project_dir)?;
    let metadata = session_file.metadata().ok()?;
    let last_activity_age_secs = age_from_system_time(metadata.modified().ok())?;
    let worked_for_secs =
        age_from_system_time(metadata.created().ok()).or_else(|| Some(last_activity_age_secs));
    Some(ClaudeSessionActivity {
        workspace: workspace.to_path_buf(),
        last_activity_age_secs,
        worked_for_secs,
    })
}

fn newest_project_session_file(project_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut newest: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
    for entry in std::fs::read_dir(project_dir).ok()? {
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
    newest.map(|(path, _)| path)
}

fn claude_session_workspace_from_process(process: &ProcessInfo) -> Option<std::path::PathBuf> {
    if !is_interactive_claude_invocation(&process.command) {
        return None;
    }
    process.cwd.clone()
}

fn is_interactive_claude_invocation(command_line: &str) -> bool {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }

    let mut idx = 1;
    while idx < tokens.len() {
        let token = tokens[idx];
        match token {
            "-p" | "--print" | "-h" | "--help" | "-v" | "--version" => return false,
            "--debug-file"
            | "--effort"
            | "--fallback-model"
            | "--file"
            | "--input-format"
            | "--json-schema"
            | "--max-budget-usd"
            | "--mcp-config"
            | "--model"
            | "-n"
            | "--name"
            | "--output-format"
            | "--permission-mode"
            | "--plugin-dir"
            | "-r"
            | "--resume"
            | "--session-id"
            | "--setting-sources"
            | "--settings"
            | "-w"
            | "--worktree"
            | "--add-dir"
            | "--allowedTools"
            | "--allowed-tools"
            | "--append-system-prompt"
            | "--agents"
            | "--betas"
            | "--disallowedTools"
            | "--disallowed-tools"
            | "--system-prompt" => {
                idx += 2;
                continue;
            }
            _ if token.starts_with("--") && token.contains('=') => {
                idx += 1;
                continue;
            }
            "--bare"
            | "--brief"
            | "--chrome"
            | "--disable-slash-commands"
            | "--fork-session"
            | "--ide"
            | "--include-hook-events"
            | "--include-partial-messages"
            | "--no-chrome"
            | "--no-session-persistence"
            | "--replay-user-messages"
            | "--strict-mcp-config"
            | "--tmux"
            | "--verbose"
            | "-c"
            | "--continue" => {
                idx += 1;
                continue;
            }
            _ if token.starts_with('-') => {
                idx += 1;
            }
            _ if NON_INTERACTIVE_COMMANDS.contains(&token)
                || token == "plugins"
                || token == "setup-token"
                || token == "update"
                || token == "upgrade" =>
            {
                return false;
            }
            _ => return true,
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_claude_is_interactive() {
        assert!(is_interactive_claude_invocation("claude"));
    }

    #[test]
    fn claude_print_is_not_interactive() {
        assert!(!is_interactive_claude_invocation("claude -p summarize"));
    }

    #[test]
    fn claude_resume_is_interactive() {
        assert!(is_interactive_claude_invocation(
            "claude --resume session-id"
        ));
    }

    #[test]
    fn claude_doctor_is_not_interactive() {
        assert!(!is_interactive_claude_invocation("claude doctor"));
    }
}
