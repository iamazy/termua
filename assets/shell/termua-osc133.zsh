# Termua OSC 133 shell integration (zsh).
#
# Emits a minimal subset of OSC 133 markers:
# - A: prompt start
# - B: prompt end
# - C: command start
# - D;<exit>: command end
#
# This is intentionally best-effort and designed to be injected only for
# Termua-spawned terminals (not by editing user dotfiles).

[[ -o interactive ]] || return 0

if [[ -n "${TERMUA_OSC133_ZSH_INSTALLED-}" ]]; then
  return 0
fi
TERMUA_OSC133_ZSH_INSTALLED=1
TERMUA_OSC133_COMMAND_ACTIVE=

__termua_osc133_print() {
  # BEL-terminated OSC: ESC ] ... BEL
  printf '\e]133;%s\a' "$1"
}

__termua_osc133_preexec() {
  TERMUA_OSC133_COMMAND_ACTIVE=1
  __termua_osc133_print "B"
  __termua_osc133_print "C"
}

__termua_osc133_precmd() {
  local exit_status=$?
  if [[ -n "${TERMUA_OSC133_COMMAND_ACTIVE-}" ]]; then
    __termua_osc133_print "D;$exit_status"
    TERMUA_OSC133_COMMAND_ACTIVE=
  fi
  __termua_osc133_print "A"
}

autoload -Uz add-zsh-hook
add-zsh-hook preexec __termua_osc133_preexec
add-zsh-hook precmd __termua_osc133_precmd
