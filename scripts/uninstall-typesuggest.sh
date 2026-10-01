#!/bin/bash

# Removes what install-typesuggest.sh set up: the TypeSuggest service, its
# autostart and the program in ~/.local/bin. Asks before turning Omarchy's
# Fcitx5 back on and before deleting settings and learned phrases.

set -uo pipefail

bold=$'\033[1m'
dim=$'\033[2m'
reset=$'\033[0m'

target="$HOME/.local/bin/typesuggest"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/typesuggest"

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

printf '%sRemove TypeSuggest%s\n\n' "$bold" "$reset"
confirm "Stop TypeSuggest and remove the program?" || { echo "Nothing was changed."; exit 0; }

run systemctl --user disable --now typesuggest.service
run rm -f "$HOME/.config/systemd/user/typesuggest.service" "$target"
run systemctl --user daemon-reload

if systemctl --user cat omarchy-fcitx5.service >/dev/null 2>&1 &&
  ! systemctl --user is-enabled --quiet omarchy-fcitx5.service &&
  confirm "Turn Omarchy's Fcitx5 input method back on?"; then
  run systemctl --user enable --now omarchy-fcitx5.service
fi

if [[ -d $config_dir ]] && confirm "Also delete settings and learned phrases ($config_dir)?"; then
  run rm -rf "$config_dir"
fi

printf '\nTypeSuggest removed. Remove the bar icon with:\n  omarchy plugin remove io.github.abdulrahmanhr.typesuggest\n'
read -r -p "Press Enter to close. " _ || true
