# Changelog

All notable changes to this project are documented in this file.

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
