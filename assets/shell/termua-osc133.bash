# Termua OSC 133 shell integration (bash).
#
# Emits a minimal subset of OSC 133 markers:
# - A: prompt start
# - B: prompt end
# - C: command start
# - D;<exit>: command end
#
# This is intentionally best-effort and designed to be injected only for
# Termua-spawned terminals (not by editing user dotfiles).

[[ $- == *i* ]] || return 0

__termua_osc133_print() {
  # BEL-terminated OSC: ESC ] ... BEL
  printf '\e]133;%s\a' "$1"
}

# Set once we're about to show a prompt; the first DEBUG trap after that is
# treated as the user's next command starting.
TERMUA_OSC133_PROMPT_READY=
TERMUA_OSC133_COMMAND_ACTIVE=

__termua_osc133_preexec() {
  if [[ -n "${TERMUA_OSC133_PROMPT_READY-}" ]]; then
    TERMUA_OSC133_PROMPT_READY=
    TERMUA_OSC133_COMMAND_ACTIVE=1
    __termua_osc133_print "B"
    __termua_osc133_print "C"
  fi
}

__termua_osc133_precmd() {
  local status=$?

  if [[ -n "${TERMUA_OSC133_COMMAND_ACTIVE-}" ]]; then
    __termua_osc133_print "D;$status"
    TERMUA_OSC133_COMMAND_ACTIVE=
  fi

  __termua_osc133_print "A"

  if [[ -n "${__termua_osc133_orig_prompt_command-}" ]]; then
    eval "$__termua_osc133_orig_prompt_command"
  fi

  TERMUA_OSC133_PROMPT_READY=1
}

trap '__termua_osc133_preexec' DEBUG

# Preserve existing PROMPT_COMMAND best-effort by running it from within our wrapper.
__termua_osc133_orig_prompt_command="${PROMPT_COMMAND-}"
PROMPT_COMMAND='__termua_osc133_precmd'
