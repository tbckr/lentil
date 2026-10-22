# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project state

Lentil is still being designed. The repository holds documents only, with no code, build system or tests:

- `docs/PRD.md`: product requirements. Covers principles, goals, usage scenarios (`N1` …), functional requirements (`FR-A1` …) and non-functional requirements, each with a target release, plus a release plan.
- `docs/SPEC.md`: the Lentil Convention Spec. Its RFC 2119 rules make every client interchangeable on one Git repository of Beancount files. It is at `spec_version` 0, so any change may break until version 1.
- `CONTRIBUTING.md`: engineering standards for testing, linting, dependencies, CI and releases.
- `README.md`: short overview with links to the documents above and the non-goals. When a non-goal changes in the PRD, change the README too.

There are no build, lint or test commands yet. The plan is a Rust core and CLI (`lentil-core` on rustledger) plus a Kotlin Android app. When that code lands, add the real commands here instead of guessing them.

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

## Engineering standards (once code exists)

The full list is in `CONTRIBUTING.md`; network, secrets and language rules are NFRs in the PRD. The points that shape code most:

- Use traits only at I/O boundaries: Git, file system, clock, randomness and import sources. Replay, ID generation and validation stay pure functions. Tests replace the boundaries with fakes, not a mocking library. The Android app wires its dependencies by hand, without a DI framework.
- Tests:
  - Unit tests, differential tests against Python Beancount's `bean-check`, and mutation tests. The mutation tests are the actual quality gate; 80 % line coverage of `lentil-core` is only a floor.
  - Multi-writer sync tests, which fail on lost intents, duplicates or non-linear history, plus property-based tests for replay.
  - Test dependencies are dev-only.
- Linting: for Rust, clippy with warnings as errors plus selected pedantic lints, and rustfmt. For Kotlin, Android Lint and detekt with warnings as errors, plus ktfmt.
- Dependencies: justify every new direct dependency in its PR, and run cargo-deny plus the license check for Gradle dependencies. Pin rustledger and UniFFI exactly. Use no update bots, and pin GitHub Actions by commit SHA.
- Network: no telemetry, and no traffic except to the Git remote and the ECB. Secrets never go into the repository, `lentil.toml` included.
- Internationalization is built in from v0.1, with German as the first translation. The core reports findings and conflicts as typed codes that each client translates, never as English text. `spec_version` governs compatibility between clients, not the app version.
