#!/usr/bin/env bash
# Vulnerability scans (CONTRIBUTING: Vulnerability scanning). scan.yml runs
# them on main, scan-release.yml on the latest release, and ci.yml runs the
# rust check on every change.
#
# Usage: scripts/scan.sh [check] [dir]
#   rust     vulnerabilities in the crates that ship: cargo-deny's advisories
#            check without dev-dependencies, with the ignores in deny.toml
#   gradle   vulnerabilities in the packages that ship in the APK: osv-scanner
#            on the releaseRuntimeClasspath of android/app/gradle.lockfile
#   malware  known malware in any package of any lockfile, dev-dependencies,
#            build tools and Python Beancount included
#   all      all three (default)
#   dir      the checkout to scan (default: this repository); the settings,
#            deny.toml and osv-scanner.toml, always come from this repository
#
# Only precise signals fail a check. A vulnerability needs attacker input, and
# build, lint and test tools only process Lentil's own code, so rust and
# gradle look only at what ships. Malware does harm wherever it runs, on
# developer machines and in CI too, so malware looks at every package.
set -euo pipefail

root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
check="${1:-all}"
dir="$(realpath -- "${2:-$root}")"
cd -- "$root"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Ignores for osv-scanner, once a finding needs one.
osv_config=()
if [[ -f osv-scanner.toml ]]; then
  osv_config=(--config osv-scanner.toml)
fi

rust() {
  # --locked scans the versions in Cargo.lock instead of resolving anew.
  cargo deny --manifest-path "$dir/Cargo.toml" --config deny.toml --exclude-dev --locked \
    check advisories
}

gradle() {
  local lockfile="$dir/android/app/gradle.lockfile"
  # osv-scanner reads no configurations from a Gradle lockfile, so it only
  # gets the lines of the one that ships. It picks its parser by file name.
  grep -E '=([^,]+,)*releaseRuntimeClasspath(,|$)' "$lockfile" >"$tmp/gradle.lockfile" || true
  if [[ ! -s "$tmp/gradle.lockfile" ]]; then
    echo "scan: FAIL - no releaseRuntimeClasspath in $lockfile" >&2
    return 1
  fi
  osv-scanner scan source "${osv_config[@]}" --lockfile "$tmp/gradle.lockfile"
}

# OSV marks malware in three ways: an ID or alias from the OpenSSF malicious
# packages (MAL-), the RustSec category "malicious", which not every malicious
# crate also has a MAL- ID for, and the CWE of GitHub's malware advisories.
# Severity varies between them, so it is no filter.
# shellcheck disable=SC2016 # jq's variables, not the shell's
malware_filter='
  .results[].packages[] as $p
  | $p.vulnerabilities[]?
  | select(
      ([.id] + (.aliases // []) | any(startswith("MAL-")))
      or any(.affected[]?; .database_specific.categories // [] | index("malicious"))
      or (.database_specific.cwe_ids // [] | index("CWE-506"))
    )
  | "\($p.package.ecosystem) \($p.package.name)@\($p.package.version): \(.id) \(.summary // "")"'

malware() {
  local status=0 found
  # Recursive, so a new lockfile is covered without a change here; files that
  # .gitignore lists stay out. Exit status 1 means vulnerabilities, whether
  # any of them is malware is decided below.
  osv-scanner scan source "${osv_config[@]}" --format json --recursive "$dir" >"$tmp/osv.json" || status=$?
  if ((status > 1)); then
    echo "scan: FAIL - osv-scanner exited with $status" >&2
    return 1
  fi
  if ! jq -e '.results | type == "array"' >/dev/null 2>&1 <"$tmp/osv.json"; then
    echo "scan: FAIL - osv-scanner returned no report" >&2
    return 1
  fi
  found="$(jq -r "$malware_filter" <"$tmp/osv.json" | sort -u)"
  if [[ -n "$found" ]]; then
    printf '%s\n' "$found" >&2
    echo "scan: FAIL - malware in the dependency tree: handle it as an incident (CONTRIBUTING.md)" >&2
    return 1
  fi
  echo "scan: no known malware in any lockfile"
}

case "$check" in
  rust) rust ;;
  gradle) gradle ;;
  malware) malware ;;
  all)
    status=0
    rust || status=1
    gradle || status=1
    malware || status=1
    exit "$status"
    ;;
  *)
    sed -n '/^# Usage/,/^#$/{/^#$/d;s/^# \{0,1\}//;p}' "$0" >&2
    exit 2
    ;;
esac
