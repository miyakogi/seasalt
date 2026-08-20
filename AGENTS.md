# AGENTS.md

seasalt: fish-style inline autosuggestion and per-directory history for bash.
Single Rust binary + a bash snippet compiled into it. SQLite (WAL) at
~/.local/share/seasalt/history.sqlite3 (SEASALT_DATA_DIR overrides).

## Commands

- Pre-commit gate, in this order:
  `cargo fmt && cargo check && cargo clippy && cargo test`
- If `flake.nix` changed, verify it parses first:
  `nix-instantiate --parse flake.nix` (fast, no fetch) and, when available,
  confirm with `nix flake show`.
- Shell integration test — NOT run by `cargo test`, always run it too:
  `cargo build --release && bash tests/bash/smoke.sh target/release/seasalt`
- Performance benchmarks — NOT run by `cargo test`: `cargo bench`
  (benches/suggest.rs; suggests per-keystroke cost in-process and
  end-to-end, incl. process spawn, across DB sizes 1k..1M rows)
- Single test: `cargo test --test <db_test|cli_test|suggest_test|paths_test|bash_test> <name>`
- Install: `cargo install --path .` (Nix: `nix build .#default`)
- CI (GitHub Actions): runs the pre-commit gate + smoke + `nix build`
  on push/PR to main (see `.github/workflows/ci.yml`)
- Dev shell (pinned Rust toolchain incl. rust-analyzer): `nix develop`
  — the repo ships a `.envrc` (`use flake`) for direnv users
- The snippet lives at src/bash/seasalt.bash and is embedded via
  include_str! (src/integration.rs): after editing it, rebuild before
  smoke tests or installs — `cargo test` alone may not exercise it.

## Changelog and release

- Keep CHANGELOG.md up to date in the same change as the code: record
  user-visible changes (features, behavior changes, bug fixes,
  performance/dependency changes) in the existing entry format.
  Internal refactors and test-only changes may be omitted.
- When preparing a release (tagging `vX.Y.Z`), update ALL of these in
  the release commit:
  - `Cargo.toml` version (`flake.nix` reads it from there — one place)
  - `CHANGELOG.md` entry heading (`## [X.Y.Z] - YYYY-MM-DD`)
  - `README.md` flake pin example (`github:miyakogi/seasalt/vX.Y.Z`)
  Then, before tagging, verify the flake still parses and builds:
  `nix-instantiate --parse flake.nix` (fast, no fetch) then
  `nix build .#default` (build normalcy; flake.nix reads Cargo.toml's
  version). Then tag `vX.Y.Z`; `.github/workflows/release.yml` builds the
  binary and fills the release notes from the matching CHANGELOG.md section.

## Architecture

- Subcommands: record/exit (hook-facing, silent), suggest, search,
  delete, clear, init bash (init must not touch the DB or data dir — tested)
- suggest scopes: exact cwd → parents (nearest first) → global;
  10 candidates per scope; exact-case preferred, icase fallback (fish parity)
- Same (cwd, cmd) dedups: record_history refreshes the existing row
- paths column (args existing at record time) filters suggestions
  referencing deleted files
- Permissions: new data dir 0700 / new DB 0600 only; existing ones untouched
- Design spec (authoritative, written in Japanese):
  docs/internal/specs/2026-08-15-seasalt-design.md

## Gotchas (hard-earned)

- Keep suggest SYNCHRONOUS: async variants (bgproc / background
  subshells) emit stray `[1] <pid>` job notifications under
  bash 5.3 + ble.sh 0.4. Do not reintroduce backgrounding (spec §4).
- Only `eval "$(seasalt init bash)"` (quoted) is supported. Unquoted
  eval fails with a syntax error BY DESIGN (snippet comments) — do not
  "fix" that.
- The snippet calls `seasalt suggest` directly; the binary enforces
  the 200ms timeout in-process. Do not reintroduce an external
  `timeout` wrapper or backgrounding.
- SQLite `LIKE` ignores COLLATE BINARY: use GLOB for case-sensitive
  prefix matching (db.rs suggest_prefix).
- Silence contract: record/exit/suggest never write to stderr;
  init/search/delete/clear report errors to stderr.
- Language conventions: comments in src/ and tests/ are English,
  README English, design spec Japanese.
- Features follow: brainstorming → spec → plan → subagent-driven
  implementation (docs/internal/).
