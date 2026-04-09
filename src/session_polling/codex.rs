use super::{
    age_from_epoch_secs, gui_app_running, gui_runtime_detail, home_dir, newest_matching_file,
    sqlite_single_line, SessionPollResult,
};

const CODEX_GUI_APP_NAME: &str = "Codex";

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CODEX_GUI_APP_NAME);
    let db_path = match newest_matching_file(&home_dir().join(".codex"), "state_*.sqlite") {
        Some(path) => path,
        None => {
            return SessionPollResult {
                active: false,
                detail: format!(
                    "idle — no Codex state database found ({})",
                    gui_runtime_detail(gui_present, CODEX_GUI_APP_NAME)
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
                if gui_present {
                    return SessionPollResult {
                        active: true,
                        detail: format!(
                            "active {} session in {} present (last update {}s ago) ({})",
                            source, cwd, age_secs, title
                        ),
                    };
                }

                let runtime = if gui_present {
                    "GUI app present"
                } else {
                    "no Codex GUI app"
                };
                return SessionPollResult {
                    active: false,
                    detail: format!(
                        "idle — latest {} thread in {} updated {}s ago ({})",
                        source, cwd, age_secs, runtime
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
            detail: "idle — no Codex thread state found".to_string(),
        },
    }
}
