#!/bin/bash

# Sets up TypeSuggest for the TypeSuggest Omarchy plugin. The panel's
# "Install TypeSuggest" button opens this in a floating terminal through
# omarchy-launch-floating-terminal-with-presentation.
#
# Nothing changes until you confirm. Every command is printed before it runs.
# The package comes from the AUR through Omarchy's own helper; nothing is
# downloaded or built by this script itself.

set -euo pipefail

bold=$'\033[1m'
dim=$'\033[2m'
red=$'\033[31m'
green=$'\033[32m'
reset=$'\033[0m'

run() {
  printf '%s$ %s%s\n' "$dim" "$*" "$reset"
  "$@"
}

step() {
  printf '\n%s%s%s\n' "$bold" "$*" "$reset"
}

fail() {
  printf '\n%s%s%s\n' "$red" "$*" "$reset" >&2
  exit 1
}

confirm() {
  if command -v gum >/dev/null 2>&1; then
    gum confirm "$1"
  else
    local answer
    read -r -p "$1 [y/N] " answer
    [[ $answer == [yY] || $answer == [yY][eE][sS] ]]
  fi
}

undo_help() {
  cat <<'EOF'
To undo this later:
  systemctl --user stop typesuggest
  typesuggest --disable-autostart
  omarchy-pkg-drop typesuggest
  systemctl --user enable --now omarchy-fcitx5.service
EOF
}

cat <<'EOF'
TypeSuggest shows Windows-style word suggestions at the text cursor while
you type. This sets it up in three steps:

  1. Install the AUR package "typesuggest"
       omarchy-pkg-aur-add typesuggest

  2. Turn off Fcitx5. Only one input method can run at a time, and Omarchy
     starts Fcitx5 by default. Fcitx5 also handles Omarchy's CapsLock compose
     sequences (~/.XCompose), which stop working while it is off.
       systemctl --user disable --now omarchy-fcitx5.service

  3. Start TypeSuggest now and on every login
       typesuggest --enable-autostart
       systemctl --user start typesuggest

EOF
undo_help
echo

if ! confirm "Set up TypeSuggest now?"; then
  echo "Nothing was changed."
  exit 0
fi

command -v omarchy-pkg-aur-add >/dev/null 2>&1 \
  || fail "omarchy-pkg-aur-add was not found. This script needs Omarchy."

step "1/3 Installing the typesuggest package"
run omarchy-pkg-aur-add typesuggest || fail "The package did not install. Nothing else was changed."

step "2/3 Turning off Fcitx5"
if systemctl --user cat omarchy-fcitx5.service >/dev/null 2>&1; then
  run systemctl --user disable --now omarchy-fcitx5.service
else
  echo "omarchy-fcitx5.service is not installed; nothing to turn off."
fi

step "3/3 Starting TypeSuggest"
run typesuggest --enable-autostart
run systemctl --user start typesuggest

sleep 2
if systemctl --user is-active --quiet typesuggest; then
  printf '\n%sTypeSuggest is running.%s Start typing in any text field.\n' "$green" "$reset"
else
  printf '\n%sTypeSuggest is not running.%s See: journalctl --user -u typesuggest\n' "$red" "$reset"
  echo "If it says \"Input method unavailable\", another input method is still running."
fi

cat <<'EOF'

Some apps need one extra setting before suggestions show up (Chromium and
Electron apps, Qt apps on Omarchy); see the TypeSuggest README:
  https://github.com/AbdulrahmanHR/typesuggest#-app-compatibility

EOF
undo_help
