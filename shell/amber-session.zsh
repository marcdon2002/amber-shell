# Source at the beginning of .zshenv, before personal startup code.
# Noninteractive/script invocations and nested sessions keep their normal startup.
if [[ -o interactive && -z ${ZSH_EXECUTION_STRING+x} && -z ${ZSH_SCRIPT+x} && -t 0 && -t 1 && -t 2 &&
      ${AMBER_SHELL_CAPTURE:-1} != 0 && -z ${AMBER_SHELL_CAPTURE_SOCKET:-} ]]; then
  _amber-shell-start-session() {
    local binary=${AMBER_SHELL_BIN:-${1:-$HOME/.local/bin/amber-shell}}
    local shell=${SHELL:-/bin/zsh}
    local -a flags=(-i)
    [[ -x $binary ]] || return 0
    [[ $ZSH_ARGZERO == /* && -x $ZSH_ARGZERO ]] && shell=$ZSH_ARGZERO
    [[ $shell == */zsh && -x $shell ]] || shell=/bin/zsh
    [[ -o login ]] && flags+=(-l)
    export ZDOTDIR=${ZDOTDIR:-$HOME}
    exec "$binary" session -- "$shell" "${flags[@]}"
  }
  _amber-shell-start-session "$1"
  unfunction _amber-shell-start-session
fi
