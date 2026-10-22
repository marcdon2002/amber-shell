# amber-shell: source after Oh My Zsh / prompt setup. Never evaluates a proposal.
[[ -o interactive ]] || return 0

_amber-shell-byte-length() {
  emulate -L zsh
  setopt nomultibyte
  REPLY=${#1}
}

_amber-shell-read-result() {
  emulate -L zsh
  setopt multibyte
  local result=$1 candidate extra fd size
  [[ -f $result && ! -L $result ]] || return 1
  size=$(command wc -c < "$result") || return 1
  [[ $size -ge 2 && $size -le 16385 ]] || return 1
  # Validate bytes before zsh interprets UTF-8. iconv does not change the result.
  command iconv -f UTF-8 -t UTF-8 "$result" >/dev/null 2>&1 || return 1
  exec {fd}< "$result" || return 1
  local valid=0
  {
    IFS= read -r -d '' -u $fd candidate || return 1
    # Exactly one terminating NUL and EOF; no second result or partial suffix.
    if IFS= read -r -k 1 -u $fd extra; then return 1; fi
    [[ -n ${candidate//[[:space:]]/} ]] || return 1
    local forbidden=$'[\x01-\x08\x0b-\x1f\x7f\u0080-\u009f]'
    [[ $candidate != *${~forbidden}* ]] || return 1
    REPLY=$candidate
    valid=1
  } always {
    exec {fd}<&-
  }
  (( valid ))
}

amber-shell-widget() {
  zle >/dev/null 2>&1 || { print -u2 -r -- 'amber-shell: Dieses Widget benötigt eine aktive ZLE-Eingabe.'; return 1; }
  emulate -L zsh
  setopt localoptions multibyte
  local old_buffer=$BUFFER old_cursor=$CURSOR
  local binary=${AMBER_SHELL_BIN:-$HOME/.local/bin/amber-shell}
  local temp='' tty_state='' REPLY entry id result_status=1 accepted=0
  # On macOS, polling /dev/tty can miss cursor-report input from a ZLE child.
  # Open the concrete controlling terminal so crossterm can read that response.
  local tty_device=${TTY:-/dev/tty}
  local -i entry_count=0 history_bytes=0
  local -a extra_args=() selected_history=()
  [[ ${AMBER_SHELL_SIMULATE:-0} == 1 ]] && extra_args+=(--simulate)
  [[ ${AMBER_SHELL_CAPTURE:-1} != 0 && -S ${AMBER_SHELL_CAPTURE_SOCKET:-} ]] && extra_args+=(--capture-socket "$AMBER_SHELL_CAPTURE_SOCKET")
  if [[ ! -x $binary ]]; then
    zle -M 'amber-shell: Binary fehlt; Installation oder AMBER_SHELL_BIN prüfen.'
    return 1
  fi
  {
    temp=$(command mktemp -d "${TMPDIR:-/tmp}/amber-shell.XXXXXXXX") || return 1
    command chmod 700 "$temp" || return 1
    : > "$temp/history"
    command chmod 600 "$temp/history"
    zmodload zsh/parameter
    # Choose the latest 20 available entries first; send accepted entries oldest first.
    for id in ${(Onk)history}; do
      (( entry_count++ ))
      (( entry_count > 20 )) && break
      entry=$history[$id]
      _amber-shell-byte-length "$entry"
      (( REPLY + history_bytes + 1 > 8192 )) && continue
      selected_history=("$entry" "${selected_history[@]}")
      (( history_bytes += REPLY + 1 ))
    done
    for entry in "${selected_history[@]}"; do
      print -rn -- "$entry"$'\0' >> "$temp/history"
    done
    tty_state=$(command stty -g <"$tty_device" 2>/dev/null) || return 1
    zle -I
    _amber-shell-capture-event W
    command "$binary" --history-file "$temp/history" --result-file "$temp/result" "${extra_args[@]}" <"$tty_device" >"$tty_device" 2>"$tty_device"
    result_status=$?
    if (( result_status == 0 )) && _amber-shell-read-result "$temp/result"; then
      BUFFER=$REPLY
      CURSOR=${#BUFFER}
      accepted=1
    elif (( result_status != 2 )); then
      zle -M 'amber-shell: Keine gültige Rückgabe; vorherige Eingabe bleibt erhalten.'
    fi
  } always {
    [[ -n $tty_state ]] && command stty "$tty_state" <"$tty_device" 2>/dev/null
    [[ -n $temp ]] && command rm -rf -- "$temp"
    if (( ! accepted )); then
      BUFFER=$old_buffer
      CURSOR=$old_cursor
    fi
    _amber-shell-capture-event P
    zle reset-prompt
  }
  return 0
}

zle -N amber-shell-widget
# Ctrl-X, then Ctrl-A is transmitted by Apple Terminal without an Option/Cmd mapping.
# Set AMBER_SHELL_KEY before sourcing to choose a different ZLE key sequence.
_amber-shell-bind() {
  emulate -L zsh
  local key=${AMBER_SHELL_KEY:-'^X^A'} map binding action
  for map in emacs viins; do
    binding=$(bindkey -M "$map" "$key" 2>/dev/null) || continue
    action=${binding##* }
    if [[ $action == undefined-key || $action == amber-shell-widget ]]; then
      bindkey -M "$map" "$key" amber-shell-widget
    else
      print -u2 -r -- "amber-shell: $map $key ist bereits belegt ($action); Bindung bleibt erhalten."
    fi
  done
}
_amber-shell-bind
unfunction _amber-shell-bind

# Ordered private markers stay in the same PTY stream as the command's output.
# The relay removes them before forwarding to the terminal. No per-command process or socket.
_amber-shell-capture-event() {
  emulate -L zsh
  local part payload='' first=1
  [[ ${AMBER_SHELL_CAPTURE:-1} != 0 && ${AMBER_SHELL_CAPTURE_OWNER:-} == $$ && -n ${AMBER_SHELL_CAPTURE_TOKEN:-} ]] || return 0
  for part in "$@"; do
    part=${part//\%/%25}
    part=${part//$'\n'/%0A}
    part=${part//$'\r'/%0D}
    part=${part//$'\t'/%09}
    part=${part//$'\a'/%07}
    part=${part//$'\e'/%1B}
    (( first )) || payload+=$'\0'
    payload+=$part
    first=0
  done
  print -rn -- $'\e]777;amber;'"$AMBER_SHELL_CAPTURE_TOKEN;${payload}"$'\a' >"${TTY:-/dev/tty}" 2>/dev/null
  return 0
}
_amber-shell-restore-eol() {
  if [[ ${_AMBER_EOL_WRAPPED:-} == 1 && ${PROMPT_EOL_MARK-} == "$_AMBER_EOL_VALUE" ]]; then
    if [[ $_AMBER_EOL_SET == 1 ]]; then
      PROMPT_EOL_MARK=$_AMBER_EOL_OLD
    else
      unset PROMPT_EOL_MARK
    fi
  fi
  _AMBER_EOL_WRAPPED=0
}
_amber-shell-capture-preexec() {
  emulate -L zsh
  local command_text=${1:-$3} REPLY
  _amber-shell-restore-eol
  _amber-shell-byte-length "$command_text"
  (( REPLY > 8192 )) && command_text=''
  # Stop before zsh draws/erases its partial-line prompt marker; preserve output without a final newline.
  if [[ ${(t)PROMPT_EOL_MARK} != *readonly* ]]; then
    typeset -g _AMBER_EOL_SET=${+PROMPT_EOL_MARK}
    typeset -g _AMBER_EOL_OLD=${PROMPT_EOL_MARK-}
    typeset -g _AMBER_EOL_VALUE=$'%{\e]777;amber;'"$AMBER_SHELL_CAPTURE_TOKEN;S"$'\a%}'"${PROMPT_EOL_MARK-%B%S%#%s%b}"
    PROMPT_EOL_MARK=$_AMBER_EOL_VALUE
    typeset -g _AMBER_EOL_WRAPPED=1
  else
    command_text=''
  fi
  _amber-shell-capture-event B "$command_text" "$PWD"
  return 0
}
_amber-shell-capture-precmd() {
  local amber_exit=$?
  emulate -L zsh
  _amber-shell-capture-event S
  _amber-shell-capture-event E "$amber_exit"
  _amber-shell-restore-eol
  return 0
}
if [[ ${AMBER_SHELL_CAPTURE:-1} != 0 && ${AMBER_SHELL_CAPTURE_OWNER:-} == $$ && -S ${AMBER_SHELL_CAPTURE_SOCKET:-} ]]; then
  autoload -Uz add-zsh-hook
  add-zsh-hook -d preexec _amber-shell-capture-preexec
  add-zsh-hook -d precmd _amber-shell-capture-precmd
  preexec_functions=(_amber-shell-capture-preexec "${preexec_functions[@]}")
  precmd_functions=(_amber-shell-capture-precmd "${precmd_functions[@]}")
fi
