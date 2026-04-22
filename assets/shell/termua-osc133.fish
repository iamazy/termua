# Termua OSC 133 shell integration (fish).
#
# Emits a minimal subset of OSC 133 markers:
# - A: prompt start
# - B: prompt end
# - C: command start
# - D;<exit>: command end

status is-interactive; or return 0

set -g fish_handle_reflow 0

if set -q TERMUA_OSC133_FISH_INSTALLED
  return 0
end

set -g TERMUA_OSC133_FISH_INSTALLED 1

function __termua_osc133_print --argument-names payload
  printf '\e]133;%s\a' "$payload"
end

if functions -q fish_prompt
  functions -c fish_prompt __termua_osc133_orig_fish_prompt
else
  function __termua_osc133_orig_fish_prompt
    printf '> '
  end
end

function fish_prompt
  __termua_osc133_print "A"
  __termua_osc133_orig_fish_prompt
end

function __termua_osc133_preexec --on-event fish_preexec
  set -g TERMUA_OSC133_COMMAND_ACTIVE 1
  __termua_osc133_print "B"
  __termua_osc133_print "C"
end

function __termua_osc133_postexec --on-event fish_postexec
  if set -q TERMUA_OSC133_COMMAND_ACTIVE
    __termua_osc133_print "D;$status"
    set -e TERMUA_OSC133_COMMAND_ACTIVE
  end
end
