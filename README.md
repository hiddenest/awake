# awake

`awake` is a macOS CLI that keeps your machine awake **only while supported AI coding tools are actively progressing**.

Instead of holding a permanent assertion, it watches local session state and only then enables:

- `caffeinate` for display and/or idle-sleep prevention
- `pmset -a disablesleep` for lid-close sleep prevention when permissions allow it

When activity stops, `awake` releases the assertion and restores the previous `SleepDisabled` value.

## Quick start

Install with Homebrew and set it up to run at login:

```bash
brew install hiddenest/awake/awake
awake setup
```

Check what `awake` currently sees:

```bash
awake status
```

Example output:

```text
[awake] Status: running (PID 12345)
[claude-code] RUNNING (worked for 42 sec)
[codex] IDLE (3 min ago)
[cursor-agent] IDLE
[opencode] IDLE
[awake]   caffeinate: active while a supported runtime has session state present
[awake]   pmset SleepDisabled: 1
```

If the daemon is not running yet:

```text
[awake] Status: stopped
```

Upgrade later with:

```bash
brew upgrade hiddenest/awake/awake
```

## Supported tools

`awake` currently watches:

- `claude-code`
- `codex`
- `cursor-agent`
- `opencode`

A provider is considered active only when there is both a live runtime and fresh session activity.
For Cursor, `awake` watches both the standalone `cursor-agent` CLI and Cursor IDE agent transcripts.

## How it works

- polls every **5 seconds**
- treats activity as fresh for **15 seconds**
- starts or stops `caffeinate` automatically based on live session state
- optionally toggles `pmset -a disablesleep` while work is active
- keeps active assertions briefly across system sleep/wake recovery

## Commands

```bash
awake start [options]
awake stop
awake status
awake setup [options]
awake install [options]
awake uninstall
```

- `start` — run the daemon in the foreground
- `stop` — stop the running daemon
- `status` — show daemon state, provider state, and current `SleepDisabled`
- `setup` — install or reuse `/usr/local/bin/awake`, install the LaunchAgent, and optionally configure `pmset` privilege
- `install` — install only the LaunchAgent
- `uninstall` — remove the LaunchAgent

### Options

These apply to `start`, `install`, and `setup`:

- `-D`, `-d`, `--display` — keep the display awake (`caffeinate -d`)
- `-i`, `--idle-system` — prevent idle system sleep (`caffeinate -i`)

Default behavior:

```bash
caffeinate -di
```

## Build from source

Requirements:

- macOS
- Rust and Cargo

Build and run:

```bash
git clone <repo-url>
cd awake
cargo build --release
./target/release/awake setup
```

## Permissions

`awake` uses:

1. `caffeinate` for active work assertions
2. `pmset -a disablesleep` for lid-close sleep prevention

If `awake` is not running as root, it tries `sudo -n pmset ...`. If passwordless sudo is unavailable, `pmset` is skipped and only `caffeinate` is used.

If you want lid-close sleep prevention without running the daemon as root, allow passwordless access to `/usr/bin/pmset` only:

```sudoers
your_username ALL=(root) NOPASSWD: /usr/bin/pmset
```

`awake setup` can install that rule for you when you answer `y`.

## Troubleshooting

Useful checks:

```bash
awake status
pmset -g assertions
pgrep -a caffeinate
pmset -g
```

If `status` says a provider is idle, that can be expected when:

- the GUI/runtime is open but the latest session update is stale
- archived sessions are ignored
- the active window has expired without new work

## Notes

- LaunchAgent file: `~/Library/LaunchAgents/com.awake.agent.plist`
- Logs: `~/Library/Logs/awake.log` and `~/Library/Logs/awake.err`
- Homebrew tap repository: `https://github.com/hiddenest/homebrew-awake`
