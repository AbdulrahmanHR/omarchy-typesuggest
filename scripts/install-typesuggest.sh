#!/bin/bash

# Sets up TypeSuggest for the TypeSuggest Omarchy plugin. The panel's
# "Install TypeSuggest" button opens this in a floating terminal through
# omarchy-launch-floating-terminal-with-presentation.
#
# Nothing changes until you confirm. Every command is printed before it runs.
# The only download is the TypeSuggest binary for this plugin version, built
# from the source in typesuggest/ by .github/workflows/release.yml. It is
# installed only if its SHA-256 matches the value committed in
# release/typesuggest-x86_64.sha256. Nothing downloaded is run by a shell.

set -euo pipefail

bold=$'\033[1m'
dim=$'\033[2m'
red=$'\033[31m'
green=$'\033[32m'
reset=$'\033[0m'

plugin_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(jq -r '.version' "$plugin_dir/manifest.json")"
expected_sha="$(cut -d' ' -f1 "$plugin_dir/release/typesuggest-x86_64.sha256" 2>/dev/null || true)"
url="https://github.com/AbdulrahmanHR/omarchy-typesuggest/releases/download/v$version/typesuggest-x86_64"
target="$HOME/.local/bin/typesuggest"

run() {
  printf '%s$ %s%s\n' "$dim" "$*" "$reset"
  "$@"
}

step() {
  printf '\n%s%s%s\n' "$bold" "$*" "$reset"
}

fail() {
  printf '\n%s%s%s\n' "$red" "$*" "$reset" >&2
  read -r -p "Press Enter to close. " _ || true
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

cat <<EOF
TypeSuggest shows Windows-style word suggestions at the text cursor while
you type. This sets up version $version in three steps:

  1. Download the TypeSuggest program, check it and install it
       $url
       SHA-256 must be $expected_sha
       installed to $target

  2. Turn off Fcitx5. Only one input method can run at a time, and Omarchy
     starts Fcitx5 by default. Fcitx5 also handles Omarchy's CapsLock compose
     sequences (~/.XCompose), which stop working while it is off.
       systemctl --user disable --now omarchy-fcitx5.service

  3. Start TypeSuggest now and on every login
       typesuggest --enable-autostart
       systemctl --user restart typesuggest

To undo it later, run:
  $plugin_dir/scripts/uninstall-typesuggest.sh

EOF

if ! confirm "Set up TypeSuggest now?"; then
  echo "Nothing was changed."
  exit 0
fi

[[ $(uname -m) == x86_64 ]] ||
  fail "Prebuilt TypeSuggest is x86_64 only. Build it from source instead: see $plugin_dir/typesuggest/README.md"
[[ $expected_sha =~ ^[0-9a-f]{64}$ ]] ||
  fail "This plugin version has no pinned checksum (release/typesuggest-x86_64.sha256). Nothing was changed."

step "1/3 Downloading TypeSuggest $version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
run curl -fL --proto '=https' --tlsv1.2 -o "$tmp/typesuggest" "$url" ||
  fail "The download failed. Nothing was changed."
actual_sha="$(sha256sum "$tmp/typesuggest" | cut -d' ' -f1)"
[[ $actual_sha == "$expected_sha" ]] ||
  fail "Checksum mismatch (got $actual_sha). Refusing to install; nothing was changed."
echo "Checksum OK."
run install -Dm755 "$tmp/typesuggest" "$target"

step "2/3 Turning off Fcitx5"
if systemctl --user is-enabled --quiet omarchy-fcitx5.service 2>/dev/null ||
  systemctl --user is-active --quiet omarchy-fcitx5.service 2>/dev/null; then
  run systemctl --user disable --now omarchy-fcitx5.service
else
  echo "Fcitx5 is not running; nothing to turn off."
fi

step "3/3 Starting TypeSuggest"
run "$target" --enable-autostart
run systemctl --user restart typesuggest

sleep 2
if systemctl --user is-active --quiet typesuggest; then
  printf '\n%sTypeSuggest is running.%s Start typing in any text field.\n' "$green" "$reset"
else
  printf '\n%sTypeSuggest is not running.%s See: journalctl --user -u typesuggest\n' "$red" "$reset"
  echo "If it says \"Input method unavailable\", another input method is still running."
fi

cat <<EOF

Some apps need one extra setting before suggestions show up (Chromium and
Electron apps, Qt apps on Omarchy); see "App Compatibility" in
  $plugin_dir/typesuggest/README.md

EOF
read -r -p "Press Enter to close. " _ || true
