#!/usr/bin/env bash
# seasalt 統合スニペットの smoke test(ble.sh なしで実行可能)
set -euo pipefail

BIN="${1:?usage: smoke.sh <path-to-seasalt-binary>}"
export SEASALT_BIN="$BIN"

fail() { echo "FAIL: $1"; exit 1; }

# フル検証スイート。$1 が quoted なら eval "$(...)"、unquoted なら eval $(...) でスニペットを読み込む。
# ble.sh のふりをする関数は、本物の ble.sh の状態を再現するためのハーネス。
# ble.sh は core-complete モジュールを .bashrc 実行後に遅延ロードし、ロード時に
# _ble_complete_auto_source を無条件リセットしたうえで、後から atuin などの
# onload 登録が先頭に挿入される。統合スニペットは ble/util/idle.push で
# 「seasalt を先頭に置き、atuin-history と bash history を除去する」タスクを
# 登録し、全ての登録が完了した idle 時点で再整列する(seasalt が必ず先頭になる)。
run_suite() {
  local data_dir
  data_dir=$(mktemp -d)
  export SEASALT_DATA_DIR="$data_dir"

  BASHER_HOOKS=()
  BASHER_IDLE_TASKS=()
  blehook() { BASHER_HOOKS+=("$*"); }
  ble/util/idle.push() { BASHER_IDLE_TASKS+=("$1"); }
  # core-complete ロード前は _ble_complete_auto_source は未定義。フック登録状態もスイート毎にリセット
  unset _ble_complete_auto_source _seasalt_hooked

  if [[ $1 == quoted ]]; then
    eval "$("$BIN" init bash)"
  else
    eval $("$BIN" init bash)
  fi

  # フック登録確認(再 eval しても重複しないことも確認)
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks not registered: ${BASHER_HOOKS[*]}"
  eval "$("$BIN" init bash)"
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks duplicated after re-eval: ${BASHER_HOOKS[*]}"
  [[ ${#BASHER_IDLE_TASKS[@]} -eq 2 ]] || fail "idle tasks not queued: ${#BASHER_IDLE_TASKS[@]}"

  # _ble_complete_auto_source が未定義のまま idle タスクが走っても安全に初期化される
  eval "${BASHER_IDLE_TASKS[0]}"
  [[ " ${_ble_complete_auto_source[*]} " == " seasalt syntax " ]] || fail "unset array not initialized: ${_ble_complete_auto_source[*]}"

  # core-complete ロード(無条件リセット)と atuin の onload 登録が完了した状態で idle タスクを実行
  _ble_complete_auto_source=(atuin-history history syntax)
  eval "${BASHER_IDLE_TASKS[1]}"
  [[ " ${_ble_complete_auto_source[*]} " == " seasalt syntax " ]] || fail "auto source not reduced: ${_ble_complete_auto_source[*]}"

  # 起動後の再 eval(手動 eval 相当)で追加された idle タスクも冪等であること
  eval "$("$BIN" init bash)"
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks duplicated on reload: ${BASHER_HOOKS[*]}"
  [[ ${#BASHER_IDLE_TASKS[@]} -eq 3 ]] || fail "idle tasks not added on reload: ${#BASHER_IDLE_TASKS[@]}"
  eval "${BASHER_IDLE_TASKS[2]}"
  [[ " ${_ble_complete_auto_source[*]} " == " seasalt syntax " ]] || fail "idle task not idempotent: ${_ble_complete_auto_source[*]}"
  [[ ${#_ble_complete_auto_source[@]} -eq 2 ]] || fail "auto source duplicated on reload: ${_ble_complete_auto_source[*]}"

  # preexec → record
  _seasalt_preexec "echo hello world"
  [[ $_seasalt_last_id =~ ^[0-9]+$ ]] || fail "last id not numeric: $_seasalt_last_id"

  # precmd → exit code update
  _seasalt_precmd

  # 履歴確認
  local out
  out=$("$BIN" search --tsv hello)
  [[ $out == *"$PWD"*"echo hello world"*"0"* ]] || fail "history missing: $out"

  # 同一コマンドの再実行は新規行を作らず既存行を最新化する (dedup)
  _seasalt_preexec "echo hello world"
  _seasalt_precmd
  rows=$("$BIN" search --tsv echo | wc -l)
  [[ $rows -eq 1 ]] || fail "duplicate history rows after re-run: $rows"

  # suggest 確認
  local sugg
  sugg=$("$BIN" suggest --cwd "$PWD" -- "echo")
  [[ $sugg == "echo hello world" ]] || fail "suggest mismatch: $sugg"

  # 削除済みファイルを参照する候補は出ないこと
  local fdir sugg2 sugg3
  fdir=$(mktemp -d)
  touch "$fdir/a.txt"
  _seasalt_preexec "ls $fdir/a.txt"
  _seasalt_precmd
  sugg2=$("$BIN" suggest --cwd "$PWD" -- "ls")
  [[ $sugg2 == "ls $fdir/a.txt" ]] || fail "suggest with existing file mismatch: $sugg2"
  rm -f "$fdir/a.txt" # テスト用の一時ファイルなので削除してよい
  sugg3=$("$BIN" suggest --cwd "$PWD" -- "ls")
  [[ -z $sugg3 ]] || fail "suggest with deleted file should be empty: $sugg3"
  rmdir "$fdir" 2>/dev/null || true

  # 先頭が空白 (スペース/タブ) のコマンドは記録されない
  _seasalt_preexec " secret-cmd-xyz"
  [[ -z $_seasalt_last_id ]] || fail "leading-space command recorded: $_seasalt_last_id"
  _seasalt_preexec "$(printf '\tsecret-cmd-tab')"
  [[ -z $_seasalt_last_id ]] || fail "leading-tab command recorded: $_seasalt_last_id"
  sugg4=$("$BIN" suggest --cwd "$PWD" -- "secret")
  [[ -z $sugg4 ]] || fail "leading-space command suggested: $sugg4"
  out=$("$BIN" search --all secret)
  [[ -z $out ]] || fail "leading-space command in history: $out"

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
