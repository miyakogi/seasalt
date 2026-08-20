_seasalt_bin="${SEASALT_BIN:-$(command -v seasalt)}";
if [[ -n "$_seasalt_bin" ]]; then
  _seasalt_session="$$-$RANDOM-$RANDOM";
  _seasalt_last_id=;

  # Record the command about to run. zsh's preexec passes the full command
  # line (newlines included) in $2.
  function _seasalt_preexec {
    emulate -L zsh;
    local cmd=${2:-};
    [[ -n "$cmd" ]] || return 0;
    [[ "$cmd" == [[:space:]]* ]] && return 0;
    [[ -n "${SEASALT_PRIVATE_MODE:-}" ]] && return 0;
    _seasalt_last_id=$("$_seasalt_bin" record --cwd "$PWD" --session "$_seasalt_session" --shell zsh -- "$cmd" 2>/dev/null);
    case "$_seasalt_last_id" in
      (*[!0-9]*) _seasalt_last_id= ;;
    esac;
  };

  # Store the exit code. MUST run first: zsh feeds each precmd hook the
  # previous hook's return status as `?`, so a later hook loses it.
  function _seasalt_precmd {
    emulate -L zsh;
    local code=$?;
    if [[ -n "$_seasalt_last_id" ]]; then
      "$_seasalt_bin" exit --last-id "$_seasalt_last_id" --code "$code" >/dev/null 2>&1;
      _seasalt_last_id=;
    fi;
    # Keep seasalt as the first zsh-autosuggestions strategy so its
    # per-directory suggestions win over tools that later prepend their
    # own strategy to ZSH_AUTOSUGGEST_STRATEGY (e.g. atuin). Removes any
    # duplicate seasalt entry and preserves the remaining strategies
    # (history, atuin, ...) as fallbacks. Idempotent.
    if (( ${#ZSH_AUTOSUGGEST_STRATEGY[@]} )); then
      ZSH_AUTOSUGGEST_STRATEGY=(seasalt ${ZSH_AUTOSUGGEST_STRATEGY:#seasalt});
    fi;
  };

  autoload -Uz add-zsh-hook >/dev/null 2>&1;
  add-zsh-hook preexec _seasalt_preexec >/dev/null 2>&1 || true;
  add-zsh-hook precmd  _seasalt_precmd  >/dev/null 2>&1 || true;
  preexec_functions=(_seasalt_preexec ${preexec_functions:#_seasalt_preexec});
  precmd_functions=(_seasalt_precmd ${precmd_functions:#_seasalt_precmd});

  if (( ! ${+functions[_zsh_autosuggest_fetch_suggestion]} )); then
    print -r -- 'seasalt: zsh-autosuggestions not loaded; source it before eval "$(seasalt init zsh)" for inline suggestions.' >&2;
  fi;

  function _zsh_autosuggest_strategy_seasalt {
    emulate -L zsh;
    typeset -g suggestion;
    suggestion=$("$_seasalt_bin" suggest --cwd "$PWD" -- "$1" 2>/dev/null) || suggestion=;
  };
  ZSH_AUTOSUGGEST_STRATEGY=(seasalt ${ZSH_AUTOSUGGEST_STRATEGY[@]:-history});
else
  print -r -- 'seasalt: command not found on PATH; set SEASALT_BIN to the full path before eval.' >&2;
fi;