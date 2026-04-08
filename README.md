# awake

`awake` is a macOS CLI that keeps your machine awake **only while supported AI coding CLIs are doing real work**.

Instead of holding a permanent `caffeinate` assertion, `awake` watches known agent processes, checks whether they are actually active, and only then enables:

- `caffeinate` for display and/or idle-sleep prevention
- `pmset -a disablesleep` for lid-close sleep prevention when permissions allow it

When activity stops, `awake` releases the assertion and restores the previous `SleepDisabled` setting.

## What it watches

`awake` watches these exact process names:

- `claude`
- `codex`
- `opencode`
- `opencode-cli`
- `pi`

## How activity detection works

The daemon polls every **5 seconds**.

For normal CLI processes, `awake` treats a process as active when either of these changes between polls:

- the direct child-process count increases
- the process CPU time increases by at least `0.01s`

For server-style processes, `awake` is stricter:

- `codex app-server`
- `opencode serve`
- `opencode web`
- `opencode acp`
- `opencode-cli serve`
- `opencode-cli web`
- `opencode-cli acp`

These activate on **child-process activity only**, not CPU-only changes.

Newly discovered PIDs do not count as active just because they appeared. If no activity is seen for 3 polls in a row, the target is treated as idle and its `caffeinate` process is released.

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
- polls watched processes every 5 seconds
- starts per-target `caffeinate` children when activity is detected
- enables `pmset -a disablesleep 1` while at least one target is active

### `stop`

Stops the running daemon identified by `/tmp/awake.pid`.

- kills the daemon
- releases any active `caffeinate` children
- attempts to restore the original `SleepDisabled` value

### `status`

Shows:

- whether the daemon is running
- which watched process names are currently detected
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

If `status` says a target is detected but `caffeinate` is not active yet, that can be expected:

- newly seen processes do not immediately count as active
- server-style processes need child-process activity
- idle targets are released after 15 seconds without new activity

## Limitations

- `codex` may appear as `node` depending on how it is launched, so `pgrep -x codex` may miss it
- activity detection is heuristic-based, not API-integrated
- `pmset` behavior depends on your permission model and system policy
