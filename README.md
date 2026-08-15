# seasalt

Fish-style inline autosuggestion and per-directory history for bash.

`seasalt` shows a gray suggestion of the rest of your command while you
type (accept it with `→`), keeps history scoped to the directory you
are working in, and exposes a small CLI to search and maintain that
history. It is a single Rust binary plus a bash integration snippet, and
it uses its own SQLite store — it does not touch the shell's history
file.

## Features

- **Inline autosuggestions** — while typing, the most likely completion
  of the current line is shown in gray via ble.sh's auto-complete
  mechanism. Press `→` (or `Ctrl-F`) to accept it.
- **Per-directory history scoping** — suggestions prefer history from
  the current directory, then parent directories (nearest first), then
  fall back to global history. Within each scope, the latest matching
  command wins (case-insensitive prefix match).
- **Stale-file filtering** — a command that referenced files which no
  longer exist in the current directory is skipped as a suggestion
  (the next candidate is tried instead). Only arguments that were
  existing files when the command was recorded constrain matching, so
  `echo hello` or `git push` are unaffected.
- **Exit-code tracking** — every recorded command stores its exit code,
  so suggestions can be refined by success in the future.
- **Search CLI** — `seasalt search` queries history across all
  directories or scoped to one.

## Requirements

- bash 4+ with [ble.sh](https://github.com/ainamota/ble.sh) (0.4.0
  development builds are fine) — required for autosuggestions.
- A Rust toolchain to build from source (or Nix).

ble.sh must be sourced in `.bashrc` **before** the seasalt integration
snippet. Recording hooks also work with bash-preexec as an alternative
to ble.sh, but suggestions require ble.sh.

## Installation

### From source with cargo

```sh
git clone <repo-url> seasalt
cd seasalt
cargo install --path .
```

### With Nix

```sh
nix build .#default
# result/bin/seasalt is the binary
```

## Setup

Add the following to `~/.bashrc`, **after** the line that sources
ble.sh:

```sh
eval "$(seasalt init bash)"
```

That is all — the snippet registers the preexec/precmd hooks for
recording and the auto-complete source for suggestions, replacing the
bash history and atuin inline-suggestion sources (details in
[Coexistence with atuin](#coexistence-with-atuin)). It is safe to
re-eval the snippet (for example when you update the binary); hooks are
not duplicated.

If `seasalt` is not on `PATH` (e.g. it lives in a Nix store path), point
the snippet at the full path:

```sh
export SEASALT_BIN=/path/to/seasalt
```

## Usage

### CLI subcommands

The CLI is mainly driven by the shell integration, but every piece is
usable by hand:

```
seasalt record --cwd DIR --session SESS -- CMD...
    Record a command into history. Prints the row id.

seasalt exit --session SESS --last-id ID --code CODE
    Store the exit code of a recorded command.

seasalt suggest --cwd DIR -- LINE...
    Print the best history match for the given line, or nothing.

seasalt search [--cwd DIR] [--all] [--limit N] [--tsv] PATTERN
    Search history. Default prints one command per line; --tsv prints
    id, cwd, cmd, exit_code, started_at separated by tabs. By default
    search is scoped to the current directory; use --all for everything.

seasalt init bash
    Print the bash integration snippet.
```

Failures are silent: `record`, `exit`, and `suggest` never write to
stderr and exit non-zero instead, because they are called from shell
hooks.

### Suggestion scoping example

With history like this:

```
/proj/sub   cargo build
/proj       cargo check
/global     cargo doc
```

running `cargo` in `/proj/sub` suggests `cargo build`; in
`/proj/deep`, `cargo check`; anywhere else, `cargo doc`.

## Configuration

The database lives at:

1. `$SEASALT_DATA_DIR/history.sqlite3` if `SEASALT_DATA_DIR` is set
   (also used for testing),
2. otherwise `$XDG_DATA_HOME/seasalt/history.sqlite3`,
3. otherwise `~/.local/share/seasalt/history.sqlite3`.

The file is created automatically on first use. WAL mode is enabled for
concurrent access by multiple shells.

## Coexistence with atuin

`seasalt` keeps an independent store and does not interfere with atuin:

- **atuin** — history search (`Ctrl-R`), sync, stats.
- **seasalt** — inline autosuggestions and per-directory scoping.

For inline suggestions, seasalt is the only source: the integration
snippet removes the `atuin-history` and bash `history` auto-complete
sources from `_ble_complete_auto_source` on the first idle, so a command
referencing a deleted file is never suggested by anyone. atuin's own
history search (`Ctrl-R`) is unaffected.
Existing atuin history can be imported later if desired; nothing is
duplicated automatically.

## Known limitations

- Autosuggestions require ble.sh; without it only recording works (via
  bash-preexec) and a warning is printed on eval.
- There is no interactive search UI (`Ctrl-R` replacement) yet; search
  is CLI-only.
- No history sync across machines or users.
