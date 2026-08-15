_seasalt_bin="${SEASALT_BIN:-$(command -v seasalt)}";
if [[ "$_seasalt_bin" ]]; then
  _seasalt_session="$$-$RANDOM-$RANDOM";
  _seasalt_last_id=;

  # Record the command about to run and keep its row id until
  # _seasalt_precmd stores the exit code.
  function _seasalt_preexec {
    local cmd;
    cmd=${1:-};
    [[ "$cmd" ]] || return 0;
    # Skip commands starting with whitespace (HISTCONTROL=ignorespace).
    [[ $cmd == [[:space:]]* ]] && return 0;
    # Skip recording while private mode is enabled (like $fish_private_mode).
    [[ "${SEASALT_PRIVATE_MODE:-}" ]] && return 0;
    _seasalt_last_id=$("$_seasalt_bin" record --cwd "$PWD" --session "$_seasalt_session" -- "$cmd" 2>/dev/null);
    case "$_seasalt_last_id" in
      (*[!0-9]*) _seasalt_last_id= ;;
    esac;
  };

  # Store the exit code of the command recorded by _seasalt_preexec.
  function _seasalt_precmd {
    local code;
    code=$?;
    if [[ "$_seasalt_last_id" ]]; then
      "$_seasalt_bin" exit --last-id "$_seasalt_last_id" --code "$code" >/dev/null 2>&1;
      _seasalt_last_id=;
    fi;
  };

  if declare -F blehook >/dev/null 2>&1; then
    if [[ -z "${_seasalt_hooked:-}" ]]; then
      blehook PREEXEC+=_seasalt_preexec;
      blehook PRECMD+=_seasalt_precmd;
      _seasalt_hooked=1;
    fi;
    # ble.sh lazily loads core-complete and resets _ble_complete_auto_source
    # to (history syntax); other integrations (e.g. atuin) register later.
    # Reorder the array on the first idle so seasalt is the only inline
    # suggestion source.
    ble/util/idle.push '
      [[ ${_ble_complete_auto_source+_} ]] || _ble_complete_auto_source=(history syntax);
      local _seasalt_source _seasalt_sources=();
      for _seasalt_source in "${_ble_complete_auto_source[@]}"; do
        case "$_seasalt_source" in
          seasalt|atuin-history|history) ;;
          *) _seasalt_sources+=("$_seasalt_source") ;;
        esac;
      done;
      _ble_complete_auto_source=(seasalt "${_seasalt_sources[@]}");';

    # Synchronous suggestion source. `seasalt suggest` is called with a
    # timeout so a slow database never freezes the UI; on timeout the
    # suggestion is simply skipped.
    function ble/complete/auto-complete/source:seasalt {
      local _seasalt_empty='' cmd suggest;
      [[ "${_ble_edit_str:-}" ]] || return 1;
      ((_ble_edit_ind == ${#_ble_edit_str})) || return 1;
      cmd=$(timeout 0.2 "$_seasalt_bin" suggest --cwd "$PWD" -- "$_ble_edit_str" 2>/dev/null) || return 1;
      [[ "$cmd" ]] || return 1;
      [[ "$cmd" == "$_ble_edit_str" ]] && return 1;
      suggest=${cmd:${#_ble_edit_str}};
      ble/complete/auto-complete/enter h 0 "$suggest" "$_seasalt_empty" "$cmd";
    };
  elif declare -p preexec_functions >/dev/null 2>&1 && declare -p precmd_functions >/dev/null 2>&1; then
    # bash-preexec fallback: recording only, no suggestions.
    if [[ -z "${_seasalt_hooked:-}" ]]; then
      case " ${preexec_functions[*]} " in
        (*" _seasalt_preexec "*) ;;
        (*) preexec_functions+=(_seasalt_preexec) ;;
      esac;
      case " ${precmd_functions[*]} " in
        (*" _seasalt_precmd "*) ;;
        (*) precmd_functions+=(_seasalt_precmd) ;;
      esac;
      _seasalt_hooked=1;
    fi;
  else
    printf '%s\n' 'seasalt: ble.sh or bash-preexec not found; hooks not registered. Source ble.sh (or bash-preexec.sh) before eval "$(seasalt init bash)".' >&2;
  fi;
else
  printf '%s\n' 'seasalt: command not found on PATH; set SEASALT_BIN to the full path before eval.' >&2;
fi;
