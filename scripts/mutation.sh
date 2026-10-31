#!/usr/bin/env bash
# Mutation testing (cargo-mutants) of lentil-core, with the settings in
# .cargo/mutants.toml.
#
# Usage: scripts/mutation.sh [ref]
#   with ref     mutate only the lines changed since the merge base with ref
#                (uncommitted changes count, untracked files only after
#                `git add -N`) and fail if the tests miss any of these
#                mutants; CI runs this on every pull request and push to main
#   without ref  for local checks: mutate all of lentil-core and fail if test
#                efficacy drops below the floor
#                MUTATION_EFFICACY_MIN  minimum percentage (default: 85)
#
# Test efficacy is the share of the viable mutants that the tests catch. A
# mutant whose tests time out counts as caught, since a hanging test fails
# too; a mutant that does not compile (unviable) does not count at all. An
# equivalent mutant, which no test can catch, goes into exclude_re in
# .cargo/mutants.toml together with the reason.
#
# The floor is provisional: a first partial run (October 2026, 174 of 1,056
# mutants) reached 92.9 %. cargo-mutants copies the workspace into TMPDIR once
# per job and builds it there. With three jobs on eight cores, a full run needs
# up to ~8 GB in TMPDIR and takes an estimated two hours; CARGO_MUTANTS_JOBS
# sets the number of jobs (default: 1).
set -euo pipefail

CDPATH='' cd -- "$(dirname -- "$0")/.."

ref="${1:-}"
min="${MUTATION_EFFICACY_MIN:-85}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

args=(--output "$tmp")
if [[ -n "$ref" ]]; then
  # A plain diff with the a/ and b/ path prefixes that cargo-mutants expects,
  # whatever the user's Git configuration says.
  GIT_CONFIG_GLOBAL=/dev/null git diff --no-ext-diff --no-color --src-prefix=a/ --dst-prefix=b/ \
    --merge-base "$ref" -- 'crates/lentil-core/src/*.rs' >"$tmp/changes.diff"
  if [[ ! -s "$tmp/changes.diff" ]]; then
    echo "mutation: no lentil-core changes since $ref"
    exit 0
  fi
  args+=(--in-diff "$tmp/changes.diff")
fi

# Exit status 2 (missed mutants) and 3 (timeouts) are results, not errors.
status=0
cargo mutants "${args[@]}" || status=$?
if [[ $status -ne 0 && $status -ne 2 && $status -ne 3 ]]; then
  exit "$status"
fi

out="$tmp/mutants.out"
# Without any mutant to test, as for a diff that only changes comments,
# cargo-mutants writes no results.
if [[ ! -d "$out" ]]; then
  if [[ -n "$ref" ]]; then
    echo "mutation: no mutants in the lines changed since $ref"
    exit 0
  fi
  echo "mutation: FAIL - no mutants" >&2
  exit 1
fi
caught=$(($(wc -l <"$out/caught.txt") + $(wc -l <"$out/timeout.txt")))
missed=$(($(wc -l <"$out/missed.txt")))

if [[ -n "$ref" ]]; then
  if ((missed > 0)); then
    echo "mutation: FAIL - the tests miss $missed mutants in the lines changed since $ref" >&2
    exit 1
  fi
  echo "mutation: OK"
  exit 0
fi

if ((caught + missed == 0)); then
  echo "mutation: FAIL - no viable mutants" >&2
  exit 1
fi
# awk formats and compares, since bash's printf would follow the locale's
# decimal separator.
efficacy="$(awk -v c="$caught" -v m="$missed" 'BEGIN { printf "%.1f", 100 * c / (c + m) }')"
echo "Test efficacy (lentil-core): ${efficacy}% of $((caught + missed)) viable mutants (minimum ${min}%)"
if awk -v c="$caught" -v m="$missed" -v min="$min" 'BEGIN { exit !(100 * c < min * (c + m)) }'; then
  echo "mutation: FAIL - test efficacy is below ${min}%" >&2
  exit 1
fi
echo "mutation: OK"
