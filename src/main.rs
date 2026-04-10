use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::flag;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

mod session_polling;

use session_polling::{poll_target_session, ACTIVE_SESSION_WINDOW_SECS, TARGETS};

const PID_FILE: &str = "/tmp/awake.pid";
const PMSET_STATE_FILE: &str = "/tmp/awake.pmset-state";
const DEFAULT_CAFFEINATE_FLAGS: &str = "di";
const POLL_INTERVAL_SECS: u64 = 5;
const WAKE_GRACE_TOLERANCE_SECS: u64 = 2;

#[derive(Clone)]
struct AppConfig {
    system_install_path: PathBuf,
    pmset_sudoers_path: PathBuf,
}

struct TargetState {
    name: &'static str,
    caffeinate_child: Option<Child>,
}

impl TargetState {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            caffeinate_child: None,
        }
    }
}

struct DaemonState {
    targets: Vec<TargetState>,
    pmset_disabled_by_awake: bool,
    pmset_original_sleep_disabled: Option<String>,
    pmset_warning_shown: bool,
    caffeinate_flags: String,
    cleanup_done: bool,
}

enum ParseFlagsResult {
    Ok(String),
    Help,
    Err,
}

enum DaemonStatus {
    PidFile(u32),
    Orphan(u32),
}

fn main() {
    let config = AppConfig {
        system_install_path: PathBuf::from(
            env::var("AWAKE_INSTALL_PATH").unwrap_or_else(|_| "/usr/local/bin/awake".to_string()),
        ),
        pmset_sudoers_path: PathBuf::from(
            env::var("AWAKE_PMSET_SUDOERS_PATH")
                .unwrap_or_else(|_| "/etc/sudoers.d/awake-pmset".to_string()),
        ),
    };

    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("start") => cmd_start(&config, &args[2..]),
        Some("stop") => cmd_stop(),
        Some("status") => cmd_status(),
        Some("install") => cmd_install(&config, &args[2..]),
        Some("setup") => cmd_setup(&config, &args[2..]),
        Some("uninstall") => cmd_uninstall(),
        _ => {
            usage(&args[0]);
            process::exit(1);
        }
    }
}

fn usage(program: &str) {
    println!(
        "Usage:\n  {0} start [options]\n    Start the awake daemon in the foreground. The daemon polls session state every {1} seconds and only keeps the Mac awake while a real session is actively progressing.\n\n  {0} stop\n    Stop the running awake daemon and release any active caffeinate/pmset state managed by it.\n\n  {0} status\n    Show whether the daemon is running, the current session state for Claude Code, Codex, and OpenCode, and the current SleepDisabled state reported by pmset.\n\n  {0} install [options]\n    Write and load the LaunchAgent plist in ~/Library/LaunchAgents so awake starts automatically at login.\n\n  {0} setup [options]\n    Install or reuse /usr/local/bin/awake, install the LaunchAgent, and optionally configure the pmset sudoers rule for lid-close sleep prevention.\n\n  {0} uninstall\n    Unload and remove the installed LaunchAgent plist.\n\nOptions for start/install/setup:\n  -D, -d, --display      Keep the display awake while a session is active (caffeinate -d).\n  -i, --idle-system      Prevent idle system sleep while a session is active (caffeinate -i).\n\nBehavior notes:\n  - Session providers: claude-code, codex, opencode\n  - OpenCode is treated as active only when a running OpenCode process matches a non-archived root session for the same directory and that session updated recently\n  - Codex is treated as active only when a running Codex process maps to a live rollout file and the matching thread updated recently\n  - Claude Code is treated as active only when a live Claude workspace maps to a project session file that updated recently\n  - A session update is considered active for up to {2} seconds after its latest observed activity\n  - If no options are provided, awake uses the default caffeinate flags: -{3}\n  - setup/install pass the selected flags through to the LaunchAgent configuration",
        program, POLL_INTERVAL_SECS, ACTIVE_SESSION_WINDOW_SECS, DEFAULT_CAFFEINATE_FLAGS
    );
}

fn cmd_start(config: &AppConfig, raw_args: &[String]) {
    let caffeinate_flags = match parse_caffeinate_flags(raw_args) {
        ParseFlagsResult::Ok(flags) => flags,
        ParseFlagsResult::Help => process::exit(0),
        ParseFlagsResult::Err => process::exit(1),
    };

    let pid_file = Path::new(PID_FILE);
    if pid_file.exists() {
        match read_pid_file(pid_file) {
            Some(existing_pid) if process_alive(existing_pid) => {
                eprintln!("[awake] Already running (PID {})", existing_pid);
                process::exit(1);
            }
            Some(_) | None => {
                println!("[awake] Removing stale PID file");
                let _ = fs::remove_file(pid_file);
            }
        }
    }

    if let Some(pid) = find_running_awake_daemon(Some(process::id())) {
        eprintln!("[awake] Already running (PID {}, no PID file present)", pid);
        process::exit(1);
    }

    if let Err(err) = fs::write(pid_file, format!("{}\n", process::id())) {
        eprintln!("[awake] Failed to write PID file: {}", err);
        process::exit(1);
    }

    let terminate = Arc::new(AtomicBool::new(false));
    if let Err(err) = flag::register(SIGTERM, Arc::clone(&terminate)) {
        eprintln!("[awake] Failed to register SIGTERM handler: {}", err);
        process::exit(1);
    }
    if let Err(err) = flag::register(SIGINT, Arc::clone(&terminate)) {
        eprintln!("[awake] Failed to register SIGINT handler: {}", err);
        process::exit(1);
    }

    println!(
        "[awake] Started (PID {}, caffeinate -{})",
        process::id(),
        caffeinate_flags
    );

    recover_stale_pmset_state();

    let mut state = DaemonState {
        targets: TARGETS.iter().copied().map(TargetState::new).collect(),
        pmset_disabled_by_awake: false,
        pmset_original_sleep_disabled: None,
        pmset_warning_shown: false,
        caffeinate_flags,
        cleanup_done: false,
    };
    let mut wake_grace_until: Option<Instant> = None;

    while !terminate.load(Ordering::Relaxed) {
        let wake_grace_active = wake_grace_until
            .map(|deadline| Instant::now() < deadline)
            .unwrap_or(false);

        for target in &mut state.targets {
            let poll = poll_target_session(target.name);
            let caffeinate_alive = target
                .caffeinate_child
                .as_mut()
                .map(child_alive)
                .unwrap_or(false);
            if !caffeinate_alive && target.caffeinate_child.is_some() {
                target.caffeinate_child = None;
            }

            if poll.active {
                if !caffeinate_alive {
                    match Command::new("caffeinate")
                        .arg(format!("-{}", state.caffeinate_flags))
                        .arg("-w")
                        .arg(process::id().to_string())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        Ok(child) => {
                            target.caffeinate_child = Some(child);
                            println!(
                                "[awake] {} session active — caffeinate -{} started ({})",
                                target.name, state.caffeinate_flags, poll.detail
                            );
                        }
                        Err(err) => {
                            eprintln!(
                                "[awake] Failed to start caffeinate for {}: {}",
                                target.name, err
                            );
                        }
                    }
                }
            } else if let Some(mut child) = target.caffeinate_child.take() {
                if wake_grace_active {
                    target.caffeinate_child = Some(child);
                    println!(
                        "[awake] {} wake grace active — keeping caffeinate ({})",
                        target.name, poll.detail
                    );
                } else {
                    terminate_child(&mut child);
                    println!(
                        "[awake] {} idle — caffeinate released ({})",
                        target.name, poll.detail
                    );
                }
            }
        }

        let any_active = state.targets.iter().any(|target| {
            target
                .caffeinate_child
                .as_ref()
                .map(|child| process_alive(child.id()))
                .unwrap_or(false)
        });
        sync_pmset_sleep_disabled(&mut state, any_active, config);

        let sleep_started_at = SystemTime::now();
        let monotonic_sleep_started_at = Instant::now();
        if sleep_with_interrupt(&terminate, Duration::from_secs(POLL_INTERVAL_SECS)) {
            break;
        }

        if detected_sleep_wake_gap(sleep_started_at, monotonic_sleep_started_at) {
            wake_grace_until =
                Some(Instant::now() + Duration::from_secs(ACTIVE_SESSION_WINDOW_SECS));
            println!(
                "[awake] Sleep/wake gap detected — preserving active assertions for {}s while sessions resume",
                ACTIVE_SESSION_WINDOW_SECS
            );
        }
    }

    cleanup(&mut state);
}

fn cmd_stop() {
    match daemon_status() {
        Some(DaemonStatus::PidFile(pid)) => {
            kill_process(pid);
            println!("[awake] Stopped (PID {})", pid);
        }
        Some(DaemonStatus::Orphan(pid)) => {
            kill_process(pid);
            println!("[awake] Stopped orphaned daemon (PID {})", pid);
        }
        None => {
            println!("[awake] Not running");
            process::exit(1);
        }
    }
}

fn cmd_status() {
    match daemon_status() {
        Some(DaemonStatus::PidFile(pid)) => {
            println!("[awake] Status: running (PID {})", pid);
        }
        Some(DaemonStatus::Orphan(pid)) => {
            println!("[awake] Status: running (PID {}, no PID file)", pid);
        }
        None => {
            println!("[awake] Status: stopped");
            return;
        }
    }

    for name in TARGETS {
        let poll = poll_target_session(name);
        println!("{}", provider_status_line(name, &poll));
    }

    println!("[awake]   caffeinate: active while a supported runtime has session state present");
    println!(
        "[awake]   pmset SleepDisabled: {}",
        get_sleep_disabled_value().unwrap_or_else(|| "unknown".to_string())
    );
}

fn cmd_install(config: &AppConfig, raw_args: &[String]) {
    let install_flags = match parse_caffeinate_flags(raw_args) {
        ParseFlagsResult::Ok(flags) => flags,
        ParseFlagsResult::Help => process::exit(0),
        ParseFlagsResult::Err => process::exit(1),
    };

    let script_path = match env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("[awake] Failed to locate current executable: {}", err);
            process::exit(1);
        }
    };

    let home = PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string()));
    let plist_dir = home.join("Library/LaunchAgents");
    let plist_file = plist_dir.join("com.awake.agent.plist");
    let log_dir = home.join("Library/Logs");

    if let Err(err) = fs::create_dir_all(&plist_dir) {
        eprintln!("[awake] Failed to create {}: {}", plist_dir.display(), err);
        process::exit(1);
    }
    if let Err(err) = fs::create_dir_all(&log_dir) {
        eprintln!("[awake] Failed to create {}: {}", log_dir.display(), err);
        process::exit(1);
    }

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\"\n  \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>com.awake.agent</string>\n    <key>ProgramArguments</key>\n    <array>\n        <string>{}</string>\n        <string>start</string>\n        <string>-{}</string>\n    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <true/>\n    <key>ProcessType</key>\n    <string>Background</string>\n    <key>StandardOutPath</key>\n    <string>{}</string>\n    <key>StandardErrorPath</key>\n    <string>{}</string>\n    <key>ThrottleInterval</key>\n    <integer>10</integer>\n</dict>\n</plist>\n",
        xml_escape(&script_path.display().to_string()),
        install_flags,
        xml_escape(&log_dir.join("awake.log").display().to_string()),
        xml_escape(&log_dir.join("awake.err").display().to_string())
    );

    if let Err(err) = fs::write(&plist_file, plist) {
        eprintln!("[awake] Failed to write {}: {}", plist_file.display(), err);
        process::exit(1);
    }

    println!("[awake] Plist written to {}", plist_file.display());

    let gui_uid = current_uid();
    if command_success(
        "launchctl",
        &[
            "bootstrap",
            &format!("gui/{}", gui_uid),
            &plist_file.display().to_string(),
        ],
    ) {
        println!("[awake] LaunchAgent loaded via bootstrap");
    } else if command_success("launchctl", &["load", &plist_file.display().to_string()]) {
        println!("[awake] LaunchAgent loaded via load (fallback)");
    } else {
        println!("[awake] Warning: failed to load LaunchAgent (plist still installed)");
    }

    let _ = config;
}

fn cmd_setup(config: &AppConfig, raw_args: &[String]) {
    let install_flags = match parse_caffeinate_flags(raw_args) {
        ParseFlagsResult::Ok(flags) => flags,
        ParseFlagsResult::Help => process::exit(0),
        ParseFlagsResult::Err => process::exit(1),
    };

    install_script_systemwide(config).unwrap_or_else(|err| {
        eprintln!("[awake] {}", err);
        process::exit(1);
    });

    let current_exe = env::current_exe().unwrap_or_else(|err| {
        eprintln!("[awake] Failed to locate current executable: {}", err);
        process::exit(1);
    });

    if current_exe == config.system_install_path {
        cmd_install(config, &[format!("-{}", install_flags)]);
    } else {
        let status = Command::new(&config.system_install_path)
            .arg("install")
            .arg(format!("-{}", install_flags))
            .status();
        match status {
            Ok(result) if result.success() => {}
            Ok(_) => process::exit(1),
            Err(err) => {
                eprintln!("[awake] Failed to run installed binary: {}", err);
                process::exit(1);
            }
        }
    }

    if should_enable_lid_close_prevention() {
        setup_pmset_privilege(config).unwrap_or_else(|err| {
            eprintln!("[awake] {}", err);
            process::exit(1);
        });
    } else {
        println!("[awake] Skipped lid-close sleep prevention setup");
    }
}

fn cmd_uninstall() {
    let home = PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".to_string()));
    let plist_file = home.join("Library/LaunchAgents/com.awake.agent.plist");
    let gui_uid = current_uid();

    if command_success(
        "launchctl",
        &["bootout", &format!("gui/{}/com.awake.agent", gui_uid)],
    ) {
        println!("[awake] LaunchAgent unloaded via bootout");
    } else if command_success("launchctl", &["unload", &plist_file.display().to_string()]) {
        println!("[awake] LaunchAgent unloaded via unload (fallback)");
    } else {
        println!("[awake] Warning: failed to unload LaunchAgent (may not be loaded)");
    }

    if plist_file.exists() {
        if let Err(err) = fs::remove_file(&plist_file) {
            eprintln!("[awake] Failed to remove {}: {}", plist_file.display(), err);
            process::exit(1);
        }
        println!("[awake] Plist removed: {}", plist_file.display());
    } else {
        println!("[awake] Plist not found: {}", plist_file.display());
    }
}

fn parse_caffeinate_flags(args: &[String]) -> ParseFlagsResult {
    let mut flags = String::new();
    let mut has_explicit_flags = false;

    for arg in args {
        match arg.as_str() {
            "--display" => {
                append_caffeinate_flag(&mut flags, 'd');
                has_explicit_flags = true;
            }
            "--idle-system" => {
                append_caffeinate_flag(&mut flags, 'i');
                has_explicit_flags = true;
            }
            "--help" | "-h" => {
                usage(&env::args().next().unwrap_or_else(|| "awake".to_string()));
                return ParseFlagsResult::Help;
            }
            _ if arg.starts_with("--") => {
                eprintln!("[awake] Unknown option: {}", arg);
                usage(&env::args().next().unwrap_or_else(|| "awake".to_string()));
                return ParseFlagsResult::Err;
            }
            _ if arg.starts_with('-') => {
                let short_flags = &arg[1..];
                if short_flags.is_empty() {
                    eprintln!("[awake] Unknown option: {}", arg);
                    usage(&env::args().next().unwrap_or_else(|| "awake".to_string()));
                    return ParseFlagsResult::Err;
                }

                for ch in short_flags.chars() {
                    match ch {
                        'D' | 'd' => {
                            append_caffeinate_flag(&mut flags, 'd');
                            has_explicit_flags = true;
                        }
                        'i' => {
                            append_caffeinate_flag(&mut flags, 'i');
                            has_explicit_flags = true;
                        }
                        _ => {
                            eprintln!("[awake] Unknown option: -{}", ch);
                            usage(&env::args().next().unwrap_or_else(|| "awake".to_string()));
                            return ParseFlagsResult::Err;
                        }
                    }
                }
            }
            _ => {
                eprintln!("[awake] Unknown option: {}", arg);
                usage(&env::args().next().unwrap_or_else(|| "awake".to_string()));
                return ParseFlagsResult::Err;
            }
        }
    }

    if !has_explicit_flags {
        flags.push_str(DEFAULT_CAFFEINATE_FLAGS);
    }

    ParseFlagsResult::Ok(flags)
}

fn append_caffeinate_flag(flags: &mut String, flag: char) {
    if !flags.contains(flag) {
        flags.push(flag);
    }
}

fn get_sleep_disabled_value() -> Option<String> {
    let output = command_output("pmset", &["-g"]).ok()?;
    if !output.status.success() {
        return None;
    }

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.contains("SleepDisabled") {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if let Some(value) = tokens.get(1) {
                return Some((*value).to_string());
            }
        }
    }

    None
}

fn sync_pmset_sleep_disabled(state: &mut DaemonState, desired: bool, _config: &AppConfig) {
    if desired && !state.pmset_disabled_by_awake {
        if state.pmset_original_sleep_disabled.is_none() {
            state.pmset_original_sleep_disabled = get_sleep_disabled_value();
        }

        if run_pmset_command(&["-a", "disablesleep", "1"]) {
            state.pmset_disabled_by_awake = true;
            persist_pmset_restore_value(
                state
                    .pmset_original_sleep_disabled
                    .as_deref()
                    .unwrap_or("0"),
            );
            println!("[awake] pmset -a disablesleep enabled");
        } else {
            warn_pmset_once(state);
        }
        return;
    }

    if !desired && state.pmset_disabled_by_awake {
        let restore_value = state
            .pmset_original_sleep_disabled
            .clone()
            .unwrap_or_else(|| "0".to_string());
        if run_pmset_command(&["-a", "disablesleep", &restore_value]) {
            state.pmset_disabled_by_awake = false;
            clear_pmset_restore_value();
            println!("[awake] pmset disablesleep restored to {}", restore_value);
        } else {
            warn_pmset_once(state);
        }
    }
}

fn recover_stale_pmset_state() {
    let Some(restore_value) = read_pmset_restore_value() else {
        return;
    };

    let current_value = get_sleep_disabled_value();
    if current_value.as_deref() == Some("1")
        && run_pmset_command(&["-a", "disablesleep", &restore_value])
    {
        println!(
            "[awake] Restored stale pmset disablesleep state to {} from previous run",
            restore_value
        );
    }

    clear_pmset_restore_value();
}

fn persist_pmset_restore_value(value: &str) {
    if let Err(err) = fs::write(PMSET_STATE_FILE, format!("{}\n", value)) {
        eprintln!(
            "[awake] Warning: failed to persist pmset restore state in {}: {}",
            PMSET_STATE_FILE, err
        );
    }
}

fn read_pmset_restore_value() -> Option<String> {
    fs::read_to_string(PMSET_STATE_FILE)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clear_pmset_restore_value() {
    let _ = fs::remove_file(PMSET_STATE_FILE);
}

fn warn_pmset_once(state: &mut DaemonState) {
    if state.pmset_warning_shown {
        return;
    }

    state.pmset_warning_shown = true;
    eprintln!("[awake] Warning: failed to toggle 'pmset -a disablesleep'; run awake as root or allow passwordless sudo for /usr/bin/pmset if you need lid-close sleep prevention");
}

fn cleanup(state: &mut DaemonState) {
    if state.cleanup_done {
        return;
    }

    state.cleanup_done = true;
    println!("[awake] Shutting down...");
    for target in &mut state.targets {
        if let Some(mut child) = target.caffeinate_child.take() {
            terminate_child(&mut child);
        }
    }
    sync_pmset_sleep_disabled(
        state,
        false,
        &AppConfig {
            system_install_path: PathBuf::new(),
            pmset_sudoers_path: PathBuf::new(),
        },
    );
    remove_pid_file_for_process(process::id());
}

fn read_pid_file(path: &Path) -> Option<u32> {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
}

fn daemon_status() -> Option<DaemonStatus> {
    let pid_file = Path::new(PID_FILE);
    if pid_file.exists() {
        match read_pid_file(pid_file) {
            Some(pid) if process_alive(pid) => return Some(DaemonStatus::PidFile(pid)),
            Some(_) | None => {
                let _ = fs::remove_file(pid_file);
            }
        }
    }

    find_running_awake_daemon(Some(process::id())).map(DaemonStatus::Orphan)
}

fn remove_pid_file_for_process(expected_pid: u32) {
    let path = Path::new(PID_FILE);
    match read_pid_file(path) {
        Some(pid) if pid == expected_pid => {
            let _ = fs::remove_file(path);
        }
        _ => {}
    }
}

fn find_running_awake_daemon(exclude_pid: Option<u32>) -> Option<u32> {
    let output = command_output("ps", &["axww", "-o", "pid=,command="]).ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_process_line)
        .find(|(pid, command)| {
            Some(*pid) != exclude_pid && detect_awake_start_command(command) && process_alive(*pid)
        })
        .map(|(pid, _)| pid)
}

fn parse_process_line(line: &str) -> Option<(u32, &str)> {
    let trimmed = line.trim();
    let split_at = trimmed.find(char::is_whitespace)?;
    let (pid, command) = trimmed.split_at(split_at);
    Some((pid.parse().ok()?, command.trim_start()))
}

fn detect_awake_start_command(command: &str) -> bool {
    let mut tokens = command.split_whitespace();
    let executable = tokens.next().unwrap_or("");
    let executable = executable.rsplit('/').next().unwrap_or(executable);
    executable == "awake" && matches!(tokens.next(), Some("start"))
}

fn install_script_systemwide(config: &AppConfig) -> Result<(), String> {
    let source_path = env::current_exe()
        .map_err(|err| format!("Failed to locate current executable: {}", err))?;
    let target_path = &config.system_install_path;

    if source_path == *target_path {
        println!(
            "[awake] Script already installed at {}",
            target_path.display()
        );
        return Ok(());
    }

    let target_dir = target_path
        .parent()
        .ok_or_else(|| format!("Invalid install path: {}", target_path.display()))?;

    if !run_setup_privileged_command("mkdir", &["-p", &target_dir.display().to_string()]) {
        return Err(format!(
            "Failed to install script to {}",
            target_path.display()
        ));
    }

    if run_setup_privileged_command(
        "install",
        &[
            "-m",
            "755",
            &source_path.display().to_string(),
            &target_path.display().to_string(),
        ],
    ) {
        println!("[awake] Installed script to {}", target_path.display());
        Ok(())
    } else {
        Err(format!(
            "Failed to install script to {}",
            target_path.display()
        ))
    }
}

fn setup_pmset_privilege(config: &AppConfig) -> Result<(), String> {
    let setup_user = env::var("SUDO_USER")
        .or_else(|_| env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string());
    let sudoers_dir = config
        .pmset_sudoers_path
        .parent()
        .ok_or_else(|| {
            format!(
                "Invalid sudoers path: {}",
                config.pmset_sudoers_path.display()
            )
        })?
        .to_path_buf();
    let temp_file = env::temp_dir().join(format!("awake-pmset-{}.tmp", process::id()));

    fs::write(
        &temp_file,
        format!("{} ALL=(root) NOPASSWD: /usr/bin/pmset\n", setup_user),
    )
    .map_err(|err| format!("Failed to prepare sudoers file: {}", err))?;

    if !run_setup_privileged_command("mkdir", &["-p", &sudoers_dir.display().to_string()]) {
        let _ = fs::remove_file(&temp_file);
        return Err("Failed to install pmset sudoers rule".to_string());
    }

    let installed = run_setup_privileged_command(
        "install",
        &[
            "-m",
            "440",
            &temp_file.display().to_string(),
            &config.pmset_sudoers_path.display().to_string(),
        ],
    );
    let _ = fs::remove_file(&temp_file);

    if installed {
        println!(
            "[awake] Installed pmset sudoers rule for {} at {}",
            setup_user,
            config.pmset_sudoers_path.display()
        );
        Ok(())
    } else {
        Err("Failed to install pmset sudoers rule".to_string())
    }
}

fn should_enable_lid_close_prevention() -> bool {
    if !io::stdin().is_terminal() {
        println!(
            "[awake] Non-interactive input detected — skipping lid-close sleep prevention setup"
        );
        return false;
    }

    print!("[awake] Prevent sleep when the lid is closed while awake is active? (y/N): ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }

    matches!(answer.trim(), "y" | "Y" | "yes" | "YES")
}

fn run_pmset_command(args: &[&str]) -> bool {
    if is_root() {
        command_success("pmset", args)
    } else {
        let mut sudo_args = vec!["-n", "pmset"];
        sudo_args.extend_from_slice(args);
        command_success("sudo", &sudo_args)
    }
}

fn run_setup_privileged_command(program: &str, args: &[&str]) -> bool {
    if is_root() {
        command_success(program, args)
    } else {
        command_success("sudo", &prepend_command(program, args))
    }
}

fn prepend_command<'a>(program: &'a str, args: &'a [&'a str]) -> Vec<&'a str> {
    let mut combined = Vec::with_capacity(args.len() + 1);
    combined.push(program);
    combined.extend_from_slice(args);
    combined
}

fn is_root() -> bool {
    command_output("id", &["-u"])
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim() == "0")
        .unwrap_or(false)
}

fn current_uid() -> String {
    command_output("id", &["-u"])
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "0".to_string())
}

fn sleep_with_interrupt(terminate: &AtomicBool, total: Duration) -> bool {
    let slice = Duration::from_millis(100);
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        if terminate.load(Ordering::Relaxed) {
            return true;
        }
        let step = std::cmp::min(slice, total - elapsed);
        thread::sleep(step);
        elapsed += step;
    }
    terminate.load(Ordering::Relaxed)
}

fn detected_sleep_wake_gap(
    sleep_started_at: SystemTime,
    monotonic_sleep_started_at: Instant,
) -> bool {
    let wall_elapsed = match SystemTime::now().duration_since(sleep_started_at) {
        Ok(duration) => duration,
        Err(_) => return false,
    };
    let monotonic_elapsed = monotonic_sleep_started_at.elapsed();
    wall_elapsed > monotonic_elapsed + Duration::from_secs(WAKE_GRACE_TOLERANCE_SECS)
}

fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn child_alive(child: &mut Child) -> bool {
    match child.try_wait() {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(_) => process_alive(child.id()),
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.try_wait();
    let _ = child.kill();
    let _ = child.wait();
}

fn kill_process(pid: u32) {
    let _ = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn command_success(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn command_output(program: &str, args: &[&str]) -> io::Result<std::process::Output> {
    Command::new(program).args(args).output()
}

fn provider_status_line(name: &str, poll: &session_polling::SessionPollResult) -> String {
    if poll.active {
        match poll.worked_for_secs.or(poll.last_activity_age_secs) {
            Some(secs) => format!("[{}] RUNNING (worked for {})", name, human_duration(secs)),
            None => format!("[{}] RUNNING", name),
        }
    } else {
        match poll.last_activity_age_secs {
            Some(secs) => format!("[{}] IDLE ({} ago)", name, human_duration(secs)),
            None => format!("[{}] IDLE", name),
        }
    }
}

fn human_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{} sec", secs);
    }

    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{} min", minutes);
    }

    let hours = minutes / 60;
    let remaining_minutes = minutes % 60;
    if hours < 24 {
        if remaining_minutes == 0 {
            return format!("{} hr", hours);
        }
        return format!("{} hr {} min", hours, remaining_minutes);
    }

    let days = hours / 24;
    let remaining_hours = hours % 24;
    if remaining_hours == 0 {
        format!("{} day", days)
    } else {
        format!("{} day {} hr", days, remaining_hours)
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_flags(args: &[&str]) -> ParseFlagsResult {
        let owned = args
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        parse_caffeinate_flags(&owned)
    }

    #[test]
    fn parse_caffeinate_flags_defaults_to_di() {
        match parse_flags(&[]) {
            ParseFlagsResult::Ok(flags) => assert_eq!(flags, "di"),
            _ => panic!("expected ok result"),
        }
    }

    #[test]
    fn parse_caffeinate_flags_deduplicates_short_and_long_flags() {
        match parse_flags(&["-D", "--display", "-i", "-d"]) {
            ParseFlagsResult::Ok(flags) => assert_eq!(flags, "di"),
            _ => panic!("expected ok result"),
        }
    }

    #[test]
    fn parse_caffeinate_flags_supports_help() {
        assert!(matches!(parse_flags(&["--help"]), ParseFlagsResult::Help));
    }

    #[test]
    fn parse_caffeinate_flags_rejects_unknown_short_flag() {
        assert!(matches!(parse_flags(&["-x"]), ParseFlagsResult::Err));
    }

    #[test]
    fn parse_caffeinate_flags_rejects_unknown_long_flag() {
        assert!(matches!(
            parse_flags(&["--unknown-flag"]),
            ParseFlagsResult::Err
        ));
    }

    #[test]
    fn target_set_matches_session_polling_contract() {
        assert_eq!(TARGETS, ["claude-code", "codex", "opencode"]);
    }

    #[test]
    fn poll_interval_matches_existing_contract() {
        assert_eq!(POLL_INTERVAL_SECS, 5);
    }

    #[test]
    fn active_window_uses_three_polls() {
        assert_eq!(ACTIVE_SESSION_WINDOW_SECS, 15);
    }

    #[test]
    fn remove_pid_file_for_process_only_removes_matching_pid() {
        let path = Path::new(PID_FILE);
        let original_contents = fs::read_to_string(path).ok();

        fs::write(path, "111\n").unwrap();
        remove_pid_file_for_process(222);
        assert_eq!(read_pid_file(path), Some(111));

        remove_pid_file_for_process(111);
        assert!(!path.exists());

        match original_contents {
            Some(contents) => {
                let _ = fs::write(path, contents);
            }
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }

    #[test]
    fn detected_sleep_wake_gap_requires_wall_clock_jump() {
        let wall_elapsed = Duration::from_secs(120);
        let monotonic_elapsed = Duration::from_secs(POLL_INTERVAL_SECS);
        assert!(wall_elapsed > monotonic_elapsed + Duration::from_secs(WAKE_GRACE_TOLERANCE_SECS));
    }

    #[test]
    fn detected_sleep_wake_gap_ignores_normal_poll_jitter() {
        let wall_elapsed = Duration::from_secs(POLL_INTERVAL_SECS + 1);
        let monotonic_elapsed = Duration::from_secs(POLL_INTERVAL_SECS);
        assert!(
            !(wall_elapsed > monotonic_elapsed + Duration::from_secs(WAKE_GRACE_TOLERANCE_SECS))
        );
    }

    #[test]
    fn detected_sleep_wake_gap_returns_true_after_large_resume_gap() {
        let wall_elapsed = Duration::from_secs(POLL_INTERVAL_SECS + WAKE_GRACE_TOLERANCE_SECS + 5);
        let monotonic_elapsed = Duration::from_secs(POLL_INTERVAL_SECS);

        assert!(detected_sleep_wake_gap(
            SystemTime::now() - wall_elapsed,
            Instant::now() - monotonic_elapsed,
        ));
    }

    #[test]
    fn detected_sleep_wake_gap_returns_false_for_normal_poll_delay() {
        let wall_elapsed = Duration::from_secs(POLL_INTERVAL_SECS + 1);
        let monotonic_elapsed = Duration::from_secs(POLL_INTERVAL_SECS);

        assert!(!detected_sleep_wake_gap(
            SystemTime::now() - wall_elapsed,
            Instant::now() - monotonic_elapsed,
        ));
    }

    #[test]
    fn parse_process_line_reads_pid_and_command() {
        assert_eq!(
            parse_process_line(" 123 /usr/local/bin/awake start -di"),
            Some((123, "/usr/local/bin/awake start -di"))
        );
    }

    #[test]
    fn detect_awake_start_command_matches_absolute_path() {
        assert!(detect_awake_start_command(
            "/Users/hiddenest/.local/bin/awake start -di"
        ));
    }

    #[test]
    fn detect_awake_start_command_rejects_other_subcommands() {
        assert!(!detect_awake_start_command("awake status"));
    }
}
