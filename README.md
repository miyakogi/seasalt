# seasalt

Fish-style inline autosuggestion and per-directory history for bash and zsh.

`seasalt` suggests the rest of your command while you type
([ble.sh](https://github.com/akinomyoga/ble.sh) displays it as gray
ghost text; `→` accepts it), keeps history scoped to the directory you
are working in, and exposes a small CLI to search and maintain that
history. It is a single Rust binary plus a bash integration snippet, and
it uses its own SQLite store — it does not touch the shell's history
file.

## Features

- **Inline autosuggestions** — while typing, the most likely completion
  of the current line is shown in gray via ble.sh's auto-complete
  mechanism.
- **Per-directory history scoping** — suggestions prefer history from
  the current directory, then parent directories (nearest first), then
  fall back to global history. Within each scope, the latest matching
  command wins, and commands matching the exact case are preferred
  over case-insensitive matches (like fish).
- **Stale-file filtering** — a command that referenced files which no
  longer exist in the current directory is skipped as a suggestion
  (the next candidate is tried instead). Only arguments that were
  existing files when the command was recorded constrain matching, so
  `echo hello` or `git push` are unaffected.
- **Duplicate suppression** — re-running the same command in the
  same directory refreshes its existing entry (moving it to the top)
  instead of adding another copy, like fish.
- **Space-prefix suppression** — commands that start with a space or
  tab are never recorded, like bash's `HISTCONTROL=ignorespace`. Run
  ` password-command` when you do not want the command (or its
  arguments, e.g. a secret) to appear in history or suggestions.
- **Multi-line command support** — multi-line commands (e.g. heredocs,
  backslash continuations) are recorded verbatim and suggested by their
  first line; `seasalt search` escapes embedded newlines, tabs, and
  backslashes so every entry stays on a single line.
- **Exit-code tracking** — every recorded command stores its exit code.
- **History size limit** — history is automatically trimmed to the
  newest 100,000 entries (configurable via `SEASALT_HISTORY_MAX`,
  `0` disables trimming). `seasalt clear` deletes everything and
  reclaims the file space.
- **Search and delete CLI** — `seasalt search` queries history across
  all directories or scoped to one, and `seasalt delete ID...` removes
  entries by id (e.g. a password recorded by accident).

## Requirements

- bash 4+ with [ble.sh](https://github.com/akinomyoga/ble.sh) (0.4.0
  development builds are fine) — required for autosuggestions.
- zsh >= 5.0.8 with the
  [zsh-autosuggestions](https://github.com/zsh-users/zsh-autosuggestions)
  plugin — required for autosuggestions.
- A Rust toolchain to build from source (or Nix).

ble.sh must be sourced in `.bashrc` **before** the seasalt integration
snippet. Recording hooks also work with
[bash-preexec](https://github.com/rcaloras/bash-preexec) as an
alternative to ble.sh, but suggestions require ble.sh.

For zsh, see the dedicated [zsh](#zsh) section below.

## Installation

### Download a release binary

```sh
curl -L https://github.com/miyakogi/seasalt/releases/latest/download/seasalt-x86_64-unknown-linux-musl.tar.gz | tar xz
sudo install -m 755 seasalt /usr/local/bin/seasalt
```

The release asset is a static binary for Linux x86_64 — no Rust
toolchain or dependencies required.

### From source with cargo

```sh
git clone https://github.com/miyakogi/seasalt seasalt
cd seasalt
cargo install --path .
```

### With Nix

```sh
nix build .#default
# result/bin/seasalt is the binary
```

### With Nix flakes (home-manager)

Add seasalt as a flake input and install the binary through
`home.packages`:

```nix
# home-manager flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
    seasalt.url = "github:miyakogi/seasalt";
  };

  # home-manager module
  home.packages = [ inputs.seasalt.packages.${pkgs.system}.default ];
}
```

Pin a release tag for stability: `seasalt.url =
"github:miyakogi/seasalt/v0.1.2";`. After installing, add
`eval "$(seasalt init bash)"` to `~/.bashrc` as usual (see
[Setup](#setup)).

## Setup

Add the following to `~/.bashrc`, **after** the line that sources
ble.sh:

```sh
eval "$(seasalt init bash)"
```

That is all — the snippet registers the preexec/precmd hooks for
recording and the auto-complete source for suggestions, replacing the
bash history and atuin inline-suggestion sources (details in
[Coexistence with atuin](#coexistence-with-atuin)).

If `seasalt` is not on `PATH` (e.g. it lives in a Nix store path), set
`SEASALT_BIN` to the full path and use it to generate the snippet:

```sh
export SEASALT_BIN=/path/to/seasalt
eval "$("$SEASALT_BIN" init bash)"
```

## zsh

Add the following to `~/.zshrc`:

```sh
eval "$(seasalt init zsh)"
```

That is all — the snippet registers the preexec/precmd history hooks
and a [zsh-autosuggestions](https://github.com/zsh-users/zsh-autosuggestions)
strategy that produces the inline suggestions.

**Requirements** — zsh >= 5.0.8 and the `zsh-autosuggestions` plugin.
Source `zsh-autosuggestions` **before** the `seasalt init zsh` line (or
place the `init zsh` line last in `.zshrc`) so that seasalt's
`ZSH_AUTOSUGGEST_STRATEGY` is preserved.

Inline suggestions come from seasalt's custom zsh-autosuggestions
strategy; if zsh-autosuggestions isn't loaded, only history recording
works (a warning is printed).

History is unified across bash and zsh — both shells share a single
database. Each record is tagged with the shell it came from, visible as
the last column of `seasalt search --tsv`.

## Usage

### CLI subcommands

The CLI is mainly driven by the shell integration, but every piece is
usable by hand:

```
seasalt record --cwd DIR --session SESS -- CMD...
    Record a command into history. Prints the row id. Re-running the
    same command in the same directory updates the existing entry.
    Commands starting with a space or tab are not recorded.

seasalt exit --last-id ID --code CODE
    Store the exit code of a recorded command.

seasalt suggest --cwd DIR -- LINE...
    Print the best history match for the given line, or nothing.

seasalt search [--cwd DIR] [--all] [--limit N] [--tsv] PATTERN
    Search history. Default prints one "id<TAB>cmd" line per entry;
    --tsv prints id, cwd, cmd, exit_code, started_at, shell separated by
    tabs. By default search is scoped to the current directory; use
    --all for everything. PATTERN is matched as a substring (SQL LIKE
    semantics), so % and _ act as wildcards. Embedded backslashes,
    newlines, and tabs in commands are escaped as `\\`, `\n`, and `\t`
    so every entry stays on a single line.

seasalt delete ID...
    Delete history entries by id. Multiple ids can be specified at
    once, separated by spaces. Silently ignores ids that do not exist
    and prints nothing on success.

seasalt clear
    Delete all history entries and reclaim the file space (VACUUM).
    Prints nothing on success.

seasalt init bash
    Print the bash integration snippet.
```

Failures are silent: `record`, `exit`, and `suggest` never write to
stderr and exit non-zero instead, because they are called from shell
hooks.

### Suggestion scoping example

With history like this (`/tmp` being an unrelated directory):

```
/proj/sub   cargo build
/proj       cargo check
/tmp        cargo doc
```

running `cargo` in `/proj/sub` suggests `cargo build`; in
`/proj` or `/proj/deep`, `cargo check`; anywhere else, `cargo doc`.

## Development

Enter the development shell (pinned Rust toolchain: cargo, rustc,
rustfmt, clippy, rust-analyzer):

```sh
nix develop
```

With [direnv](https://direnv.net), the repo ships a `.envrc` that
enters the same shell automatically:

```sh
direnv allow
```

## Configuration

The database lives at:

1. `$SEASALT_DATA_DIR/history.sqlite3` if `SEASALT_DATA_DIR` is set
   (also used for testing),
2. otherwise `$XDG_DATA_HOME/seasalt/history.sqlite3`,
3. otherwise `~/.local/share/seasalt/history.sqlite3`.

The file is created automatically on first use. WAL mode is enabled for
concurrent access by multiple shells.

Set `SEASALT_PRIVATE_MODE` to a non-empty value to stop recording
commands (like fish's `$fish_private_mode`); existing history and
suggestions are unaffected. Unset it to resume recording.

Set `SEASALT_HISTORY_MAX` to change the automatic history size limit
(default 100,000 entries; the oldest entries are dropped on record).
`0` disables trimming.

## Coexistence with atuin

`seasalt` keeps an independent store and does not interfere with atuin:

- **atuin** — history search (`Ctrl-R`), sync, stats.
- **seasalt** — inline autosuggestions and per-directory scoping.

For inline suggestions, seasalt is the only source: the integration
snippet removes the `atuin-history` and bash `history` auto-complete
sources from `_ble_complete_auto_source` on the first idle, so inline
suggestions come only from seasalt. atuin's own history search
(`Ctrl-R`) is unaffected.
atuin keeps its own history store and seasalt does not read or copy
from it: commands recorded by atuin do not appear in seasalt's
suggestions, and vice versa.

## Known limitations

- Autosuggestions require ble.sh; without it only recording works (via
  bash-preexec) and a warning is printed on eval.
- Interactive `Ctrl-R` search is intentionally out of scope: `seasalt`
  keeps search CLI-only; atuin covers interactive history search (see
  [Coexistence with atuin](#coexistence-with-atuin)).
- Tab completion is ble.sh's standard completion; a fish-style
  completion database (per-command descriptions) is intentionally out
  of scope.
- No history sync across machines or users.
