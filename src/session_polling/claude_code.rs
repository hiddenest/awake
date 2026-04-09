use super::{
    claude_ide_lock_active, claude_runtime_detail, gui_app_running, home_dir, newest_file_age_secs,
    SessionPollResult, ACTIVE_SESSION_WINDOW_SECS,
};

const CLAUDE_GUI_APP_NAME: &str = "Claude";

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CLAUDE_GUI_APP_NAME);
    let ide_lock_present = claude_ide_lock_active();
    let transcript_dir = home_dir().join(".claude/transcripts");

    match newest_file_age_secs(&transcript_dir) {
        Some((path, age_secs))
            if (gui_present || ide_lock_present) && age_secs <= ACTIVE_SESSION_WINDOW_SECS =>
        {
            SessionPollResult {
                active: true,
                detail: format!(
                    "active session transcript {} updated {}s ago",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("unknown"),
                    age_secs
                ),
            }
        }
        Some((path, age_secs)) => SessionPollResult {
            active: false,
            detail: format!(
                "idle — last transcript {} updated {}s ago ({})",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown"),
                age_secs,
                claude_runtime_detail(gui_present, ide_lock_present)
            ),
        },
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no Claude Code transcripts found ({})",
                claude_runtime_detail(gui_present, ide_lock_present)
            ),
        },
    }
}
