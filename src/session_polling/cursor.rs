use super::{
    activity_within_window, age_from_system_time, gui_app_running, home_dir, process_infos,
    unfinished_activity_within_window, ProcessInfo, SessionPollResult,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const CURSOR_GUI_APP_NAME: &str = "Cursor";
const CURSOR_AGENT_PROCESS_NAME: &str = "cursor-agent";

const NON_SESSION_COMMANDS: [&str; 15] = [
    "about",
    "create-chat",
    "generate-rule",
    "help",
    "install-shell-integration",
    "login",
    "logout",
    "ls",
    "mcp",
    "models",
    "rule",
    "status",
    "uninstall-shell-integration",
    "update",
    "whoami",
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct CursorChatActivity {
    store_path: PathBuf,
    last_activity_age_secs: u64,
    worked_for_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CursorIdeActivity {
    transcript_path: PathBuf,
    last_activity_age_secs: u64,
    worked_for_secs: Option<u64>,
    unfinished_task: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorProcessKind {
    Session,
    PrintTask,
    NonSession,
}

pub(super) fn poll_session() -> SessionPollResult {
    let gui_present = gui_app_running(CURSOR_GUI_APP_NAME);
    let processes = process_infos(CURSOR_AGENT_PROCESS_NAME);
    let mut session_process_present = false;
    let mut print_task_process: Option<ProcessInfo> = None;

    for process in processes {
        match cursor_process_kind(&process.command) {
            CursorProcessKind::Session => session_process_present = true,
            CursorProcessKind::PrintTask => {
                if print_task_process.is_none() {
                    print_task_process = Some(process);
                }
            }
            CursorProcessKind::NonSession => {}
        }
    }

    if let Some(process) = print_task_process {
        return SessionPollResult {
            active: true,
            detail: format!(
                "active Cursor CLI print task present{}",
                process
                    .cwd
                    .as_ref()
                    .map(|cwd| format!(" in {}", cwd.display()))
                    .unwrap_or_default()
            ),
            last_activity_age_secs: None,
            worked_for_secs: None,
        };
    }

    if let Some(activity) = newest_cursor_ide_activity(&home_dir().join(".cursor/projects")) {
        if gui_present
            && (activity_within_window(activity.last_activity_age_secs)
                || (activity.unfinished_task
                    && unfinished_activity_within_window(activity.last_activity_age_secs)))
        {
            return SessionPollResult {
                active: true,
                detail: format!(
                    "active Cursor IDE agent session present (last transcript update {}s ago{}, {})",
                    activity.last_activity_age_secs,
                    if activity.unfinished_task {
                        ", unfinished task"
                    } else {
                        ""
                    },
                    activity.transcript_path.display()
                ),
                last_activity_age_secs: Some(activity.last_activity_age_secs),
                worked_for_secs: activity.worked_for_secs,
            };
        }
    }

    let activity = newest_cursor_chat_activity(&home_dir().join(".cursor/chats"));
    match activity {
        Some(activity)
            if session_process_present
                && activity_within_window(activity.last_activity_age_secs) =>
        {
            SessionPollResult {
                active: true,
                detail: format!(
                    "active Cursor CLI session present (last chat update {}s ago, {})",
                    activity.last_activity_age_secs,
                    activity.store_path.display()
                ),
                last_activity_age_secs: Some(activity.last_activity_age_secs),
                worked_for_secs: activity.worked_for_secs,
            }
        }
        Some(activity) => SessionPollResult {
            active: false,
            detail: format!(
                "idle — latest Cursor CLI chat updated {}s ago ({})",
                activity.last_activity_age_secs,
                runtime_presence_detail(session_process_present)
            ),
            last_activity_age_secs: Some(activity.last_activity_age_secs),
            worked_for_secs: activity.worked_for_secs,
        },
        None => SessionPollResult {
            active: false,
            detail: format!(
                "idle — no Cursor CLI chat store found ({})",
                runtime_presence_detail(session_process_present)
            ),
            last_activity_age_secs: None,
            worked_for_secs: None,
        },
    }
}

fn newest_cursor_ide_activity(projects_dir: &Path) -> Option<CursorIdeActivity> {
    newest_cursor_ide_transcript(projects_dir).and_then(|transcript_path| {
        let metadata = transcript_path.metadata().ok()?;
        let last_activity_age_secs = age_from_system_time(metadata.modified().ok())?;
        let worked_for_secs =
            age_from_system_time(metadata.created().ok()).or(Some(last_activity_age_secs));
        let unfinished_task = cursor_transcript_has_unfinished_task(&transcript_path);
        Some(CursorIdeActivity {
            transcript_path,
            last_activity_age_secs,
            worked_for_secs,
            unfinished_task,
        })
    })
}

fn newest_cursor_ide_transcript(projects_dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;

    for project_entry in fs::read_dir(projects_dir).ok()? {
        let Ok(project_entry) = project_entry else {
            continue;
        };
        let transcripts_dir = project_entry.path().join("agent-transcripts");
        let Ok(session_entries) = fs::read_dir(transcripts_dir) else {
            continue;
        };

        for session_entry in session_entries {
            let Ok(session_entry) = session_entry else {
                continue;
            };
            collect_newest_jsonl_in_dir(&session_entry.path(), &mut newest);
            collect_newest_subagent_transcript(&session_entry.path(), &mut newest);
        }
    }

    newest.map(|(path, _)| path)
}

fn collect_newest_subagent_transcript(
    session_dir: &Path,
    newest: &mut Option<(PathBuf, SystemTime)>,
) {
    let Ok(subagent_entries) = fs::read_dir(session_dir.join("subagents")) else {
        return;
    };
    for subagent_entry in subagent_entries {
        let Ok(subagent_entry) = subagent_entry else {
            continue;
        };
        let path = subagent_entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        update_newest_file(path, newest);
    }
}

fn collect_newest_jsonl_in_dir(dir: &Path, newest: &mut Option<(PathBuf, SystemTime)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        update_newest_file(path, newest);
    }
}

fn update_newest_file(path: PathBuf, newest: &mut Option<(PathBuf, SystemTime)>) {
    let Ok(metadata) = path.metadata() else {
        return;
    };
    if !metadata.is_file() {
        return;
    }
    let Ok(modified) = metadata.modified() else {
        return;
    };
    match newest {
        Some((_, current)) if modified <= *current => {}
        _ => *newest = Some((path, modified)),
    }
}

fn cursor_transcript_has_unfinished_task(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    cursor_transcript_content_has_unfinished_task(&content)
}

fn cursor_transcript_content_has_unfinished_task(content: &str) -> bool {
    let mut saw_user_turn = false;
    let mut latest_assistant_used_tool = false;
    let mut latest_assistant_final_text = false;

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        if line.contains("\"role\":\"user\"") {
            saw_user_turn = true;
            latest_assistant_used_tool = false;
            latest_assistant_final_text = false;
            continue;
        }

        if line.contains("\"role\":\"assistant\"") {
            latest_assistant_used_tool = line.contains("\"type\":\"tool_use\"");
            latest_assistant_final_text =
                line.contains("\"type\":\"text\"") && !latest_assistant_used_tool;
        }
    }

    saw_user_turn && (latest_assistant_used_tool || !latest_assistant_final_text)
}

fn newest_cursor_chat_activity(chats_dir: &Path) -> Option<CursorChatActivity> {
    newest_cursor_chat_store(chats_dir).and_then(|store_path| {
        let metadata = store_path.metadata().ok()?;
        let last_activity_age_secs = age_from_system_time(metadata.modified().ok())?;
        let worked_for_secs =
            age_from_system_time(metadata.created().ok()).or(Some(last_activity_age_secs));
        Some(CursorChatActivity {
            store_path,
            last_activity_age_secs,
            worked_for_secs,
        })
    })
}

fn newest_cursor_chat_store(chats_dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;

    for profile_entry in fs::read_dir(chats_dir).ok()? {
        let Ok(profile_entry) = profile_entry else {
            continue;
        };
        let Ok(profile_metadata) = profile_entry.metadata() else {
            continue;
        };
        if !profile_metadata.is_dir() {
            continue;
        }

        let Ok(chat_entries) = fs::read_dir(profile_entry.path()) else {
            continue;
        };
        for chat_entry in chat_entries {
            let Ok(chat_entry) = chat_entry else {
                continue;
            };
            let store_path = chat_entry.path().join("store.db");
            let Ok(metadata) = store_path.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            match &newest {
                Some((_, current)) if modified <= *current => {}
                _ => newest = Some((store_path, modified)),
            }
        }
    }

    newest.map(|(path, _)| path)
}

fn runtime_presence_detail(session_process_present: bool) -> &'static str {
    if session_process_present {
        "CLI session present"
    } else {
        "no Cursor CLI session"
    }
}

fn cursor_process_kind(command_line: &str) -> CursorProcessKind {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    if tokens.is_empty() {
        return CursorProcessKind::NonSession;
    }

    let mut idx = 1;
    let mut print_mode = false;

    while idx < tokens.len() {
        let token = tokens[idx];
        match token {
            "-h" | "--help" | "-v" | "--version" | "--list-models" => {
                return CursorProcessKind::NonSession;
            }
            "-p" | "--print" => {
                print_mode = true;
                idx += 1;
            }
            "--api-key" | "-H" | "--header" | "--output-format" | "--mode" | "--resume"
            | "--model" | "--sandbox" | "--workspace" | "-w" | "--worktree" | "--worktree-base" => {
                idx += 2;
            }
            "--cloud"
            | "-c"
            | "--plan"
            | "--continue"
            | "-f"
            | "--force"
            | "--yolo"
            | "--approve-mcps"
            | "--trust"
            | "--skip-worktree-setup"
            | "--stream-partial-output" => {
                idx += 1;
            }
            _ if token.starts_with("--") && token.contains('=') => {
                idx += 1;
            }
            "agent" => {
                idx += 1;
            }
            "resume" => return CursorProcessKind::Session,
            _ if NON_SESSION_COMMANDS.contains(&token) => return CursorProcessKind::NonSession,
            _ if token.starts_with('-') => {
                idx += 1;
            }
            _ => {
                return if print_mode {
                    CursorProcessKind::PrintTask
                } else {
                    CursorProcessKind::Session
                };
            }
        }
    }

    if print_mode {
        CursorProcessKind::PrintTask
    } else {
        CursorProcessKind::Session
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn bare_cursor_agent_is_session() {
        assert_eq!(
            cursor_process_kind("cursor-agent"),
            CursorProcessKind::Session
        );
    }

    #[test]
    fn cursor_agent_with_prompt_is_session() {
        assert_eq!(
            cursor_process_kind("cursor-agent fix the auth flow"),
            CursorProcessKind::Session
        );
    }

    #[test]
    fn cursor_agent_subcommand_with_prompt_is_session() {
        assert_eq!(
            cursor_process_kind("cursor-agent agent fix tests"),
            CursorProcessKind::Session
        );
    }

    #[test]
    fn cursor_agent_print_with_prompt_is_print_task() {
        assert_eq!(
            cursor_process_kind("cursor-agent --print --output-format stream-json fix tests"),
            CursorProcessKind::PrintTask
        );
    }

    #[test]
    fn cursor_agent_resume_is_session() {
        assert_eq!(
            cursor_process_kind("cursor-agent resume"),
            CursorProcessKind::Session
        );
    }

    #[test]
    fn cursor_agent_login_is_not_session() {
        assert_eq!(
            cursor_process_kind("cursor-agent login"),
            CursorProcessKind::NonSession
        );
    }

    #[test]
    fn cursor_agent_help_is_not_session() {
        assert_eq!(
            cursor_process_kind("cursor-agent --help"),
            CursorProcessKind::NonSession
        );
    }

    #[test]
    fn cursor_transcript_with_latest_tool_use_is_unfinished() {
        let content = r#"{"role":"user","message":{"content":[{"type":"text","text":"implement it"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"I'll inspect files."},{"type":"tool_use","name":"ReadFile","input":{"path":"a.ts"}}]}}"#;

        assert!(cursor_transcript_content_has_unfinished_task(content));
    }

    #[test]
    fn cursor_transcript_with_latest_text_only_assistant_is_finished() {
        let content = r#"{"role":"user","message":{"content":[{"type":"text","text":"implement it"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#;

        assert!(!cursor_transcript_content_has_unfinished_task(content));
    }

    #[test]
    fn newest_cursor_chat_store_returns_newest_nested_store() {
        let dir = std::env::temp_dir().join(format!("awake-cursor-chats-{}", process::id()));
        let older_dir = dir.join("profile-a/chat-a");
        let newer_dir = dir.join("profile-a/chat-b");
        fs::create_dir_all(&older_dir).unwrap();
        fs::create_dir_all(&newer_dir).unwrap();
        let older = older_dir.join("store.db");
        let newer = newer_dir.join("store.db");
        fs::write(&older, "older").unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&newer, "newer").unwrap();

        assert_eq!(newest_cursor_chat_store(&dir), Some(newer.clone()));

        let _ = fs::remove_file(older);
        let _ = fs::remove_file(newer);
        let _ = fs::remove_dir_all(dir);
    }
}
