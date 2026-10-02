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
# Step 4 (app settings) asks separately and records what it changes, so
# uninstall-typesuggest.sh can undo exactly that.

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

config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
state_file="${XDG_STATE_HOME:-$HOME/.local/state}/typesuggest/app-settings"
qt_env_file="$config_home/environment.d/90-typesuggest.conf"
ime_flag="--enable-wayland-ime"

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

# ~/... form of a path, for display
tidy() {
  printf '%s' "${1/#"$HOME"/\~}"
}

# Step 4: Chromium-based browsers and Electron apps only send text to an input
# method when started with --enable-wayland-ime. These are the flags files of
# the ones that are installed or already have a flags file, minus the files
# that already have the flag.
flags_files=()
want_flags_file() {
  grep -qxF -- "$ime_flag" "$1" 2>/dev/null || flags_files+=("$1")
}
for app in chromium:chromium brave:brave chrome:google-chrome-stable code:code; do
  file="$config_home/${app%%:*}-flags.conf"
  if [[ -f $file ]] || command -v "${app#*:}" >/dev/null 2>&1; then
    want_flags_file "$file"
  fi
done
# Arch's electron packages read electronNN-flags.conf, or electron-flags.conf if there is none
if [[ -f $config_home/electron-flags.conf ]] || compgen -G '/usr/bin/electron*' >/dev/null; then
  want_flags_file "$config_home/electron-flags.conf"
fi
for file in "$config_home"/electron[0-9]*-flags.conf; do
  if [[ -f $file ]]; then want_flags_file "$file"; fi
done

# Step 4: Omarchy points Qt apps at Fcitx5 (QT_IM_MODULE=fcitx). Qt uses the
# Wayland input method when QT_IM_MODULE is unset or "wayland".
qt_im_module="$(systemctl --user show-environment 2>/dev/null | sed -n 's/^QT_IM_MODULE=//p' || true)"
needs_qt_env=false
if [[ -n $qt_im_module && $qt_im_module != wayland && ! -f $qt_env_file ]]; then
  needs_qt_env=true
fi

app_settings_plan() {
  local file
  if ((${#flags_files[@]})); then
    echo "       Chromium-based browsers and Electron apps: add $ime_flag to"
    for file in "${flags_files[@]}"; do
      if [[ -e $file ]]; then echo "         $(tidy "$file")"; else echo "         $(tidy "$file") (new file)"; fi
    done
  fi
  if $needs_qt_env; then
    echo "       Qt apps: QT_IM_MODULE=wayland in"
    echo "         $(tidy "$qt_env_file") (new file)"
  fi
}

# Lets uninstall-typesuggest.sh undo exactly what step 4 changed
remember() {
  mkdir -p "$(dirname "$state_file")"
  grep -qxF -- "$1" "$state_file" 2>/dev/null || echo "$1" >>"$state_file"
}

add_ime_flag() {
  local file=$1 change=added
  [[ -e $file ]] || change=created
  printf '%s%s: add %s%s\n' "$dim" "$(tidy "$file")" "$ime_flag" "$reset"
  mkdir -p "$(dirname "$file")"
  # Start on a new line if the file doesn't end with one
  if [[ -s $file && -n $(tail -c1 "$file") ]]; then echo >>"$file"; fi
  echo "$ime_flag" >>"$file"
  remember "$change $file"
}

write_qt_env() {
  printf '%s%s: QT_IM_MODULE=wayland%s\n' "$dim" "$(tidy "$qt_env_file")" "$reset"
  mkdir -p "$(dirname "$qt_env_file")"
  cat >"$qt_env_file" <<'EOF'
# Written by the TypeSuggest installer: Qt apps use the Wayland input method
# (TypeSuggest) instead of Fcitx5. uninstall-typesuggest.sh deletes this file.
QT_IM_MODULE=wayland
EOF
}

cat <<EOF
TypeSuggest shows Windows-style word suggestions at the text cursor while
you type. This sets up version $version in four steps:

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

EOF
if ((${#flags_files[@]})) || $needs_qt_env; then
  echo "  4. Turn on suggestions in apps that need a setting (asks again first)"
  app_settings_plan
else
  echo "  4. Apps that need a setting (Chromium, Electron, Qt) are already set up"
fi
cat <<EOF

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

step "1/4 Downloading TypeSuggest $version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
run curl -fL --proto '=https' --tlsv1.2 -o "$tmp/typesuggest" "$url" ||
  fail "The download failed. Nothing was changed."
actual_sha="$(sha256sum "$tmp/typesuggest" | cut -d' ' -f1)"
[[ $actual_sha == "$expected_sha" ]] ||
  fail "Checksum mismatch (got $actual_sha). Refusing to install; nothing was changed."
echo "Checksum OK."
run install -Dm755 "$tmp/typesuggest" "$target"

step "2/4 Turning off Fcitx5"
if systemctl --user is-enabled --quiet omarchy-fcitx5.service 2>/dev/null ||
  systemctl --user is-active --quiet omarchy-fcitx5.service 2>/dev/null; then
  run systemctl --user disable --now omarchy-fcitx5.service
else
  echo "Fcitx5 is not running; nothing to turn off."
fi

step "3/4 Starting TypeSuggest"
run "$target" --enable-autostart
run systemctl --user restart typesuggest

sleep 2
if systemctl --user is-active --quiet typesuggest; then
  printf '%sTypeSuggest is running.%s\n' "$green" "$reset"
else
  printf '%sTypeSuggest is not running.%s See: journalctl --user -u typesuggest\n' "$red" "$reset"
  echo "If it says \"Input method unavailable\", another input method is still running."
fi

step "4/4 Suggestions in browsers, Electron and Qt apps"
if ! ((${#flags_files[@]})) && ! $needs_qt_env; then
  echo "Already set up; nothing to change."
else
  app_settings_plan
  echo
  if confirm "Change these app settings?"; then
    for file in "${flags_files[@]}"; do
      add_ime_flag "$file"
    done
    if $needs_qt_env; then
      write_qt_env
      run systemctl --user daemon-reload
      # Omarchy's autostart copies the session's QT_IM_MODULE=fcitx into systemd,
      # which outranks environment.d until systemd's user manager restarts.
      # Dropping that copy lets the next login use the new file.
      run systemctl --user unset-environment QT_IM_MODULE
    fi
    echo
    if ((${#flags_files[@]})); then
      echo "Fully close and reopen browsers and Electron apps to pick up the flag."
    fi
    if $needs_qt_env; then
      echo "Log out and back in for Qt apps."
    fi
  else
    echo "Skipped. Without these settings, suggestions show up in terminals and GTK apps only."
  fi
fi

cat <<EOF

Start typing in any text field. App-by-app details, and what to do if the
bar sits off to the side of the cursor in Chromium-based apps:
  "App Compatibility" in $plugin_dir/typesuggest/README.md

EOF
read -r -p "Press Enter to close. " _ || true
