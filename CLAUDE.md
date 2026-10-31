# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project state

Lentil has a first, unreleased draft of v0.1 next to its design documents:

- `docs/PRD.md`: product requirements. Covers principles, goals, usage scenarios (`N1` …), functional requirements (`FR-A1` …) and non-functional requirements, each with a target release, plus a release plan.
- `docs/SPEC.md`: the Lentil Convention Spec. Its RFC 2119 rules make every client interchangeable on one Git repository of Beancount files. It is at `spec_version` 0, so any change may break until version 1.
- `CONTRIBUTING.md`: engineering standards for testing, linting, dependencies, CI and releases.
- `README.md`: short overview with links to the documents above and the non-goals. When a non-goal changes in the PRD, change the README too.
- `flake.nix`: the Nix development shell, the only source of development tools. Rust is pinned in `rust-toolchain.toml`, Python Beancount in `pyproject.toml` and `uv.lock`.
- `crates/lentil-core`: all logic, on rustledger. Everything works on a `Snapshot` (repository files in memory): `ledger` indexes a snapshot with exact byte spans, `intent` replays edit intents, `validate` checks a state and a role's changes, `import` plans CSV import runs, `sync` runs the fetch–replay–validate–commit–push loop on libgit2.
- `crates/lentil-cli`: the `lentil` binary (`init`, `import`, `status`, `check`); its messages come from a catalog in `i18n.rs`, English and German.
- `crates/lentil-ffi`: UniFFI bindings of the core for the Android app.
- `android/`: the Kotlin and Compose triage app. It holds no finance logic: pending intents live in a JSON file in app storage and the core replays them. `build-logic` runs cargo-ndk and uniffi-bindgen through AGP's Variant API; versions, including the NDK, live in `gradle/libs.versions.toml`, which the flake reads.
- `testdata/`: synthetic bank exports and their mappings.

## Commands

Run every command inside the development shell.

- `cargo nextest run --workspace` runs the tests; `cargo test --workspace --doc` runs the doc tests, which nextest skips.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all` lint and format.
- `cargo deny check` and `cargo machete` check the dependencies.
- `cargo llvm-cov nextest -p lentil-core` measures coverage, and `cargo mutants -p lentil-core --test-tool nextest` runs the mutation tests.
- The differential tests in `crates/lentil-core/tests/differential.rs` compare rustledger with Python Beancount through `uv`; `LENTIL_SKIP_DIFFERENTIAL=1` skips them where uv is missing. `cargo test -p lentil-core --release --test differential -- --ignored --nocapture` measures the load time of a synthetic 50,000-transaction ledger.
- `cargo run -p lentil-cli -- <command>` runs the CLI.
- In `android/`, `./gradlew :app:assembleDebug :app:testDebugUnitTest :app:lint` builds the app with the core for `arm64-v8a` and `x86_64`, runs its unit tests and Android Lint. Gradle verifies every dependency against `gradle/verification-metadata.xml` and locks the app's configurations in `app/gradle.lockfile`; after a dependency change, rewrite both with `--write-verification-metadata sha256 --write-locks`.
- `detekt --build-upon-default-config --config android/detekt.yml --input android/app/src,android/build-logic/src` and `ktfmt --kotlinlang-style` check the Kotlin code.
- `nix develop .#emulator` adds an emulator and an x86_64 system image for manual tests.

## Development shell

Every development tool comes from the flake's development shell, Python packages through its uv, so never install one any other way.

- Enter it with direnv (`.envrc` contains `use flake`) or with `nix develop`. A change to `flake.nix`, `flake.lock` or `rust-toolchain.toml` rebuilds the shell on the next direnv load, or on `nix-direnv-reload` where nix-direnv reloads manually; under `nix develop`, enter the shell again.
- `uv run bean-check <file>` runs the reference check of Python Beancount from anywhere in the repository. uv creates `.venv` from `uv.lock` on first use.
- `nix fmt` formats every Nix file, and `nix flake check --all-systems` evaluates the flake for every supported system.

## Editing the documents

- Everything in the repository is written in English: code, comments, commit messages, docs and UI strings.
- Sections open with one sentence that states their conclusion, followed by bullets or a table. Keep that shape.
- Requirement and scenario IDs never change. The tables are sorted by release (`v0.1`, `v1`, `Later`). A new entry takes the next free number in its group and goes where its release and topic fit, even if that breaks the numeric order. When you add an entry or change its release, update the PRD's release plan table too, because it lists every ID. The non-goals table additionally uses `After v1`, `Open` and `Never`.
- The documents refer to each other by number. The spec cites `PRD: FR-D8`, `PRD: Security` and its own sections ("section 15"), and the PRD cites "Spec section 15". If you renumber spec sections or rename PRD sections, update the references in both files in the same change.
- In SPEC §18, tick resolved open questions with their outcome, and keep the counts in the lead sentence in line with the lists below it.
- The repository is public. Documents describe the product, not the author's setup, so leave out hosting details, personal tool stack, real banks, account types and household details. Keep examples fictional and test data synthetic. Real bank mappings and sample exports belong in the user's private ledger repository, and tests against real ledgers run only locally, never in CI.
- The documents state results, not how they were reached, and never point to unpublished documents such as reports or notes.
- The nine PRD principles are a hard filter: a feature that breaks one is not built. Check the non-goals table before you propose new scope.

## Architecture (as specified)

There is no server. A Git repository of standard Beancount files is the single source of truth, and every client works on its own clone.

- **Clients:** the CLI handles `init`, CSV import, `status`, `check`, `prices`, `export tax` and `adopt`. The Android app handles triage and manual capture. Fava or rustfava is the read-only desktop view. An own web UI (`lentil serve`) and a Tauri app wait until Fava plus the CLI demonstrably fall short.
- **One core:** all clients compute with the same core, which is rustledger pinned to an exact version (GPL-3.0-only). The spec itself does not depend on any core library. If a ledger uses anything outside the supported profile (SPEC §14), the client treats the repository as read-only instead of computing it approximately. Git runs inside the core too, so the CLI and the Android app share one sync implementation. The Android app holds no finance logic; it keeps its pending intents in its own storage and hands them to the core for replay. UniFFI generates the Kotlin bindings to the core.
- **Roles and write areas (§7):**
  - Importers only create new files (`imports/<source-id>/`, later `prices/`).
  - Editors change the flag, account, payee, narration and metadata of existing transactions, and write only under `manual/<device-id>/`.
  - The admin role may change anything.

  Fixed include globs in `main.beancount` pick up new files, so no writer maintains include lists. Validation checks every commit against its role.
- **Identity (§5):** clients address transactions by their `lentil_id` metadata. Imports get a deterministic hash, and manual entries get `<device-id>-<ulid>`. No client may change `lentil_id`, `lentil_ref` or `lentil_desc`.
- **Review state (§6):** only the transaction flag carries it: `!` means open, `*` means reviewed.
- **Sync by operation replay (§10–§12):** clients store intents (`confirm`, `set-account`, `dismiss`, …) that carry the old value; they never store text diffs.
  - A sync fetches `main`, builds a fresh working tree, re-applies the intents, validates, makes one commit with the `Lentil-*` trailers and pushes without force. If the push is rejected, it starts over, up to 5 attempts.
  - When an intent is replayed, an already-applied intent is skipped. If the old value no longer matches, the intent is a conflict, and conflicts are shown to the user, never dropped silently.
  - History stays linear: no merge commits and no force-push.
- **Minimal edits:** applying an intent changes only the bytes of the affected element. The rest of the file stays byte-identical, and files are never reformatted.
- **Import pipeline (§8, §15, §16):**
  1. An adapter reads the bank export. In v0 that is CSV with a declarative mapping in `sources/*.toml`. Mappings are pure data and never name commands.
  2. The adapter produces statement records.
  3. The importer assigns IDs, removes duplicates and picks a suggested account: a `rules.toml` rule if one matches, otherwise a suggestion learned from the `lentil_desc` of reviewed transactions, otherwise `fallback_account`.
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

## Engineering standards

The full list is in `CONTRIBUTING.md`; network, secrets and language rules are NFRs in the PRD. The points that shape code most:

- Use traits only at I/O boundaries: Git, file system, clock, randomness and import sources. Replay, ID generation and validation stay pure functions. Tests replace the boundaries with fakes, not a mocking library. The Android app wires its dependencies by hand, without a DI framework.
- Tests:
  - Unit tests, differential tests against Python Beancount's `bean-check`, and mutation tests with cargo-mutants. The mutation tests are the actual quality gate; 80 % line coverage of `lentil-core`, measured with cargo-llvm-cov, is only a floor.
  - Multi-writer sync tests, which fail on lost intents, duplicates or non-linear history, plus property-based tests for replay.
  - cargo-nextest runs the Rust tests but skips doc tests, so `cargo test --doc` runs those separately.
  - Test dependencies are dev-only.
- Linting: for Rust, clippy over all targets with warnings as errors plus selected pedantic lints, configured in `[workspace.lints]`, and rustfmt with default settings. For Kotlin, Android Lint and detekt with warnings as errors, plus ktfmt.
- Dependencies: justify every new direct dependency in its PR. Run cargo-deny, cargo-machete and the license check for Gradle dependencies; cargo-deny admits only crates.io unless its configuration lists an explicit exception. Pin rustledger and UniFFI exactly. Use no update bots, and pin GitHub Actions by commit SHA.
- Toolchain: Rust is pinned only in `rust-toolchain.toml`. The Nix dev shell reads it through rust-overlay and supplies every cargo tool, so never `cargo install` one. It also supplies uv with a pinned Python that stays off `PATH`, and Python Beancount is pinned in `pyproject.toml` and `uv.lock`, so never `pip install` it. Bump the Python minor version in `flake.nix` and `requires-python` together.
- Network: no telemetry, and no traffic except to the Git remote and the ECB. Secrets never go into the repository, `lentil.toml` included.
- Internationalization is built in from v0.1, with German as the first translation. The core reports findings and conflicts as typed codes that each client translates, never as English text. `spec_version` governs compatibility between clients, not the app version.
