use super::{
    age_from_epoch_millis, gui_app_running, home_dir, process_running, sqlite_single_line,
    SessionPollResult, ACTIVE_SESSION_WINDOW_SECS,
};

const OPENCODE_GUI_APP_NAME: &str = "OpenCode";
const OPENCODE_CLI_PROCESS_NAME: &str = "opencode";

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(OPENCODE_GUI_APP_NAME);
    let cli_present = process_running(OPENCODE_CLI_PROCESS_NAME);
    let runtime_present = gui_present || cli_present;
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
                if runtime_present && age_secs <= ACTIVE_SESSION_WINDOW_SECS {
                    return SessionPollResult {
                        active: true,
                        detail: format!(
                            "active OpenCode {} session in {} updated {}s ago ({})",
                            runtime_detail(gui_present, cli_present),
                            directory,
                            age_secs,
                            title
                        ),
                    };
                }

                let runtime = runtime_presence_detail(gui_present, cli_present);
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
                runtime_presence_detail(gui_present, cli_present)
            ),
        },
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
        (true, true) => "GUI + CLI process present",
        (true, false) => "GUI process present",
        (false, true) => "CLI process present",
        (false, false) => "no OpenCode GUI or CLI process",
    }
}
