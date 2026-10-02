#!/bin/bash

# Removes what install-typesuggest.sh set up: the TypeSuggest service, its
# autostart, the program in ~/.local/bin and the app settings its step 4
# added. Asks before turning Omarchy's Fcitx5 back on and before deleting
# settings and learned phrases.

set -uo pipefail

bold=$'\033[1m'
dim=$'\033[2m'
reset=$'\033[0m'

target="$HOME/.local/bin/typesuggest"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/typesuggest"
state_file="${XDG_STATE_HOME:-$HOME/.local/state}/typesuggest/app-settings"
qt_env_file="${XDG_CONFIG_HOME:-$HOME/.config}/environment.d/90-typesuggest.conf"
ime_flag="--enable-wayland-ime"

run() {
  printf '%s$ %s%s\n' "$dim" "$*" "$reset"
  "$@"
}

confirm() {
  if command -v gum >/dev/null 2>&1; then
    gum confirm "$1" --default=No
  else
    local answer
    read -r -p "$1 [y/N] " answer
    [[ $answer == [yY] || $answer == [yY][eE][sS] ]]
  fi
}

# Removes the flag line the installer added. A file the installer created is
# deleted if nothing else was added to it since.
remove_ime_flag() {
  local change=$1 file=$2 rest
  [[ -f $file ]] || return 0
  rest="$(grep -vxF -- "$ime_flag" "$file")"
  if [[ $change == created && -z ${rest//[[:space:]]/} ]]; then
    run rm -f "$file"
  else
    printf '%s%s: remove %s%s\n' "$dim" "$file" "$ime_flag" "$reset"
    if [[ -n $rest ]]; then printf '%s\n' "$rest" >"$file"; else : >"$file"; fi
  fi
}

printf '%sRemove TypeSuggest%s\n\n' "$bold" "$reset"
confirm "Stop TypeSuggest and remove the program?" || { echo "Nothing was changed."; exit 0; }

run systemctl --user disable --now typesuggest.service
run rm -f "$HOME/.config/systemd/user/typesuggest.service" "$target"

# Undo the installer's step 4, and only what it changed
app_settings_removed=false
if [[ -f $state_file ]]; then
  app_settings_removed=true
  while read -r change file; do
    remove_ime_flag "$change" "$file"
  done <"$state_file"
  run rm -f "$state_file"
  rmdir "$(dirname "$state_file")" 2>/dev/null
fi
if [[ -f $qt_env_file ]]; then
  app_settings_removed=true
  run rm -f "$qt_env_file"
fi

run systemctl --user daemon-reload
# Drop the session's copy of QT_IM_MODULE so the next login uses Omarchy's value again
$app_settings_removed && run systemctl --user unset-environment QT_IM_MODULE

if systemctl --user cat omarchy-fcitx5.service >/dev/null 2>&1 &&
  ! systemctl --user is-enabled --quiet omarchy-fcitx5.service &&
  confirm "Turn Omarchy's Fcitx5 input method back on?"; then
  run systemctl --user enable --now omarchy-fcitx5.service
fi

if [[ -d $config_dir ]] && confirm "Also delete settings and learned phrases ($config_dir)?"; then
  run rm -rf "$config_dir"
fi

printf '\nTypeSuggest removed.'
if $app_settings_removed; then
  printf ' Reopen browsers and Electron apps, and log out and back in\nfor Qt apps, to drop the input method settings.'
fi
printf '\nRemove the bar icon with:\n  omarchy plugin remove io.github.abdulrahmanhr.typesuggest\n'
read -r -p "Press Enter to close. " _ || true
