#!/usr/bin/env bash
# Dependency upgrades (CONTRIBUTING: Updating dependencies). latest-deps.yml
# runs them weekly in a sandbox and tests the result without committing it,
# and lists what they leave out for review.
#
# Usage: scripts/deps.sh upgrade [set] | outdated
#   rust     every direct crate to the newest version its requirement allows;
#            the exact pins stay
#   android  every version in android/gradle/libs.versions.toml except the
#            Android toolchain (android-*) to the newest stable release of
#            its major, then the Gradle verification metadata and lockfiles
#            rewritten from an empty Gradle user home, which builds, tests
#            and lints the app
#   pins     the exact pins, rustledger, UniFFI and Beancount, to their
#            newest release; they are upgraded deliberately, so all leaves
#            them out
#   all      rust and android (default)
#   outdated  what the upgrades leave out, for review; never fails: newer
#            releases of the direct dependencies, majors and pins included,
#            the platform versions (Rust, Gradle, Android SDK and NDK, JDK,
#            Python, flake inputs) and the notes in code and configuration
#            that say Revisit or Drop this once
#
# Only direct dependencies are upgraded. Transitive versions change only where
# an upgraded direct dependency requires it: `cargo update -p <crate>` and
# `uv lock --upgrade-package <package>` update conservatively, and Gradle has
# no version ranges in the catalog to re-resolve. Major versions, the Gradle
# wrapper, the Android SDK and NDK, the Rust toolchain, Python and the flake
# inputs stay manual steps.
set -euo pipefail

CDPATH='' cd -- "$(dirname -- "$0")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

catalog=android/gradle/libs.versions.toml

usage() {
  sed -n '/^# Usage/,/^#$/{/^#$/d;s/^# \{0,1\}//;p}' "$0" >&2
  exit 2
}

fail() {
  echo "deps: FAIL - $*" >&2
  exit 1
}

# The registry crates that a workspace member depends on directly, one
# "name version requirement" per line; a crate locked in two versions has two
# lines. An exact requirement (=x.y.z) is a pin.
direct_crates() {
  cargo metadata --format-version 1 --all-features --locked | jq -r '
    .workspace_members as $members
    | ([.packages[] | select(.id as $id | $members | index($id))
        | .dependencies[] | select(.source != null) | {key: .name, value: .req}]
        | from_entries) as $requirements
    | (.packages | map({(.id): .}) | add) as $packages
    | [.resolve.nodes[] | select(.id as $id | $members | index($id)) | .deps[].pkg]
    | unique | map($packages[.]) | map(select(.source != null))
    | .[] | "\(.name) \(.version) \($requirements[.name])"'
}

upgrade_rust() {
  local crates name version requirement args=()
  crates="$(direct_crates)"
  # name@version, so a crate locked in two versions stays unambiguous.
  while read -r name version requirement; do
    if [[ "$requirement" != =* ]]; then
      args+=(-p "$name@$version")
    fi
  done <<<"$crates"
  # Without a spec, cargo would update every crate, transitive ones included.
  ((${#args[@]} > 0)) || fail "no direct crates in the workspace"
  cargo update "${args[@]}"
}

# Relaxes each exact pin to a lower bound, lets the package manager pick the
# newest release and pins that.
upgrade_pins() {
  local crates=() packages=() name version args=()
  # The crates pinned in [workspace.dependencies] of Cargo.toml.
  mapfile -t crates < <(sed -nE 's/^([A-Za-z0-9_-]+) = \{ version = "=[^"]+".*/\1/p' Cargo.toml)
  ((${#crates[@]} > 0)) || fail "no exact pins in Cargo.toml"
  sed -i -E 's/^([A-Za-z0-9_-]+ = \{ version = )"=([^"]+)"/\1">=\2"/' Cargo.toml
  for name in "${crates[@]}"; do
    args+=(-p "$name")
  done
  cargo update "${args[@]}"
  for name in "${crates[@]}"; do
    version="$(cargo pkgid "$name")"
    version="${version##*@}"
    sed -i -E "s/^($name = \\{ version = )\">=[^\"]+\"/\\1\"=$version\"/" Cargo.toml
  done

  # The Python packages pinned in pyproject.toml.
  mapfile -t packages < <(grep -oE '"[A-Za-z0-9_.-]+==[^"]+"' pyproject.toml | sed -E 's/^"([^=]+)==.*/\1/')
  ((${#packages[@]} > 0)) || fail "no exact pins in pyproject.toml"
  for name in "${packages[@]}"; do
    sed -i -E "s/\"$name==([^\"]+)\"/\"$name>=\\1\"/" pyproject.toml
    uv lock --upgrade-package "$name"
    version="$(uv export --frozen --all-groups --no-hashes --no-annotate --no-header |
      sed -nE "s/^$name==(.+)$/\\1/p")"
    [[ -n "$version" ]] || fail "$name is not in uv.lock"
    sed -i -E "s/\"$name>=[^\"]+\"/\"$name==$version\"/" pyproject.toml
  done
  # Records the exact pins in uv.lock again.
  uv lock
}

# The Maven coordinate whose versions decide a version key of the catalog: the
# first library that uses it, or else the marker of the first plugin.
catalog_coordinate() {
  local key=$1 found
  found="$(sed -nE "s/^[A-Za-z0-9-]+ = \\{ module = \"([^\"]+)\", version\\.ref = \"$key\" \\}.*/\\1/p" "$catalog" | head -n 1)"
  if [[ -n "$found" ]]; then
    echo "library $found"
    return
  fi
  found="$(sed -nE "s/^[A-Za-z0-9-]+ = \\{ id = \"([^\"]+)\", version\\.ref = \"$key\" \\}.*/\\1/p" "$catalog" | head -n 1)"
  [[ -n "$found" ]] || fail "no library or plugin in $catalog uses the version $key"
  echo "plugin $found:$found.gradle.plugin"
}

# The newest stable release of a coordinate within the major of the current
# version, or of any major with "any" as the fourth argument; never older than
# the current version. A calendar version such as 2026.09.00 (the Compose BOM)
# has no major.
newest_release() {
  local kind=$1 coordinate=$2 current=$3 scope=${4:-major} group artifact repos=() repo metadata="" versions
  group="${coordinate%%:*}"
  artifact="${coordinate#*:}"
  # The repositories of android/settings.gradle.kts in its order; Google's
  # only serves its own groups, and plugins also come from the plugin portal.
  if [[ "$group" =~ ^(com\.android|com\.google|androidx) ]]; then
    repos+=(https://dl.google.com/android/maven2)
  fi
  repos+=(https://repo.maven.apache.org/maven2)
  if [[ "$kind" == plugin ]]; then
    repos+=(https://plugins.gradle.org/m2)
  fi
  for repo in "${repos[@]}"; do
    if metadata="$(curl -fsSL "$repo/${group//.//}/$artifact/maven-metadata.xml")"; then
      break
    fi
    metadata=""
  done
  [[ -n "$metadata" ]] || fail "no maven-metadata.xml for $coordinate"
  versions="$(grep -oE '<version>[0-9]+(\.[0-9]+)*</version>' <<<"$metadata" | sed -E 's|</?version>||g' || true)"
  # shellcheck disable=SC2086 # one version per word
  printf '%s\n' "$current" $versions |
    awk -F. -v major="${current%%.*}" -v scope="$scope" 'scope == "any" || major >= 1000 || $1 == major' |
    sort -V | tail -n 1
}

upgrade_android() {
  local key current coordinate newest
  while read -r key current; do
    coordinate="$(catalog_coordinate "$key")"
    newest="$(newest_release "${coordinate%% *}" "${coordinate#* }" "$current")"
    if [[ "$newest" != "$current" ]]; then
      sed -i -E "s/^$key = \"$current\"/$key = \"$newest\"/" "$catalog"
      echo "$key $current -> $newest"
    fi
  done < <(sed -nE '/^\[versions\]/,/^\[/{s/^([A-Za-z0-9-]+) = "([^"]+)".*/\1 \2/p}' "$catalog" | grep -v '^android-')

  # Keeps the configuration and drops every component, so that Gradle lists
  # exactly the artifacts this build resolves and no stale version stays. An
  # empty Gradle user home makes it resolve everything a clean CI build does;
  # :app:dependencies resolves the configurations the other tasks leave out.
  local metadata=android/gradle/verification-metadata.xml
  cp "$metadata" "$tmp/verification-metadata.xml"
  sed -i '/^   <components>$/,/^   <\/components>$/{//!d}' "$metadata"
  if ! (cd ./android && ./gradlew -g "$tmp/gradle-home" --no-daemon \
    --write-verification-metadata sha256 --write-locks \
    :app:assembleDebug :app:testDebugUnitTest :app:lint :app:checkLicenses :app:dependencies); then
    # Without its components, the file would fail every later build. The
    # raised catalog stays, so the build can be fixed and the step rerun.
    cp "$tmp/verification-metadata.xml" "$metadata"
    fail "the Gradle build failed; $metadata is restored"
  fi
}

# True if the second version is a later one than the first.
newer() {
  [[ -n "$2" && "$1" != "$2" && "$(printf '%s\n' "$1" "$2" | sort -V | tail -n 1)" == "$2" ]]
}

# One line of the platform report: "label current -> newest (where)" if a
# later version exists.
report() {
  local label=$1 current=$2 newest=$3 where=$4
  if [[ -z "$newest" ]]; then
    echo "$label $current ($where): latest unknown"
  elif newer "$current" "$newest"; then
    echo "$label $current -> $newest ($where)"
  else
    echo "$label $current ($where): up to date"
  fi
}

# The newest release on crates.io that is neither yanked nor a pre-release,
# from the sparse index.
crate_newest() {
  local name=${1,,} path
  case ${#name} in
    1) path=1 ;;
    2) path=2 ;;
    3) path="3/${name:0:1}" ;;
    *) path="${name:0:2}/${name:2:2}" ;;
  esac
  curl -fsS "https://index.crates.io/$path/$name" |
    jq -r 'select(.yanked | not) | .vers | select(test("^[0-9]+\\.[0-9]+\\.[0-9]+(\\+.*)?$"))' |
    sort -V | tail -n 1
}

# A version of the catalog.
catalog_value() {
  sed -nE "s/^$1 = \"([^\"]+)\".*/\\1/p" "$catalog"
}

# The newest stable package of a kind in Google's SDK repository, from $sdk,
# which outdated fetches once.
sdk_newest() {
  local kind=$1
  grep -oE "path=\"$kind;(android-)?[0-9][0-9.]*\"" <<<"$sdk" | sed -E 's/.*;(android-)?//; s/"$//' |
    sort -V | tail -n 1
}

outdated() {
  local crates name version requirement newest found key current coordinate packages sdk

  echo "== Rust: direct crates with a newer release"
  crates="$(direct_crates)"
  found=""
  while read -r name version requirement; do
    newest="$(crate_newest "$name")" || newest=""
    if newer "$version" "$newest"; then
      echo "$name $version -> $newest (requirement $requirement)"
      found=1
    fi
  done <<<"$crates"
  [[ -n "$found" ]] || echo "none"

  echo
  echo "== Python: pinned packages with a newer release"
  packages="$(grep -oE '"[A-Za-z0-9_.-]+==[^"]+"' pyproject.toml | tr -d '"' | sed 's/==/ /')"
  found=""
  while read -r name version; do
    newest="$(curl -fsS "https://pypi.org/pypi/$name/json" | jq -r .info.version)" || newest=""
    if newer "$version" "$newest"; then
      echo "$name $version -> $newest"
      found=1
    fi
  done <<<"$packages"
  [[ -n "$found" ]] || echo "none"

  echo
  echo "== Gradle catalog: versions with a newer stable release, majors included"
  found=""
  while read -r key current; do
    coordinate="$(catalog_coordinate "$key")"
    newest="$(newest_release "${coordinate%% *}" "${coordinate#* }" "$current" any 2>/dev/null)" || newest=""
    if newer "$current" "$newest"; then
      echo "$key $current -> $newest"
      found=1
    fi
  done < <(sed -nE '/^\[versions\]/,/^\[/{s/^([A-Za-z0-9-]+) = "([^"]+)".*/\1 \2/p}' "$catalog" | grep -v '^android-')
  [[ -n "$found" ]] || echo "none"

  echo
  echo "== Platform"
  newest="$(curl -fsS https://static.rust-lang.org/dist/channel-rust-stable.toml |
    sed -nE '/^\[pkg\.rust\]$/,/^\[/{s/^version = "([0-9.]+) .*/\1/p}' | head -n 1)" || newest=""
  report Rust "$(sed -nE 's/^channel = "([^"]+)"/\1/p' rust-toolchain.toml)" "$newest" rust-toolchain.toml
  newest="$(curl -fsS https://services.gradle.org/versions/current | jq -r .version)" || newest=""
  report Gradle "$(sed -nE 's/^distributionUrl=.*gradle-([0-9.]+)-bin\.zip$/\1/p' android/gradle/wrapper/gradle-wrapper.properties)" \
    "$newest" "Gradle wrapper"
  sdk="$(curl -fsS https://dl.google.com/android/repository/repository2-3.xml)" || sdk=""
  report "Android platform" "$(catalog_value android-compileSdk).$(catalog_value android-compileSdkMinor)" "$(sdk_newest platforms)" \
    "android-compileSdk, android-compileSdkMinor"
  newest="$(sdk_newest platforms)"
  report targetSdk "$(catalog_value android-targetSdk)" "${newest%%.*}" android-targetSdk
  report "Android build tools" "$(catalog_value android-buildTools)" "$(sdk_newest build-tools)" android-buildTools
  report "Android NDK" "$(catalog_value android-ndk)" "$(sdk_newest ndk)" android-ndk
  echo "JDK $(sed -nE 's/.*pkgs\.jdk([0-9]+)_headless.*/\1/p' flake.nix) (flake.nix): review its support"
  echo "Python $(sed -nE 's/.*pkgs\.python3([0-9]+);.*/3.\1/p' flake.nix) (flake.nix, requires-python): review its support"
  jq -r '.nodes as $nodes | .nodes.root.inputs | to_entries[] | .key as $input | $nodes[.value]
    | "\($input) \(.original.ref // "(default branch)") locked \(.locked.lastModified | strftime("%Y-%m-%d")),"
      + " \((now - .locked.lastModified) / 86400 | floor) days ago (flake.lock)"' flake.lock

  echo
  echo "== Reminders"
  git grep -n -E '(Revisit|Drop this once) ' -- ':!*.md' ':!scripts/deps.sh' |
    sed -E 's/^([^:]+:[0-9]+:)[[:space:]#/]*/\1 /' || echo "none"
}

case "${1:-}" in
  upgrade)
    case "${2:-all}" in
      rust) upgrade_rust ;;
      android) upgrade_android ;;
      pins) upgrade_pins ;;
      all)
        upgrade_rust
        upgrade_android
        ;;
      *) usage ;;
    esac
    ;;
  outdated) outdated ;;
  *) usage ;;
esac
