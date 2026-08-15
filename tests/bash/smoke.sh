#!/usr/bin/env bash
# seasalt 統合スニペットの smoke test(ble.sh なしで実行可能)
set -euo pipefail

BIN="${1:?usage: smoke.sh <path-to-seasalt-binary>}"
export SEASALT_BIN="$BIN"

fail() { echo "FAIL: $1"; exit 1; }

# フル検証スイート。$1 が quoted なら eval "$(...)"、unquoted なら eval $(...) でスニペットを読み込む。
# ble.sh のふりをする関数と _ble_complete_auto_source は、本物の ble.sh の状態を再現するためのハーネス。
run_suite() {
  local data_dir
  data_dir=$(mktemp -d)
  export SEASALT_DATA_DIR="$data_dir"

  BASHER_HOOKS=()
  blehook() { BASHER_HOOKS+=("$*"); }
  _ble_complete_auto_source=(history syntax)

  if [[ $1 == quoted ]]; then
    eval "$("$BIN" init bash)"
  else
    eval $("$BIN" init bash)
  fi

  # フック登録確認(再 eval しても重複しないことも確認)
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks not registered: ${BASHER_HOOKS[*]}"
  eval "$("$BIN" init bash)"
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks duplicated after re-eval: ${BASHER_HOOKS[*]}"
  [[ " ${_ble_complete_auto_source[*]} " == *" seasalt "* ]] || fail "auto source missing: ${_ble_complete_auto_source[*]}"
  [[ ${#_ble_complete_auto_source[@]} -eq 3 ]] || fail "auto source duplicated: ${_ble_complete_auto_source[*]}"

  # preexec → record
  _seasalt_preexec "echo hello world"
  [[ $_seasalt_last_id =~ ^[0-9]+$ ]] || fail "last id not numeric: $_seasalt_last_id"

  # precmd → exit code update
  _seasalt_precmd

  # 履歴確認
  local out
  out=$("$BIN" search --tsv hello)
  [[ $out == *"$PWD"*"echo hello world"*"0"* ]] || fail "history missing: $out"

  # suggest 確認
  local sugg
  sugg=$("$BIN" suggest --cwd "$PWD" -- "echo")
  [[ $sugg == "echo hello world" ]] || fail "suggest mismatch: $sugg"

  # auto-complete ソースが定義されていること
  declare -F ble/complete/auto-complete/source:seasalt >/dev/null || fail "source fn missing"
}

run_suite unquoted
run_suite quoted

# ble.sh も bash-preexec も無い環境では stderr に警告が出て、非 0 で終わらないこと
nohook_err=$(bash -c 'eval "$("$1" init bash)" 2>&1 >/dev/null; echo "rc=$?"' _ "$BIN")
[[ $nohook_err == *"seasalt:"* ]] || fail "no-hook warning missing: $nohook_err"
[[ $nohook_err == *"rc=0"* ]] || fail "no-hook eval returned non-zero: $nohook_err"

# bash-preexec 環境では preexec_functions / precmd_functions に登録され、stderr は空であること
bp_err=$(bash -c 'declare -a preexec_functions precmd_functions; eval "$("$1" init bash)" 2>&1 >/dev/null' _ "$BIN")
[[ -z $bp_err ]] || fail "unexpected stderr in bash-preexec env: $bp_err"
bp_out=$(bash -c 'declare -a preexec_functions precmd_functions; eval "$("$1" init bash)"; printf "%s\n" "${preexec_functions[*]}" "${precmd_functions[*]}"' _ "$BIN")
[[ $bp_out == *"_seasalt_preexec"* ]] || fail "bash-preexec preexec registration missing: $bp_out"
[[ $bp_out == *"_seasalt_precmd"* ]] || fail "bash-preexec precmd registration missing: $bp_out"

# SEASALT_BIN が無い環境では、seasalt が PATH に無い場合の警告が出ること
nobin_err=$(env -i PATH=/nonexistent "$(command -v bash)" -c 'unset SEASALT_BIN; eval "$("$1" init bash)" 2>&1 >/dev/null' _ "$BIN")
[[ $nobin_err == *"seasalt:"* ]] || fail "no-binary warning missing: $nobin_err"

echo "OK"
