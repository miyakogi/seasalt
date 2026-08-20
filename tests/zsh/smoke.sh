#!/usr/bin/env bash
# zsh integration smoke test. NOT run by `cargo test`; run explicitly:
#   cargo build --release && bash tests/zsh/smoke.sh target/release/seasalt
set -euo pipefail
BIN=${1:?usage: smoke.sh /path/to/seasalt}
command -v zsh >/dev/null || { echo "SKIP: zsh not installed"; exit 0; }

fail() { echo "FAIL: $1"; exit 1; }

DATA_DIR=$(mktemp -d)
trap 'rm -rf "$DATA_DIR"' EXIT
export SEASALT_DATA_DIR="$DATA_DIR"
export SEASALT_BIN="$BIN"

# The snippet is designed for `eval "$(seasalt init zsh)"` in .zshrc. Here we
# source it in a non-interactive zsh and then clear the live hook arrays, so we
# can call `_seasalt_preexec` / `_seasalt_precmd` directly. (zsh fires preexec
# per line of a script file, which would otherwise double-record.) preexec
# reads the full command line from $2.
zshscript=$(mktemp)
cat > "$zshscript" <<'ZSH'
source <("$SEASALT_BIN" init zsh)
preexec_functions=()
precmd_functions=()
_seasalt_preexec "dummy" "echo seasalt-smoke-cmd"
# record() must return a numeric id.
[[ "$_seasalt_last_id" =~ ^[0-9]+$ ]] || { print -u2 "FAIL: record produced no numeric id"; exit 1; }
_seasalt_precmd
ZSH
zsh "$zshscript"
rm -f "$zshscript"

# The recorded command must be tagged with shell=zsh (search --tsv 6th column).
OUT=$("$BIN" search --all --tsv 'seasalt-smoke-cmd')
grep -q $'\tzsh' <<<"$OUT" || fail "recorded row lacks shell=zsh: $OUT"
[[ "$OUT" == *"echo seasalt-smoke-cmd"* ]] || fail "command not found in history: $OUT"

# The suggestion pipeline returns the recorded command for the typed prefix.
SUGG=$("$BIN" suggest --cwd "$PWD" -- "echo")
[[ "$SUGG" == "echo seasalt-smoke-cmd" ]] || fail "suggest mismatch: $SUGG"

echo "OK"