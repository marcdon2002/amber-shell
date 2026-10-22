#!/bin/sh
# Installs the chosen binary, two shell files and marked zshenv/zshrc blocks.
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
prefix="${HOME}/.local"
zshrc="${ZDOTDIR:-$HOME}/.zshrc"
zshenv="${ZDOTDIR:-$HOME}/.zshenv"
binary="$repo_dir/target/release/amber-shell"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix|--zshrc|--zshenv|--binary)
      [ "$#" -ge 2 ] || { printf '%s\n' 'Missing option value' >&2; exit 2; }
      case "$1" in --prefix) prefix=$2;; --zshrc) zshrc=$2;; --zshenv) zshenv=$2;; --binary) binary=$2;; esac
      shift 2;;
    *) printf '%s\n' "Unknown argument: $1" >&2; exit 2;;
  esac
done
[ -x "$binary" ] || { printf '%s\n' "Build a release binary first: $binary" >&2; exit 1; }
for config_path in "$zshenv" "$zshrc"; do
  [ ! -e "$config_path" ] || [ -f "$config_path" ] || { printf '%s\n' 'Shell config must be a regular file.' >&2; exit 1; }
done
[ ! -L "$zshenv" ] || { printf '%s\n' 'Refusing to rewrite a symlinked zshenv; use --zshenv with its resolved file.' >&2; exit 1; }
[ ! -L "$zshrc" ] || { printf '%s\n' 'Refusing to rewrite a symlinked zshrc; install with --zshrc targeting its resolved file.' >&2; exit 1; }
# Restrict paths used in the quoted shell setup; never construct executable input from them.
case "$prefix" in *"'"*|*'
'*) printf '%s\n' 'Installation prefix must not contain quotes or newlines.' >&2; exit 2;; esac
mkdir -p "$prefix/bin" "$prefix/share/amber-shell" "$(dirname -- "$zshrc")" "$(dirname -- "$zshenv")"
install -m 755 "$binary" "$prefix/bin/amber-shell"
install -m 644 "$repo_dir/shell/amber-shell.zsh" "$prefix/share/amber-shell/amber-shell.zsh"
install -m 644 "$repo_dir/shell/amber-session.zsh" "$prefix/share/amber-shell/amber-session.zsh"
# Prepending makes personal startup files run only in the inner shell, once.
capture_begin='# >>> amber-shell capture >>>'
capture_end='# <<< amber-shell capture <<<'
if [ -f "$zshenv" ] && grep -Fqx "$capture_begin" "$zshenv"; then
  grep -Fqx "$capture_end" "$zshenv" || { printf '%s\n' 'Incomplete capture block; inspect zshenv.' >&2; exit 1; }
  grep -Fqx "source '$prefix/share/amber-shell/amber-session.zsh' '$prefix/bin/amber-shell'" "$zshenv" || { printf '%s\n' 'Existing capture block uses another prefix.' >&2; exit 1; }
else
  if [ -f "$zshenv" ]; then
    cp -p "$zshenv" "$zshenv.amber-backup-$(date +%Y%m%d-%H%M%S)-$$"
  fi
  env_temp=$(mktemp "$zshenv.amber.XXXXXXXX")
  chmod 600 "$env_temp"
  {
    printf '%s\n' "$capture_begin"
    printf "source '%s/share/amber-shell/amber-session.zsh' '%s/bin/amber-shell'\n" "$prefix" "$prefix"
    printf '%s\n' "$capture_end"
    [ ! -f "$zshenv" ] || cat "$zshenv"
  } > "$env_temp"
  mv -f "$env_temp" "$zshenv"
fi
begin='# >>> amber-shell >>>'
end='# <<< amber-shell <<<'
# Avoid updating an existing marked block silently (including a changed prefix).
if [ -f "$zshrc" ] && grep -Fqx "$begin" "$zshrc"; then
  grep -Fqx "$end" "$zshrc" || { printf '%s\n' 'Incomplete amber-shell block; inspect zshrc before retrying.' >&2; exit 1; }
  expected="source '$prefix/share/amber-shell/amber-shell.zsh'"
  grep -Fqx "$expected" "$zshrc" || { printf '%s\n' 'Existing amber-shell block uses another prefix; left unchanged.' >&2; exit 1; }
  printf '%s\n' 'Binary/plugin updated; existing zshrc block preserved.'
  exit 0
fi
if [ -f "$zshrc" ]; then
  backup="$zshrc.amber-backup-$(date +%Y%m%d-%H%M%S)-$$"
  cp -p "$zshrc" "$backup"
  printf 'Backup: %s\n' "$backup"
fi
{
  printf '\n%s\n' "$begin"
  printf "export AMBER_SHELL_BIN='%s/bin/amber-shell'\n" "$prefix"
  printf "source '%s/share/amber-shell/amber-shell.zsh'\n" "$prefix"
  printf '%s\n' "$end"
} >> "$zshrc"
printf '%s\n' 'Installed. Open a new zsh session, then press Ctrl-X followed by Ctrl-A.'
