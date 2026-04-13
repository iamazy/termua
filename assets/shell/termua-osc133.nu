# Termua OSC 133 shell integration (nushell).
#
# This file is written as the temporary `config.nu`. It first sources the user's
# real Nushell config, then appends Termua's OSC 133 hooks.

const __termua_orig_config = __TERMUA_ORIG_CONFIG__
source $__termua_orig_config

let __termua_pre_prompt = ($env.config | get -o hooks.pre_prompt | default [])
let __termua_pre_execution = ($env.config | get -o hooks.pre_execution | default [])
$env.config = (
    $env.config
    | upsert hooks.pre_prompt (
        $__termua_pre_prompt | append {||
            if ($env.TERMUA_OSC133_COMMAND_ACTIVE? | default false) {
                let __termua_exit = ($env.LAST_EXIT_CODE? | default 0)
                print -n $"\u{1b}]133;D;($__termua_exit)\u{7}"
                load-env { TERMUA_OSC133_COMMAND_ACTIVE: false }
            }
            print -n "\u{1b}]133;A\u{7}"
        }
    )
    | upsert hooks.pre_execution (
        $__termua_pre_execution | append {||
            print -n "\u{1b}]133;B\u{7}"
            print -n "\u{1b}]133;C\u{7}"
            load-env { TERMUA_OSC133_COMMAND_ACTIVE: true }
        }
    )
)
