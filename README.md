# awake

`awake` is a macOS CLI that keeps your machine awake **only while supported AI coding tools have an actively progressing session**.

Instead of holding a permanent `caffeinate` assertion, `awake` polls provider-specific session state and only then enables:

- `caffeinate` for display and/or idle-sleep prevention
- `pmset -a disablesleep` for lid-close sleep prevention when permissions allow it

When session activity stops, `awake` releases the assertion and restores the previous `SleepDisabled` setting.

## What it watches

`awake` currently tracks these session providers:

- `claude-code`
- `codex`
- `opencode`

Each provider has its own polling strategy, but they all share the same contract: a provider is only considered active when there is a live runtime plus a recently updated session artifact.

## How activity detection works

The daemon polls every **5 seconds**.

`awake` treats a provider as active only when its latest observed session activity is still within a **15 second active window**.

If macOS enters **system sleep** (for example, lid close) and later wakes, `awake` detects the sleep/wake gap and keeps any already-active assertion alive for one recovery window while provider session files/databases resume updating.

### Provider-specific checks

- **Claude Code**
  - checks whether the Claude GUI app is present, or whether a live IDE lock exists in `~/.claude/ide`
  - treats the newest transcript in `~/.claude/transcripts` as an active session while the runtime is present

- **Codex**
  - checks whether the Codex GUI app is present
  - opens the newest `~/.codex/state_*.sqlite` database read-only
  - treats the newest non-archived thread from `threads` as active while the GUI runtime is present

- **OpenCode**
  - checks whether the OpenCode GUI app is present, or whether an `opencode` CLI session is running with a real interactive subcommand such as `run`, `attach`, or `pr`
  - opens `~/.local/share/opencode/opencode.db` read-only
  - treats the newest non-archived activity across `session`, `message`, `part`, and `todo` as active while the runtime is present

If a supported runtime is present and matching session state exists, the target remains active and its `caffeinate` process stays alive.

`awake` also persists the previous `pmset SleepDisabled` value while it owns that setting, so if the daemon is restarted unexpectedly it can restore stale `pmset` state on the next launch.

## Architecture

Provider polling logic is split by service so each integration can evolve independently:

```text
src/
  main.rs
  session_polling/
    mod.rs
    claude_code.rs
    codex.rs
    opencode.rs
```

`main.rs` owns daemon lifecycle, CLI commands, `caffeinate`, and `pmset` handling. `src/session_polling/` owns provider-specific session detection.

## Requirements

- macOS
- Rust and Cargo to build from source
- Permission to run `pmset` if you want lid-close sleep prevention

## Build

```bash
git clone <repo-url>
cd awake
cargo build --release
```

The built binary will be at:

```bash
./target/release/awake
```

## Install with Homebrew

`awake` is also distributed through a custom Homebrew tap:

```bash
brew tap hiddenest/awake
brew install awake
```

Upgrade later with:

```bash
brew update
brew upgrade awake
```

After installation, run:

```bash
awake setup
```

Tap repository:

```text
https://github.com/hiddenest/homebrew-awake
```

## Quick start

For a first-time local install:

```bash
./target/release/awake setup
```

`setup` does the following:

1. installs or reuses `/usr/local/bin/awake`
2. installs and loads the LaunchAgent
3. asks whether to configure lid-close sleep prevention
4. if you answer `y`, installs a sudoers rule for `/usr/bin/pmset`

## Updating an existing local install

If `/usr/local/bin/awake` already exists and you want to replace it with the Rust binary:

```bash
cargo build --release
sudo install -m 755 target/release/awake /usr/local/bin/awake
```

If you already have the LaunchAgent installed, reload it so launchd uses the updated binary:

```bash
launchctl bootout "gui/$(id -u)/com.awake.agent" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.awake.agent.plist" 2>/dev/null || \
  launchctl load "$HOME/Library/LaunchAgents/com.awake.agent.plist"
```

Verify the installed binary:

```bash
file /usr/local/bin/awake
/usr/local/bin/awake --help
```

## Commands

```bash
awake start [options]
awake stop
awake status
awake setup [options]
awake install [options]
awake uninstall
```

### `start`

Starts the daemon in the foreground.

- creates `/tmp/awake.pid`
- polls supported session providers every 5 seconds
- starts per-target `caffeinate` children when an active session is detected
- enables `pmset -a disablesleep 1` while at least one target is active
- preserves existing assertions briefly after a sleep/wake resume so active sessions are not dropped immediately on wake

### `stop`

Stops the running daemon identified by `/tmp/awake.pid`.

- kills the daemon
- releases any active `caffeinate` children
- attempts to restore the original `SleepDisabled` value

### `status`

Shows:

- whether the daemon is running
- the current session state for Claude Code, Codex, and OpenCode
- the current `pmset` `SleepDisabled` value

### `install`

Writes and loads a LaunchAgent plist at:

```bash
~/Library/LaunchAgents/com.awake.agent.plist
```

The LaunchAgent runs:

```bash
awake start -<flags>
```

using the currently running executable path.

### `setup`

Performs the normal “install this for daily use” flow:

- installs the binary to `/usr/local/bin/awake`
- runs the installed binary’s `install` command
- optionally configures `pmset` privilege for lid-close prevention

### `uninstall`

Unloads and removes the LaunchAgent plist.

## Options

These options apply to `start`, `install`, and `setup`:

- `-D`, `-d`, `--display` — keep the display awake (`caffeinate -d`)
- `-i`, `--idle-system` — prevent idle system sleep (`caffeinate -i`)

If you provide no options, the default is:

```bash
caffeinate -di
```

## Typical usage

Run manually in the foreground:

```bash
./target/release/awake start
```

Run manually in the background:

```bash
./target/release/awake start &
```

Display-only mode:

```bash
./target/release/awake start -D
```

Check status:

```bash
./target/release/awake status
```

Stop the daemon:

```bash
./target/release/awake stop
```

Install auto-start at login:

```bash
./target/release/awake install
```

## Sleep prevention and permissions

`awake` uses two layers of sleep prevention:

1. `caffeinate` for active work assertions
2. `pmset -a disablesleep` for system sleep / lid-close behavior

`pmset` is attempted in this order:

- direct call when `awake` is running as root
- `sudo -n pmset ...` when running as a normal user

If passwordless sudo is not available and `awake` is not root, `pmset` is skipped and `awake` prints a warning once.

### Recommended sudoers rule

If you want lid-close sleep prevention without running the whole daemon as root, allow passwordless access to `/usr/bin/pmset` only:

```sudoers
your_username ALL=(root) NOPASSWD: /usr/bin/pmset
```

That is the rule `awake setup` can install for you when you answer `y`.

## LaunchAgent details

The installed LaunchAgent:

- lives at `~/Library/LaunchAgents/com.awake.agent.plist`
- is configured with `RunAtLoad` and `KeepAlive`
- writes logs to:
  - `~/Library/Logs/awake.log`
  - `~/Library/Logs/awake.err`

During normal operation, each `caffeinate` child is started with `-w <awake-pid>` so it exits automatically if the daemon itself dies or is restarted.

## Environment overrides

For advanced setups and testing, these environment variables are supported:

- `AWAKE_INSTALL_PATH` — override the default install path (`/usr/local/bin/awake`)
- `AWAKE_PMSET_SUDOERS_PATH` — override the default sudoers path (`/etc/sudoers.d/awake-pmset`)

## Troubleshooting

Check whether the daemon is running:

```bash
awake status
```

Check active power assertions:

```bash
pmset -g assertions
```

Inspect `caffeinate` processes directly:

```bash
pgrep -a caffeinate
```

Check the current `SleepDisabled` value:

```bash
pmset -g
```

If `status` says a provider is idle and `caffeinate` is not active, that can be expected:

- the GUI/runtime may be present but the latest session update is stale
- archived sessions are ignored
- idle targets are released after 15 seconds without new activity

If the machine was only **locked** (display sleep), `awake` should continue polling normally. The wake-grace logic is specifically for **system sleep / lid-close** gaps where polling is suspended by macOS.

## Limitations

- activity detection depends on provider-local artifacts such as transcript mtimes and SQLite metadata
- session freshness is inferred from local timestamps, not a first-party provider API
- `pmset` behavior depends on your permission model and system policy
