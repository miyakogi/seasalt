#!/usr/bin/env bash
# Smoke test for the seasalt integration snippet (runs without ble.sh)
set -euo pipefail

BIN="${1:?usage: smoke.sh <path-to-seasalt-binary>}"
export SEASALT_BIN="$BIN"

fail() { echo "FAIL: $1"; exit 1; }

# Full verification suite; loads the snippet via quoted eval
# (eval "$(...)"). The functions pretending to be ble.sh reproduce the
# state of the real ble.sh: it lazily loads the core-complete module
# after .bashrc, resets _ble_complete_auto_source unconditionally on
# load, and later integrations (e.g. atuin) register via onload. The
# snippet schedules a task through ble/util/idle.push that reorders the
# array once all registrations have completed (seasalt always first).
run_suite() {
  local data_dir
  data_dir=$(mktemp -d)
  export SEASALT_DATA_DIR="$data_dir"

  BASHER_HOOKS=()
  BASHER_IDLE_TASKS=()
  blehook() { BASHER_HOOKS+=("$*"); }
  ble/util/idle.push() { BASHER_IDLE_TASKS+=("$1"); }
  # _ble_complete_auto_source is undefined before core-complete loads; reset hook state per suite
  unset _ble_complete_auto_source _seasalt_hooked

  eval "$("$BIN" init bash)"

  # Hook registration (also verified to not duplicate on re-eval)
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks not registered: ${BASHER_HOOKS[*]}"
  eval "$("$BIN" init bash)"
  [[ ${#BASHER_HOOKS[@]} -eq 2 ]] || fail "hooks duplicated after re-eval: ${BASHER_HOOKS[*]}"
  [[ ${#BASHER_IDLE_TASKS[@]} -eq 2 ]] || fail "idle tasks not queued: ${#BASHER_IDLE_TASKS[@]}"

  # Running the idle task with _ble_complete_auto_source unset initializes it safely
  eval "${BASHER_IDLE_TASKS[0]}"
  [[ " ${_ble_complete_auto_source[*]} " == " seasalt syntax " ]] || fail "unset array not initialized: ${_ble_complete_auto_source[*]}"

  # Run the idle task after core-complete load (unconditional reset) and atuin onload registration
  _ble_complete_auto_source=(atuin-history history syntax)
  eval "${BASHER_IDLE_TASKS[1]}"
  [[ " ${_ble_complete_auto_source[*]} " == " seasalt syntax " ]] || fail "auto source not reduced: ${_ble_complete_auto_source[*]}"

  # The idle task added by a post-startup re-eval (manual eval) is idempotent
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

  # History check
  local out
  out=$("$BIN" search --tsv hello)
  [[ $out == *"$PWD"*"echo hello world"*"0"* ]] || fail "history missing: $out"

  # Re-running the same command refreshes the existing row instead of adding one (dedup)
  _seasalt_preexec "echo hello world"
  _seasalt_precmd
  rows=$("$BIN" search --tsv echo | wc -l)
  [[ $rows -eq 1 ]] || fail "duplicate history rows after re-run: $rows"

  # Suggest check
  local sugg
  sugg=$("$BIN" suggest --cwd "$PWD" -- "echo")
  [[ $sugg == "echo hello world" ]] || fail "suggest mismatch: $sugg"

  # Candidates referencing deleted files are not suggested
  local fdir sugg2 sugg3
  fdir=$(mktemp -d)
  touch "$fdir/a.txt"
  _seasalt_preexec "ls $fdir/a.txt"
  _seasalt_precmd
  sugg2=$("$BIN" suggest --cwd "$PWD" -- "ls")
  [[ $sugg2 == "ls $fdir/a.txt" ]] || fail "suggest with existing file mismatch: $sugg2"
  rm -f "$fdir/a.txt" # temp file for the test; safe to delete
  sugg3=$("$BIN" suggest --cwd "$PWD" -- "ls")
  [[ -z $sugg3 ]] || fail "suggest with deleted file should be empty: $sugg3"
  rmdir "$fdir" 2>/dev/null || true

  # Commands starting with whitespace (space/tab) are not recorded
  _seasalt_preexec " secret-cmd-xyz"
  [[ -z $_seasalt_last_id ]] || fail "leading-space command recorded: $_seasalt_last_id"
  _seasalt_preexec "$(printf '\tsecret-cmd-tab')"
  [[ -z $_seasalt_last_id ]] || fail "leading-tab command recorded: $_seasalt_last_id"
  sugg4=$("$BIN" suggest --cwd "$PWD" -- "secret")
  [[ -z $sugg4 ]] || fail "leading-space command suggested: $sugg4"
  out=$("$BIN" search --all secret)
  [[ -z $out ]] || fail "leading-space command in history: $out"

  # Nothing is recorded while SEASALT_PRIVATE_MODE is set (like $fish_private_mode)
  export SEASALT_PRIVATE_MODE=1
  _seasalt_preexec "echo private-mode-cmd"
  [[ -z $_seasalt_last_id ]] || fail "private-mode command recorded: $_seasalt_last_id"
  out=$("$BIN" search --all private-mode-cmd)
  [[ -z $out ]] || fail "private-mode command in history: $out"
  # Recording resumes after unsetting it
  unset SEASALT_PRIVATE_MODE
  _seasalt_preexec "echo normal-after-private"
  [[ $_seasalt_last_id =~ ^[0-9]+$ ]] || fail "record after unset failed: $_seasalt_last_id"
  _seasalt_precmd

  # The auto-complete source is defined
  declare -F ble/complete/auto-complete/source:seasalt >/dev/null || fail "source fn missing"

  # Suggest runs synchronously: a history match is entered via enter
  BASHER_ENTER=()
  ble/complete/auto-complete/enter() { BASHER_ENTER+=("$1|$3"); }
  _ble_edit_str="echo"
  _ble_edit_ind=4
  ble/complete/auto-complete/source:seasalt
  [[ ${#BASHER_ENTER[@]} -gt 0 ]] || fail "suggestion not entered"
  [[ ${BASHER_ENTER[0]} == "h| normal-after-private" ]] || fail "suggest mismatch: ${BASHER_ENTER[0]}"
  unset _ble_edit_str _ble_edit_ind

  # clear removes everything
  _seasalt_preexec "echo to-be-cleared"
  _seasalt_precmd
  "$BIN" clear
  out=$("$BIN" search --all to-be-cleared)
  [[ -z $out ]] || fail "clear did not remove history: $out"
}

run_suite

# Without ble.sh or bash-preexec, a warning goes to stderr and eval does not fail
nohook_err=$(bash -c 'eval "$("$1" init bash)" 2>&1 >/dev/null; echo "rc=$?"' _ "$BIN")
[[ $nohook_err == *"seasalt:"* ]] || fail "no-hook warning missing: $nohook_err"
[[ $nohook_err == *"rc=0"* ]] || fail "no-hook eval returned non-zero: $nohook_err"

# Unquoted eval (eval $(...)) fails with a syntax error because of the snippet's comments
unq_err=$(bash -c 'eval $("$1" init bash) 2>&1; echo "rc=$?"' _ "$BIN")
[[ $unq_err == *"rc=0"* ]] && fail "unquoted eval should fail: $unq_err"
[[ -n $unq_err ]] || fail "unquoted eval produced no output: $unq_err"

# In a bash-preexec environment, hooks register in preexec_functions/precmd_functions and stderr stays empty
bp_err=$(bash -c 'declare -a preexec_functions precmd_functions; eval "$("$1" init bash)" 2>&1 >/dev/null' _ "$BIN")
[[ -z $bp_err ]] || fail "unexpected stderr in bash-preexec env: $bp_err"
bp_out=$(bash -c 'declare -a preexec_functions precmd_functions; eval "$("$1" init bash)"; printf "%s\n" "${preexec_functions[*]}" "${precmd_functions[*]}"' _ "$BIN")
[[ $bp_out == *"_seasalt_preexec"* ]] || fail "bash-preexec preexec registration missing: $bp_out"
[[ $bp_out == *"_seasalt_precmd"* ]] || fail "bash-preexec precmd registration missing: $bp_out"

# Without SEASALT_BIN, a warning about seasalt missing from PATH is printed
nobin_err=$(env -i PATH=/nonexistent "$(command -v bash)" -c 'unset SEASALT_BIN; eval "$("$1" init bash)" 2>&1 >/dev/null' _ "$BIN")
[[ $nobin_err == *"seasalt:"* ]] || fail "no-binary warning missing: $nobin_err"

echo "OK"
