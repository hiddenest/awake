use super::{
    age_from_epoch_millis, gui_app_running, home_dir, process_running, sqlite_single_line,
    SessionPollResult, ACTIVE_SESSION_WINDOW_SECS,
};

const OPENCODE_GUI_APP_NAME: &str = "OpenCode";
const OPENCODE_CLI_PROCESS_NAME: &str = "opencode";
const OPENCODE_ACTIVITY_QUERY: &str = "select s.id,s.title,s.directory,max(s.time_updated,ifnull((select max(m.time_updated) from message m where m.session_id = s.id),0),ifnull((select max(p.time_updated) from part p where p.session_id = s.id),0),ifnull((select max(t.time_updated) from todo t where t.session_id = s.id),0)) from session s where s.time_archived is null order by 4 desc limit 1;";

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(OPENCODE_GUI_APP_NAME);
    let cli_present = process_running(OPENCODE_CLI_PROCESS_NAME);
    let runtime_present = gui_present || cli_present;
    let db_path = home_dir().join(".local/share/opencode/opencode.db");

    match sqlite_single_line(&db_path, OPENCODE_ACTIVITY_QUERY) {
        Some(row) => {
            let Some(activity) = parse_activity_row(&row) else {
                return SessionPollResult {
                    active: false,
                    detail: "idle — OpenCode activity state unreadable".to_string(),
                };
            };

            if let Some(age_secs) = age_from_epoch_millis(Some(activity.updated_at)) {
                if runtime_present && age_secs <= ACTIVE_SESSION_WINDOW_SECS {
                    return SessionPollResult {
                        active: true,
                        detail: format!(
                            "active OpenCode {} activity in {} updated {}s ago ({})",
                            runtime_detail(gui_present, cli_present),
                            activity.directory,
                            age_secs,
                            activity.title
                        ),
                    };
                }

                let runtime = runtime_presence_detail(gui_present, cli_present);
                return SessionPollResult {
                    active: false,
                    detail: format!(
                        "idle — latest OpenCode activity in {} updated {}s ago ({})",
                        activity.directory, age_secs, runtime
                    ),
                };
            }

            SessionPollResult {
                active: false,
                detail: "idle — OpenCode activity timestamp unreadable".to_string(),
            }
        }
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no OpenCode activity state found ({})",
                runtime_presence_detail(gui_present, cli_present)
            ),
        },
    }
}

struct OpenCodeActivity<'a> {
    title: &'a str,
    directory: &'a str,
    updated_at: u64,
}

fn parse_activity_row(row: &str) -> Option<OpenCodeActivity<'_>> {
    let mut parts = row.split('|');
    let _id = parts.next()?;
    let title = parts.next()?;
    let directory = parts.next()?;
    let updated_at = parts.next()?.parse::<u64>().ok()?;
    Some(OpenCodeActivity {
        title,
        directory,
        updated_at,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_activity_row_reads_title_directory_and_timestamp() {
        let row = "ses_123|Build WXT migration|/tmp/project|1775714298852";
        let activity = parse_activity_row(row).unwrap();
        assert_eq!(activity.title, "Build WXT migration");
        assert_eq!(activity.directory, "/tmp/project");
        assert_eq!(activity.updated_at, 1_775_714_298_852);
    }

    #[test]
    fn parse_activity_row_rejects_missing_timestamp() {
        assert!(parse_activity_row("ses_123|title|/tmp/project").is_none());
    }
}
