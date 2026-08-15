_seasalt_bin="${SEASALT_BIN:-$(command -v seasalt)}";
if [[ "$_seasalt_bin" ]]; then
  _seasalt_session="$$-$RANDOM-$RANDOM";
  _seasalt_last_id=;

  function _seasalt_preexec {
    local cmd;
    cmd=${1:-};
    [[ "$cmd" ]] || return 0;
    _seasalt_last_id=$("$_seasalt_bin" record --cwd "$PWD" --session "$_seasalt_session" -- "$cmd" 2>/dev/null);
    case "$_seasalt_last_id" in
      (*[!0-9]*) _seasalt_last_id= ;;
    esac;
  };

  function _seasalt_precmd {
    local code;
    code=$?;
    if [[ "$_seasalt_last_id" ]]; then
      "$_seasalt_bin" exit --session "$_seasalt_session" --last-id "$_seasalt_last_id" --code "$code" >/dev/null 2>&1;
      _seasalt_last_id=;
    fi;
  };

  if declare -F blehook >/dev/null 2>&1; then
    if [[ -z "${_seasalt_hooked:-}" ]]; then
      blehook PREEXEC+=_seasalt_preexec;
      blehook PRECMD+=_seasalt_precmd;
      _seasalt_hooked=1;
    fi;
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

    function ble/complete/auto-complete/source:seasalt {
      local _seasalt_empty='';
      [[ "${_ble_edit_str:-}" ]] || return 1;
      ((_ble_edit_ind == ${#_ble_edit_str})) || return 1;
      local cmd suggest;
      cmd=$(timeout 0.2 "$_seasalt_bin" suggest --cwd "$PWD" -- "$_ble_edit_str" 2>/dev/null) || return 1;
      [[ "$cmd" ]] || return 1;
      [[ "$cmd" == "$_ble_edit_str" ]] && return 1;
      suggest=${cmd:${#_ble_edit_str}};
      ble/complete/auto-complete/enter h 0 "$suggest" "$_seasalt_empty" "$cmd";
    };
  elif declare -p preexec_functions >/dev/null 2>&1 && declare -p precmd_functions >/dev/null 2>&1; then
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
