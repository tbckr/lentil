# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project state

Lentil has a first, unreleased draft next to its design documents:

- `docs/PRD.md`: product requirements. Covers principles, goals and non-goals, and describes what Lentil does today, without releases, in usage scenarios (`N1` …), functional requirements (`FR-A1` …) and non-functional requirements.
- `docs/SPEC.md`: the Lentil Convention Spec. Its RFC 2119 rules make every client interchangeable on one Git repository of Beancount files. It is at `spec_version` 0, so any change may break until version 1.
- `CONTRIBUTING.md`: engineering standards for testing, linting, dependencies, CI and releases.
- `README.md`: short overview with links to the documents above and the non-goals. When a non-goal changes in the PRD, change the README too.
- `flake.nix`: the Nix development shell, the only source of development tools, and the `lentil` package of the CLI, built with crane. Rust is pinned in `rust-toolchain.toml`, Python Beancount in `pyproject.toml` and `uv.lock`.
- `crates/lentil-core`: all logic, on rustledger. Everything works on a `Snapshot` (repository files in memory): `ledger` indexes a snapshot with exact byte spans, `intent` replays edit intents, `validate` checks a state and a role's changes, `import` plans CSV import runs, `reapply` re-applies the rules to imported transactions as editor intents, `learn` derives learned suggestions from the words of reviewed transactions, `correction` computes the intents of one account correction in the app, with its rule and the open transactions that move with it, `tags` reads the tag taxonomy and checks tags against it, `trips` reads trip events and computes the trip tag an open transaction gets suggested, `sync` runs the fetch–replay–validate–commit–push loop on libgit2.
- `crates/lentil-cli`: the `lentil` binary (`init`, `import`, `rules apply`, `status`, `check`); its messages come from a catalog in `i18n.rs`, English and German.
- `crates/lentil-ffi`: UniFFI bindings of the core for the Android app.
- `android/`: the Kotlin and Compose triage app. It holds no finance logic: pending intents live in a JSON file in app storage and the core replays them. `build-logic` runs cargo-ndk and uniffi-bindgen through AGP's Variant API; versions, including the NDK, live in `gradle/libs.versions.toml`, which the flake reads.
- `testdata/`: synthetic bank exports and their mappings.
- `examples/`: a fictional ledger after one import and a review (`examples/ledger`), with the export it came from. `crates/lentil-core/tests/example.rs` generates it and fails when it no longer matches what the core writes.

## Commands

Run every command inside the development shell.

- `just` lists the recipes of the `justfile`, which name the commands below; CI runs the same recipes, so change a command there and not in a workflow.
- `cargo nextest run --workspace` runs the tests; `cargo test --workspace --doc` runs the doc tests, which nextest skips.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all` lint and format.
- `cargo deny check bans licenses sources`, `scripts/scan.sh rust` and `cargo machete` check the dependencies, as CI does. `scripts/scan.sh` without an argument also runs the other checks of `scan.yml`: vulnerabilities in the packages of the APK (`gradle`) and known malware in any package (`malware`).
- `cargo llvm-cov nextest -p lentil-core` measures coverage.
- `scripts/deps.sh upgrade` raises the direct crates within their requirements and the Gradle catalog within each major, then rewrites the Gradle verification metadata and lockfiles; `scripts/deps.sh upgrade pins` raises the exact pins rustledger, UniFFI, Beancount and Fava. `scripts/deps.sh outdated` lists what both leave out: newer majors, the platform versions and the notes that say "Revisit" or "Drop this once". `latest-deps.yml` runs all of it weekly, the upgrades in a gVisor sandbox, and commits nothing.
- `scripts/mutation.sh origin/main` mutation-tests the lines of `lentil-core` changed since the merge base and fails if the tests miss a mutant, as CI does. Without a ref, it mutates all of `lentil-core` and fails below 85 % test efficacy, which takes hours. The settings are in `.cargo/mutants.toml`.
- The differential tests in `crates/lentil-core/tests/differential.rs` compare rustledger with Python Beancount through `uv`; `LENTIL_SKIP_DIFFERENTIAL=1` skips them where uv is missing. `cargo test -p lentil-core --release --test differential -- --ignored --nocapture` measures the load time of a synthetic 50,000-transaction ledger.
- `just example` regenerates `examples/ledger` after a change in what Lentil writes; review the diff like any other output change.
- `just fava` opens the example ledger in Fava, from the uv dependency group `fava`.
- `cargo run -p lentil-cli -- <command>` runs the CLI.
- `nix build` builds the CLI as a package into `result/bin/lentil`, and `nix run . -- <command>` runs it.
- In `android/`, `./gradlew :app:assembleDebug :app:testDebugUnitTest :app:lint :app:checkLicenses` builds the app with the core for `arm64-v8a` and `x86_64`, runs its unit tests and Android Lint, and checks the licenses of its runtime and test dependencies against the allowlist in `deny.toml`. Gradle verifies every dependency against `gradle/verification-metadata.xml` and locks the app's configurations in `app/gradle.lockfile`; after a dependency change, rewrite both with `--write-verification-metadata sha256 --write-locks` and an empty Gradle user home (`-g <empty directory>`), because a warm cache hides artifacts that the clean CI build resolves. `scripts/deps.sh upgrade android` does both after raising the catalog.
- `detekt --build-upon-default-config --config android/detekt.yml --input android/app/src,android/build-logic/src` and `ktfmt --kotlinlang-style` check the Kotlin code.
- `actionlint` checks the GitHub workflows, with shellcheck on their `run` scripts.
- `nix develop .#emulator` adds an emulator and an x86_64 system image for manual tests.

## Development shell

Every development tool comes from the flake's development shell, Python packages through its uv, so never install one any other way.

- Enter it with direnv (`.envrc` contains `use flake`) or with `nix develop`. A change to `flake.nix`, `flake.lock` or `rust-toolchain.toml` rebuilds the shell on the next direnv load, or on `nix-direnv-reload` where nix-direnv reloads manually; under `nix develop`, enter the shell again.
- `uv run bean-check <file>` runs the reference check of Python Beancount from anywhere in the repository. uv creates `.venv` from `uv.lock` on first use.
- `nix fmt` formats every Nix file, and `nix flake check --all-systems` evaluates the flake for every supported system and builds the package for the local one.

## Editing the documents

- Everything in the repository is written in English: code, comments, commit messages, docs and UI strings.
- Sections open with one sentence that states their conclusion, followed by bullets or a table. Keep that shape.
- Requirement and scenario IDs never change, and an ID that leaves the PRD is never reused. The highest IDs used so far are N18, FR-A8, FR-B5, FR-C22, FR-D15, FR-E8 and FR-F8, so new numbers start above them. The tables are grouped by topic: a new entry takes the next free number in its group and goes where its topic fits, even if that breaks the numeric order.
- The documents refer to each other by number. The spec cites `PRD: FR-D13`, `PRD: Security` and its own sections ("section 15"), and the PRD cites "Spec section 15". If you renumber spec sections or rename PRD sections, update the references in both files in the same change.
- In SPEC §18, tick resolved open questions with their outcome, and keep the counts in the lead sentence in line with the lists below it.
- The repository is public. Documents describe the product, not the author's setup, so leave out hosting details, personal tool stack, real banks, account types and household details. Keep examples fictional and test data synthetic. Real bank mappings and sample exports belong in the user's private ledger repository, and tests against real ledgers run only locally, never in CI.
- The documents state results, not how they were reached, and never point to unpublished documents such as reports or notes.
- The ten PRD principles are a hard filter: a feature that breaks one is not built. Check the non-goals table before you propose new scope.

## Architecture (as specified)

There is no server. A Git repository of standard Beancount files is the single source of truth, and every client works on its own clone.

- **Clients:** the CLI handles `init`, CSV import, re-applying rules (`rules apply`), `status` and `check`. The Android app handles triage, tags, trips and rules from corrections ("Always book like this"). Fava or rustfava is the read-only desktop view. An own web UI (`lentil serve`) and a Tauri app wait until Fava plus the CLI demonstrably fall short.
- **One core:** all clients compute with the same core, which is rustledger pinned to an exact version (GPL-3.0-only). Lentil uses rustledger as it is and builds the convention and the clients around it: a plugin is in the profile only if rustledger runs it natively and its Python original exists for Fava, otherwise Lentil writes the data into the files. The spec itself does not depend on any core library. If a ledger uses anything outside the supported profile (SPEC §14), the client treats the repository as read-only instead of computing it approximately. Git runs inside the core too, so the CLI and the Android app share one sync implementation. The Android app holds no finance logic; it keeps its pending intents in its own storage and hands them to the core for replay. UniFFI generates the Kotlin bindings to the core.
- **Roles and write areas (§7):**
  - Importers only create new files (`imports/<source-id>/`, `prices/`).
  - Editors change the flag, account, payee, narration, tags and metadata of existing transactions, insert rules at the top of `rules.toml`, and write their own entries only under `manual/<device-id>/`.
  - The admin role may change anything.

  Fixed include globs in `main.beancount` pick up new files, so no writer maintains include lists. Validation checks every commit against its role.
- **Identity (§5):** clients address transactions by their `lentil_id` metadata. Imports get a deterministic hash, and manual entries get `<device-id>-<ulid>`. No client may change `lentil_id`, `lentil_ref` or the bank's original fields `lentil_counterparty`, `lentil_purpose` and `lentil_type`.
- **Review state (§6):** only the transaction flag carries it: `!` means open, `*` means reviewed.
- **Sync by operation replay (§10–§12):** clients store intents (`confirm`, `set-account`, `dismiss`, …) that carry the old value; they never store text diffs.
  - A sync fetches `main`, builds a fresh working tree, re-applies the intents, validates, makes one commit with the `Lentil-*` trailers and pushes without force. If the push is rejected, it starts over, up to 5 attempts.
  - When an intent is replayed, an already-applied intent is skipped. If the old value no longer matches, the intent is a conflict, and conflicts are shown to the user, never dropped silently.
  - History stays linear: no merge commits and no force-push.
- **Minimal edits:** applying an intent changes only the bytes of the affected element. The rest of the file stays byte-identical, and files are never reformatted.
- **Import pipeline (§8, §15, §16):**
  1. An adapter reads the bank export. In v0 that is CSV with a declarative mapping in `sources/*.toml`. Mappings are pure data and never name commands.
  2. The adapter produces statement records.
  3. The importer assigns IDs, removes duplicates and picks the payee and a suggested account. Rules in `rules.toml` match the original fields and set the payee, the account or both, the first matching rule per field, and add tags, those of all matching rules. Without a rule, the payee is the counterparty, and the account is a suggestion learned from the words of the counterparty and purpose of reviewed transactions, otherwise `fallback_account`.
  4. It adds a balance assertion and writes everything to one new run file.

## Invariants that are easy to get wrong

Each one is spelled out in the spec section given in brackets.

- Every balance assertion carries an explicit zero tolerance (`1234.56 ~ 0.00 EUR`), because the default tolerance hides one-cent gaps. An assertion holds at the start of its date (§8).
- Never write a `pad` without a difference, because unused pads are errors in Python Beancount and rustledger. That is why `add-snapshot` decides about the pad when it is applied. Never pad a source account (§9, §10).
- Dismissed (commented-out) transactions still count for deduplication, so scan comment lines for `lentil_id` too (§8, §10).
- Read the review flag from the parsed transaction directive, not from query results, because query output can return posting flags instead (§6).
- Include globs use at most one `*` per path segment, with no `**` and no character classes (§3).
- Never commit load caches (`.*.cache`, `.*.picklecache`) (§3).
- Report a failing balance assertion. Never hide it or correct it automatically.
- Every posting on a source account is a source posting, and nothing moves a counter posting onto a source account. Transfers between source accounts stay two transactions against `Assets:Transfer`, because a merged transaction has one date and breaks balance assertions between the two booking dates (§7, §8).

## Engineering standards

The full list is in `CONTRIBUTING.md`; network, secrets and language rules are NFRs in the PRD. The points that shape code most:

- Use traits only at I/O boundaries: Git, file system, clock, randomness and import sources. Replay, ID generation and validation stay pure functions. Tests replace the boundaries with fakes, not a mocking library. The Android app wires its dependencies by hand, without a DI framework.
- Tests:
  - Unit tests, differential tests against Python Beancount's `bean-check`, and mutation tests with cargo-mutants. The mutation tests are the actual quality gate: CI fails if the tests miss a mutant in the changed lines of `lentil-core`, and an equivalent mutant goes into `exclude_re` in `.cargo/mutants.toml` with its reason. 80 % line coverage of `lentil-core`, measured with cargo-llvm-cov, is only a floor.
  - Multi-writer sync tests, which fail on lost intents, duplicates or non-linear history, plus property-based tests for replay.
  - cargo-nextest runs the Rust tests but skips doc tests, so `cargo test --doc` runs those separately.
  - Test dependencies are dev-only.
- Linting: for Rust, clippy over all targets with warnings as errors plus selected pedantic lints, configured in `[workspace.lints]`, and rustfmt with default settings. For Kotlin, Android Lint and detekt with warnings as errors, plus ktfmt. CodeQL scans the Rust code and the workflows in CI.
- Dependencies: justify every new direct dependency in its PR. Run cargo-deny, cargo-machete and the license check for Gradle dependencies (`:app:checkLicenses`); cargo-deny admits only crates.io unless its configuration lists an explicit exception. Pin rustledger and UniFFI exactly. Use no update bots except Dependabot, which only bumps the GitHub Actions, and pin every action by commit SHA. The license allowlist exists twice, in `deny.toml`, which the Gradle check reads too, and in `.github/dependency-review-config.yml` for CI's dependency review, so change both together.
- Toolchain: Rust is pinned only in `rust-toolchain.toml`. The Nix dev shell reads it through rust-overlay and supplies every cargo tool, so never `cargo install` one. It also supplies uv with a pinned Python that stays off `PATH`, and Python Beancount is pinned in `pyproject.toml` and `uv.lock`, so never `pip install` it. Bump the Python minor version in `flake.nix` and `requires-python` together.
- Network: no telemetry, and today no traffic except to the Git remote; principle 8 also admits the ECB and portfolio services the user connects, such as Parqet. Secrets never go into the repository, `lentil.toml` included.
- Internationalization is built in, with German as the first translation. The core reports findings and conflicts as typed codes that each client translates, never as English text. `spec_version` governs compatibility between clients, not the app version.
