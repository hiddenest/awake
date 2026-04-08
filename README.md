# awake

> A bash script that manages `caffeinate` and `pmset -c disablesleep` so macOS stays awake only while AI coding agents are actually working

## Requirements

- macOS
- bash 3.2 or later (compatible with the default macOS `/bin/bash`)
- You need permission to run `pmset -c disablesleep` if you want to prevent lid-close sleep

## Quick start

```bash
# Clone the repository
git clone <repo-url>
cd experiments-claude-code-awake

# Make the script executable
chmod +x awake

# Run the first-time setup flow
./awake setup
```

`awake setup` performs the following steps automatically:

1. Installs `awake` to `/usr/local/bin/awake`
2. Installs and loads the LaunchAgent
3. Asks whether to enable lid-close sleep prevention while `awake` is active
4. If you answer `y`, installs the `pmset` sudoers rule needed for automatic `pmset -c disablesleep` toggling

## Usage

### Manual usage

```bash
# Run the interactive setup flow
awake setup

# Start in the background
awake start &

# Start in display-only mode
awake start -D &

# Check status
awake status

# Stop
awake stop
```

`awake start` and `awake install` accept the same options.
`awake setup` accepts the same flags and passes them through to the installed LaunchAgent.

- `-D`, `-d`, `--display` — keep the display awake while a target is active
- `-i`, `--idle-system` — prevent idle system sleep while a target is active

If no options are provided, the default is `caffeinate -di`.

While real work is in progress, `awake` also tries `pmset -c disablesleep 1` and `pmset -b disablesleep 1` alongside `caffeinate -di`. When the work ends or `awake` exits, it restores the original `SleepDisabled` values for both AC power and battery.
Two privilege models are supported:

- If `awake` itself runs as root, it calls `pmset` directly.
- If it runs as a regular user, it uses `sudo -n pmset ...`.

If non-interactive `sudo` is not available and `awake` is not running as root, the `pmset` toggle is skipped and only a warning is printed.

### Granting `pmset` permission

If you answer `y` during `awake setup`, the script installs this sudoers rule for you automatically.

#### 1. Recommended: allow NOPASSWD for `pmset` only

Add a sudoers rule like this with `visudo`:

```sudoers
your_username ALL=(root) NOPASSWD: /usr/bin/pmset
```

This lets `awake` run as a normal user while still allowing `pmset` sleep-prevention commands to run without a password.

#### 2. Alternative: run `awake` as root

```bash
sudo ./awake start
```

This is simpler operationally, but it gives root privileges to the entire script rather than just `pmset`.

### LaunchAgent (start automatically at login)

```bash
# Install the LaunchAgent (start automatically at login)
awake install

# Install the LaunchAgent in display-only mode
awake install -D

# Remove the LaunchAgent
awake uninstall
```

## Watched processes

`awake` watches the following process names:

- `claude` — Claude Code CLI
- `codex` — OpenAI Codex CLI
- `opencode` — OpenCode CLI
- `opencode-cli` — OpenCode CLI (alias)
- `pi` — Pi Coding Agent CLI

When a process is detected, `awake` checks for activity signals every 5 seconds. Long-lived **server-style processes** such as `codex app-server` and `opencode serve` / `opencode web` / `opencode acp` are treated as active only when their direct child process count increases. Other CLI processes are treated as active when either their direct child process count increases or their CPU time increases by at least 0.01 seconds. If multiple processes share the same name, `awake` considers the target active when **any one of them** shows an activity signal. While a target is active, `awake` keeps the per-target `caffeinate` process alive and also attempts a global `pmset -c disablesleep 1` toggle when at least one target is active.
New PIDs are not treated as active just because they appeared. If no activity signal appears for 3 consecutive polls (15 seconds), the target is treated as **idle** and `caffeinate` is released.
If work resumes, `caffeinate` is activated again automatically.

## Limitations

Because Codex CLI is Node.js-based, its actual process name may appear as `node`. In that case, `pgrep -x codex` may not detect it. You can add `node` to the `TARGETS` array, but that may also match unrelated Node.js processes.

## How it works

1. `awake start` creates a PID file (`/tmp/awake.pid`) and enters the polling loop.
2. Every 5 seconds, it checks the `TARGETS` process names with `pgrep -x`.
3. For each detected process, it measures the direct child process count and `ps -o cputime` values for all matching PIDs.
4. Server-style processes such as `codex app-server` and `opencode serve` / `opencode web` / `opencode acp` are considered **active** only when their direct child count increases.
5. Other CLI processes are considered **active** when their direct child count increases or their CPU time increases by at least 0.01 seconds → `awake` starts `caffeinate` with the selected flags (default: `-di`).
6. If any target is active, `awake` also attempts global `pmset -c disablesleep 1` and `pmset -b disablesleep 1` toggles (either via root execution or the `sudo -n` path).
7. If no activity signal is seen for 3 consecutive polls (15 seconds), the target is treated as **idle** → `caffeinate` is released.
8. When all active targets are gone, `awake` restores `pmset` to the original `SleepDisabled` values for both AC power and battery.
9. If activity resumes, `caffeinate` / `pmset` are activated again.
10. When a target process exits, `caffeinate` is released immediately.
11. On `awake stop` or SIGTERM, `awake` kills all `caffeinate` processes, removes the PID file, and attempts to restore `pmset`.

## Debugging

Use the following commands to check whether `caffeinate` is active:

```bash
# Show currently active power assertions
pmset -g assertions

# Check awake status
awake status

# Inspect caffeinate processes directly
pgrep -a caffeinate

# Check the current SleepDisabled values
pmset -g custom | grep disablesleep
```

If `pmset -g assertions` shows `PreventUserIdleDisplaySleep` or `PreventUserIdleSystemSleep`, `caffeinate` is working as expected.
