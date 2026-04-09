use super::{
    age_from_epoch_millis, gui_app_running, gui_runtime_detail, home_dir, sqlite_single_line,
    SessionPollResult, ACTIVE_SESSION_WINDOW_SECS,
};

const OPENCODE_GUI_APP_NAME: &str = "OpenCode";

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(OPENCODE_GUI_APP_NAME);
    let db_path = home_dir().join(".local/share/opencode/opencode.db");
    let query = "select id,title,time_updated,directory from session where time_archived is null order by time_updated desc limit 1;";

    match sqlite_single_line(&db_path, query) {
        Some(row) => {
            let parts: Vec<&str> = row.split('|').collect();
            if parts.len() < 4 {
                return SessionPollResult {
                    active: false,
                    detail: "idle — OpenCode session state unreadable".to_string(),
                };
            }

            let title = parts[1];
            let updated_at = parts[2].parse::<u64>().ok();
            let directory = parts[3];

            if let Some(age_secs) = age_from_epoch_millis(updated_at) {
                if gui_present && age_secs <= ACTIVE_SESSION_WINDOW_SECS {
                    return SessionPollResult {
                        active: true,
                        detail: format!(
                            "active OpenCode GUI session in {} updated {}s ago ({})",
                            directory, age_secs, title
                        ),
                    };
                }

                let runtime = if gui_present {
                    "GUI process present"
                } else {
                    "no OpenCode GUI process"
                };
                return SessionPollResult {
                    active: false,
                    detail: format!(
                        "idle — latest OpenCode session in {} updated {}s ago ({})",
                        directory, age_secs, runtime
                    ),
                };
            }

            SessionPollResult {
                active: false,
                detail: "idle — OpenCode session timestamp unreadable".to_string(),
            }
        }
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no OpenCode session state found ({})",
                gui_runtime_detail(gui_present, OPENCODE_GUI_APP_NAME)
            ),
        },
    }
}
