# Termua OSC 133 environment integration (nushell).
#
# This file is written as the temporary `env.nu`.

const __termua_orig_env = __TERMUA_ORIG_ENV__
source-env $__termua_orig_env

$env.TERMUA_OSC133_COMMAND_ACTIVE = false
