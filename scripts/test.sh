#!/bin/bash

# Checks a TypeSuggest checkout the way a release is checked: formatting, lints,
# the unit tests, the end-to-end behaviour tests (typesuggest/tests/behaviour.rs)
# against the built-in dictionary, the plugin's scripts and manifest, and that
# the plugin and the program carry the same version. Needs only Rust; no
# Wayland session, nothing is installed or started.
#
#   scripts/test.sh            # everything
#   scripts/test.sh --quick    # tests only, skipping formatting and lints
#   scripts/test.sh --data     # also rebuild data/proper_nouns.txt and compare
#                              # (needs hunspell, unmunch and the en_US dictionary)

set -uo pipefail

bold=$'\033[1m'
dim=$'\033[2m'
red=$'\033[31m'
green=$'\033[32m'
reset=$'\033[0m'

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate="$root/typesuggest"
quick=0
data=0
for arg in "$@"; do
  case "$arg" in
    --quick) quick=1 ;;
    --data) data=1 ;;
    -h | --help)
      sed -n '3,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown option: $arg (see --help)" >&2
      exit 2
      ;;
  esac
done

failed=()

check() {
  local name="$1"
  shift
  printf '\n%s== %s%s\n%s$ %s%s\n' "$bold" "$name" "$reset" "$dim" "$*" "$reset"
  if "$@"; then
    printf '%sok%s %s\n' "$green" "$reset" "$name"
  else
    printf '%sFAILED%s %s\n' "$red" "$reset" "$name"
    failed+=("$name")
  fi
}

versions_match() {
  local plugin program
  plugin="$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$root/manifest.json")"
  program="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$crate/Cargo.toml" | head -n1)"
  echo "manifest.json $plugin, Cargo.toml $program"
  [[ -n "$plugin" && "$plugin" == "$program" ]] &&
    grep -A1 '^name = "typesuggest"' "$crate/Cargo.lock" | grep -q "^version = \"$program\"$"
}

manifest_is_json() {
  python3 -c 'import json, sys; json.load(open(sys.argv[1]))' "$root/manifest.json"
}

scripts_parse() {
  local script ok=0
  for script in "$root"/scripts/*.sh; do
    bash -n "$script" || ok=1
  done
  if command -v shellcheck >/dev/null 2>&1; then
    shellcheck -S warning "$root"/scripts/*.sh || ok=1
  else
    echo "shellcheck not installed; checked syntax only"
  fi
  return "$ok"
}

proper_nouns_reproduce() {
  local built
  built="$(mktemp)"
  python3 "$crate/data/build_proper_nouns.py" /usr/share/hunspell/en_US.dic >"$built" &&
    diff -u "$crate/data/proper_nouns.txt" "$built" | head -n 40
  local status=("${PIPESTATUS[@]}")
  rm -f "$built"
  [[ "${status[0]}" == 0 ]]
}

cd "$crate" || exit 1

if ((!quick)); then
  check "formatting" cargo fmt --check
  check "lints" cargo clippy --locked --all-targets -- -D warnings
fi
check "unit tests" cargo test --locked --lib --bins
check "behaviour tests" cargo test --locked --test behaviour
check "versions match" versions_match
check "manifest.json is valid JSON" manifest_is_json
check "plugin scripts" scripts_parse
if ((data)); then
  check "proper_nouns.txt rebuilds the same" proper_nouns_reproduce
fi

echo
if ((${#failed[@]})); then
  printf '%s%d failed:%s %s\n' "$red$bold" "${#failed[@]}" "$reset" "${failed[*]}"
  exit 1
fi
printf '%sAll checks passed.%s\n' "$green$bold" "$reset"
