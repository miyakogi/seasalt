#!/usr/bin/env bash
# seasalt 統合スニペットの smoke test(ble.sh なしで実行可能)
set -euo pipefail

BIN="${1:?usage: smoke.sh <path-to-seasalt-binary>}"
export SEASALT_BIN="$BIN"
export SEASALT_DATA_DIR="$(mktemp -d)"

# ble.sh のふりをする関数。登録内容を記録するだけ
BASHER_HOOKS=()
blehook() { BASHER_HOOKS+=("$*"); }
# 本物の ble.sh は _ble_complete_auto_source を常に定義しているので、ハーネスも再現する
# (未定義のまま set -u 下で ${#arr[@]} を参照すると unbound variable で失敗するため)
_ble_complete_auto_source=(history syntax)

eval "$("$BIN" init bash)"

# フック登録確認
[[ ${#BASHER_HOOKS[@]} -eq 2 ]] || { echo "FAIL: hooks not registered: ${BASHER_HOOKS[*]}"; exit 1; }

# preexec → record
_seasalt_preexec "echo hello world"
[[ $_seasalt_last_id =~ ^[0-9]+$ ]] || { echo "FAIL: last id not numeric: $_seasalt_last_id"; exit 1; }

# precmd → exit code update
_seasalt_precmd

# 履歴確認
OUT=$("$BIN" search --tsv hello)
[[ $OUT == *"$PWD"*"echo hello world"*"0"* ]] || { echo "FAIL: history missing: $OUT"; exit 1; }

# suggest 確認
SUGG=$("$BIN" suggest --cwd "$PWD" -- "echo")
[[ $SUGG == "echo hello world" ]] || { echo "FAIL: suggest mismatch: $SUGG"; exit 1; }

# auto-complete ソースが定義されていること
declare -F ble/complete/auto-complete/source:seasalt >/dev/null || { echo "FAIL: source fn missing"; exit 1; }

echo "OK"
