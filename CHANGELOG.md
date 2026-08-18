# Changelog

All notable changes to this project are documented in this file.

## [Unreleased]

- Multi-line commands are recorded verbatim and suggested by their first
  line; `seasalt search` escapes embedded newlines/tabs/backslashes in
  its output so every entry stays on a single line
- Commands starting with any whitespace (including a leading newline)
  are never recorded (unified with the shell-side guard)

## [0.1.1] - 2026-08-18

Fixes and robustness improvements on top of 0.1.0.

- Suggestion correctness: the case-insensitive fallback now matches
  typed lines containing `_` or `%` (SQLite LIKE got an explicit
  ESCAPE clause)
- Faster suggestions: the 200ms timeout is enforced in-process — the
  external coreutils `timeout` wrapper is gone, so each keystroke spawns
  one process instead of two and macOS no longer needs coreutils
- Atomic duplicate suppression: re-running a command in the same
  directory refreshes its row via a unique (cwd, cmd) index, removing a
  race between concurrent shells; legacy duplicate rows are collapsed to
  the newest once on first use
- Automatic schema migration (`PRAGMA user_version`) upgrades existing
  databases on first use
- Internal: silence-contract classification via `Command::interactive()`;
  test hardening (search wildcards, mid-query timeout path, benchmarks)

## [0.1.0] - 2026-08-16

First release: fish-style inline autosuggestions and per-directory
history for bash.

- Inline autosuggestions: the most likely completion appears in gray as
  you type — accept with `→`
- Per-directory history: suggestions prefer the current directory, then
  parents, then global history
- Duplicate suppression: re-running a command refreshes it instead of
  adding a copy (fish parity)
- Sensitive-command controls: leading-space commands are never
  recorded; `SEASALT_PRIVATE_MODE` pauses recording
- Stale-file filtering: suggestions referencing deleted files are
  skipped
- History maintenance: `seasalt search` / `delete` / `clear`, plus an
  automatic 100,000-entry limit (`SEASALT_HISTORY_MAX`)
- Exit-code tracking for every command
- Coexists with atuin: seasalt handles suggestions, atuin keeps Ctrl-R
  search & sync
- Nix flake packaging and CI

Requirements: bash + ble.sh for autosuggestions (recording alone works
with bash-preexec). Install via `cargo install --path .` or
`nix build .#default` — see the README.

Intentionally not included: interactive Ctrl-R search (use atuin) and
fish-style Tab completion with per-command descriptions.
