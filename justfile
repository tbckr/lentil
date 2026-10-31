# Lentil's development commands. Run them inside the development shell, which
# supplies just and every tool below; CI runs the same recipes. The Android
# recipes need the x86_64 Linux shell, the only one with the Android SDK.
#
# Recipes that take arguments pass them on unchanged, for example
# `just test sync` or `just coverage --html`.

set positional-arguments

# Lists the recipes.
default:
    @just --list --unsorted

# Runs the tests except the doc tests.
[group('rust')]
test *args:
    cargo nextest run --workspace "$@"

# Runs the doc tests, which nextest skips.
[group('rust')]
test-doc:
    cargo test --workspace --doc

# Runs clippy over all targets with warnings as errors.
[group('rust')]
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Checks the formatting of the Rust code.
[group('rust')]
fmt-check:
    cargo fmt --all --check

# Measures the line coverage of lentil-core and fails below 80 %.
[group('rust')]
coverage *args:
    cargo llvm-cov nextest -p lentil-core --fail-under-lines 80 "$@"

# Mutation-tests lentil-core changes since the merge base with a ref, or all of it (hours).
[group('rust')]
mutants *ref:
    scripts/mutation.sh "$@"

# Measures the load time of a synthetic 50,000-transaction ledger.
[group('rust')]
load-time:
    cargo test -p lentil-core --release --test differential -- --ignored --nocapture

# Runs the lentil CLI from the current directory.
[group('ledger')]
[no-cd]
run *args:
    cargo run -p lentil-cli -- "$@"

# Runs the reference check of Python Beancount on a ledger file.
[group('ledger')]
[no-cd]
bean-check file:
    uv run bean-check "$1"

# Builds, unit-tests and lints the app and checks its licenses; arguments go to Gradle.
[group('android')]
android *args:
    cd android && ./gradlew "$@" :app:assembleDebug :app:testDebugUnitTest :app:lint :app:checkLicenses

# Runs the Gradle wrapper in android/.
[group('android')]
gradle *args:
    cd android && ./gradlew "$@"

# Runs detekt and checks the formatting of the Kotlin code.
[group('android')]
kotlin-lint:
    detekt --build-upon-default-config --config android/detekt.yml --input android/app/src,android/build-logic/src
    find android -name "*.kt" -not -path "*/build/*" -exec ktfmt --kotlinlang-style --dry-run --set-exit-if-changed {} +

# Formats the Kotlin code.
[group('android')]
kotlin-fmt:
    find android -name "*.kt" -not -path "*/build/*" -exec ktfmt --kotlinlang-style {} +

# Checks the licenses, bans, sources and advisories of the crates and unused ones.
[group('deps')]
deps-check:
    cargo deny check bans licenses sources
    scripts/scan.sh rust
    cargo machete

# Runs the vulnerability and malware scans: rust, gradle, malware or all.
[group('deps')]
scan *args:
    scripts/scan.sh "$@"

# Upgrades the direct dependencies: rust, android, pins or all.
[group('deps')]
upgrade *set:
    scripts/deps.sh upgrade "$@"

# Lists what the upgrades leave out, for review.
[group('deps')]
outdated:
    scripts/deps.sh outdated

# Evaluates the flake for every system and builds the package for this one.
[group('nix')]
flake-check:
    nix flake check --all-systems

# Builds the CLI package into result/bin/lentil.
[group('nix')]
package:
    nix build

# Checks the GitHub workflows, with shellcheck on their run scripts.
[group('workflows')]
actionlint:
    actionlint

# Formats the Rust, Kotlin and Nix code.
[group('all')]
fmt: kotlin-fmt
    cargo fmt --all
    nix fmt

# Runs the quick checks before a commit.
[group('all')]
check: fmt-check lint test test-doc

# Runs the checks of ci.yml; mutation testing runs with `just mutants origin/main`.
[group('all')]
ci: check deps-check coverage android kotlin-lint flake-check
