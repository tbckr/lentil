# PRD: Lentil (working title)

## Summary

Lentil makes [Beancount](https://github.com/beancount/beancount/) practical for everyday use: a Git repository of standard Beancount files is the single source of truth, and several interchangeable clients work on it following a shared convention. In Lentil, review means confirming suggestions, not editing text.

- **Clients:** the CLI sets up the repository, imports CSV exports, re-applies the rules and checks the ledger; the Android app triages, tags and records trips; Fava or rustfava is the read-only desktop view.
- **Language:** everything in the repository is written in English, including this PRD.

## Problem

Beancount has an open, version-controllable data format but a clunky workflow: creating, editing and reviewing entries happens in a text editor. The real bottleneck is not the text format but the number of decisions per transaction.

- **Capture:** imports require custom scripts; cash and one-off entries on the go are cumbersome.
- **Review:** every imported transaction needs decisions (counter account, duplicate, transfer between own accounts), usually made in an editor.
- **Trust:** whether an import is complete stays invisible without balance reconciliation.
- **Mobile:** there is no good way to use short idle moments for triage.
- **Alternatives:** Git-based solutions with a comfortable UI are centrally hosted, for example [BeanHub](https://docs.beanhub.io/getting-started/what-is-beanhub/).

## Target users

Lentil is built for technically skilled individuals who want to own their financial data; existing Beancount users are served on a best-effort basis.

| User | Situation | Needs | Priority |
| --- | --- | --- | --- |
| Technically skilled individual | Runs or chooses a Git remote, wants to own their financial data | Little everyday effort, reliable numbers, no lock-in | Primary |
| Existing Beancount users | Have kept a ledger for years, often with custom scripts | A better review UI without rewriting their ledger | Secondary, best-effort |
| Households with several people | Shared accounts, several reviewers | Permissions and conflict rules for several people | Not supported |

Not a target group: people who cannot or will not use a Git remote. A client can set up the repository, but Git does not disappear entirely.

## Principles

Ten principles apply to every decision; a feature that breaks one of them is not built.

1. **The Git repository is the single source of truth.** No central logic and no central state apart from a replaceable, dumb Git remote.
2. **Standard Beancount.** Everything Lentil writes is valid Beancount and readable by other tools such as Fava. No proprietary add-on format.
3. **Computable locally everywhere, identical everywhere.** All clients use the same core. Anything outside the supported profile is shown read-only, never computed wrongly. The core is rustledger, used as it is; Lentil builds the convention and the clients around it. A plugin belongs to the profile only if rustledger runs it natively and its Python original exists for Fava; anything else Lentil writes into the files as standard Beancount.
4. **Git visible during setup and in emergencies, invisible in daily use.** No everyday step requires a commit, a merge or text-level conflict resolution.
5. **Review means confirming.** Every transaction arrives with a suggestion; the goal is one gesture per transaction.
6. **Convention before client.** The convention is documented in its own spec. Every client that follows it is a first-class client.
7. **Minimal changes.** Clients change only the affected characters of a file; formatting and comments are preserved.
8. **Privacy by default.** No telemetry, and no network traffic except to the Git remote, the ECB for exchange rates and portfolio services the user connects with their own account, starting with Parqet; bank access may follow later.
9. **Net worth exact, categories approximate but visible.** Balances are checked with zero tolerance. A category may stay open as `Expenses:Uncategorized` or `Expenses:Unattributed`, as long as `lentil status` shows how much is still open.
10. **Mobile-first.** What users do often works on the phone with one gesture: import, triage, tags, capture and a glance at net worth. What is rare, such as writing rules by hand, maintaining the list of tags or setting up a repository, may stay at the desktop.

## Goals and success criteria

Lentil aims at an overview of spending and net worth plus tax preparation, with under 15 minutes of review per month; budgets are a product goal too.

| Goal | Success criterion | Measurement (local, no telemetry) |
| --- | --- | --- |
| Low effort | Review takes under 15 minutes per month | Active time in the triage view on Android, measured on the device |
| Few decisions | Over 90 % of transactions are only confirmed | From Git history: share of transactions where only the flag changed from `!` to `*` |
| Completeness | No undetected import gaps | All balance assertions pass; every import run has a balance assertion, from the CSV or entered by the user, or `lentil status` reports the run as unchecked |
| Lasting use | Still in use after three months | Triage commits in at least 10 of the first 13 weeks |

Lentil does not take these measurements itself yet.

## Non-goals

The non-goals also appear in the README; they filter feature requests from the community.

| Non-goal | Reason |
| --- | --- |
| Budgets | A product goal, but not part of the current scope; will use [Fava's `custom "budget"` format](https://beancount.io/docs/Tips/budgets) |
| Receipts from a document archive | Valuable, but not needed for the core goals |
| Own web UI | Fava plus CLI may be enough; built only when the success criteria show a gap. Saves TypeScript, npm and the security work of a local web server |
| Automatic bank access (FinTS) | Needs a product registration with the Deutsche Kreditwirtschaft, renewal of strong authentication every 180 days, a second runtime and a bot container; CSV exports cover the need for now |
| Matching manual entries with later imports | Lentil avoids the case: manual transactions may not post to imported accounts ([Spec section 9](SPEC.md#9-manual-entries-and-balance-snapshots)) |
| Desktop app (Tauri) | Requires the own web UI; Tauri adds per-OS packaging and a WebKitGTK test matrix on Linux, which pays off mainly for other users |
| iOS | Android first; a GPL core complicates the App Store |
| Several people or households | Lentil supports a single user; the data model must not rule out several |
| End-to-end encrypted remote | Lentil relies on a Git remote the user trusts |
| Commercial bank aggregators | Contracts and cost; CSV exports are enough. A portfolio service the user connects with their own account, such as Parqet, does not count |
| Third-party plugins and importers | Lentil computes only the defined profile. Plugins that rustledger runs natively and that exist as Python originals may join it (principle 3). Other foreign code would run only in a WASM sandbox without access to files, network or credentials, as rustledger already does for Python plugins |
| Hosted service | Contradicts principle 1 |

## Usage scenarios

Lentil covers the core loop of import and mobile triage, plus tags and trips; Fava or rustfava serves as the read-only desktop view.

| ID | Scenario | Client |
| --- | --- | --- |
| N1 | Set up an empty repository (`init`) | CLI |
| N2 | Import a bank's CSV export, entering the bank balance if the CSV has none | CLI |
| N3 | Confirm or correct open transactions on the go | Android |
| N4 | Connect the Android app to an existing repository | Android |
| N15 | Tag transactions for evaluations across accounts, by rule and by hand on the phone | Android, CLI |

## System overview

Every client acts in one of three roles with separate write areas, so concurrent writers never edit the same text; conflicts are resolved per transaction by operation replay, not by a Git merge.

| Role | Clients | Writes |
| --- | --- | --- |
| Importer | CLI import | New files only: import runs |
| Editor | Android; `lentil rules apply` | Flag, account, payee, narration, tags and metadata of existing transactions; new accounts; rules at the top of `rules.toml`; manual entries, balance snapshots and trip events in its own device's files |
| Admin | CLI `init` and hand edits checked with `lentil check` | Anything, as long as the result follows the spec |

Fava and rustfava only read the repository.

## Functional requirements

Every requirement has an ID; how it is implemented is defined in the [convention spec](SPEC.md).

### A. Repository and convention

| ID | Requirement |
| --- | --- |
| FR-A1 | `init` creates an empty repository: directory layout, main file, config file, .gitignore, a Fava configuration with sidebar links to open transactions and uncategorized expenses, and a README explaining the convention. |
| FR-A2 | A config file in the repository defines the main file, sources and account defaults; write areas per client role are fixed by the spec. |
| FR-A3 | Every transaction Lentil imports carries a stable ID as metadata, derived from the bank's reference or the transaction's fields; trip events carry a generated ID. |
| FR-A4 | Review status uses only the standard flags: `!` for open, `*` for reviewed. |
| FR-A5 | Every change is validated before it is committed; invalid states are never pushed. |
| FR-A8 | An optional `tags.toml` lists known tags, tag families and mutually exclusive groups; `lentil check` warns about unknown tags and clashing tags but never blocks ([Spec section 16](SPEC.md#16-suggestion-rules)). |

### B. Computation

| ID | Requirement |
| --- | --- |
| FR-B1 | All clients compute balances with the same core and arrive at identical results. |
| FR-B2 | The core supports a defined profile of directives, options and plugins. Outside that profile a client switches to read-only mode and states the reason. |
| FR-B3 | Balance assertions are evaluated, and deviations are shown per account. |
| FR-B4 | A comparison against the reference implementation (`bean-check`) is available via the CLI. |

### C. Import

| ID | Requirement |
| --- | --- |
| FR-C1 | CSV import with a declarative mapping per source ([Spec section 15](SPEC.md#15-import-mappings)): encoding, header, columns, number and date formats, sign convention, rows to skip and where the balance comes from. Mappings are data only; a changed export format aborts the import instead of being guessed. |
| FR-C2 | Imports are idempotent: already imported transactions are recognized by their ID and skipped. |
| FR-C3 | Imports only write new files to the import directory of their source and never change existing files. |
| FR-C4 | Every imported transaction gets a suggested counter account and the flag `!`. |
| FR-C5 | Where the CSV contains a balance, each import run writes it as a balance assertion. |
| FR-C6 | If the CSV contains no balance, the CLI asks for the balance shown in online banking and the date it refers to, and writes it as a balance assertion with zero tolerance. The user may skip; `lentil status` then reports the import as unchecked. |
| FR-C7 | If the export no longer reproduces the ID of an already imported transaction in its date range, for example after a mapping change, a new transaction with the same date, amount and currency is reported as a possible duplicate and imported only after confirmation. |
| FR-C8 | Imported transactions keep the bank's counterparty, purpose and booking type as separate protected metadata. Suggestions learn from counterparty and purpose, not from a payee or narration the user has edited. |
| FR-C22 | Learned suggestions come from the words of counterparty and purpose, so a varying wording still teaches. Words that occur in many transactions of a source count little, and an account is suggested only if the words a transaction shares with a reviewed transaction booked to it carry at least half of the transaction's weight; otherwise `fallback_account` stays ([Spec section 16](SPEC.md#16-suggestion-rules)). |
| FR-C18 | Rules in `rules.toml` match the bank's counterparty, purpose and booking type and set the payee, the counter account or both; for each field, the first matching rule that sets it wins ([Spec section 16](SPEC.md#16-suggestion-rules)). |
| FR-C19 | `lentil rules apply` applies the rules again to transactions already imported and commits the changed payees and counter accounts. By default it changes only open transactions; options include reviewed ones, limit it to the payee or the account, or only list the changes. It never confirms a transaction ([Spec section 16](SPEC.md#16-suggestion-rules)). |
| FR-C20 | Rules in `rules.toml` also add tags. The tags of all matching rules are united, a rule may match tags if it only adds tags, and a rule never adds a tag whose exclusive rival the transaction already carries ([Spec section 16](SPEC.md#16-suggestion-rules)). |
| FR-C21 | `lentil rules apply` adds the rule tags a transaction lacks, on reviewed transactions only on request, and can be limited to tags. On request it also lists tags that the rules no longer yield for a transaction, among the tags some rule adds, and removes each only after confirmation ([Spec section 16](SPEC.md#16-suggestion-rules)). |

### D. Triage (Android)

| ID | Requirement |
| --- | --- |
| FR-D1 | A queue shows all open transactions, newest first. |
| FR-D2 | Confirming with one gesture sets the flag to `*`. |
| FR-D3 | The counter account can be changed via search and recently used accounts; a missing account can be opened directly. Changing the account confirms the transaction. |
| FR-D14 | When changing the account, the user can switch on "Always book like this" and pick words of counterparty and purpose; a rule on these words that sets the account goes to the top of `rules.toml`, and open transactions it matches get the account as their suggestion and stay open. Without the switch, changing the account stays one tap ([Spec section 16](SPEC.md#16-suggestion-rules)). |
| FR-D15 | Changing the account refreshes the open transactions still on the fallback account from what was learned ([Spec section 16](SPEC.md#16-suggestion-rules)). |
| FR-D4 | A duplicate, such as a transaction imported again after the bank changed its wording, can be dismissed; it stays in its file as a comment and is never imported again. |
| FR-D5 | Triage works offline; changes are transferred on the next sync. |
| FR-D6 | The UI syncs without using Git terminology. |
| FR-D12 | Open transactions show their tags. A tag is added from the taxonomy or removed with one tap, picking a tag replaces its exclusive rival, and tagging never confirms a transaction. |
| FR-D13 | A trip is started and ended on the phone. Open transactions from its start to three days after its end show its tag as a suggestion that one tap accepts ([Spec section 9](SPEC.md#9-manual-entries-and-balance-snapshots)). |
| FR-D10 | Split transactions are shown read-only; splitting happens in a text editor (FR-E6). |

### E. Reporting and corrections (desktop and CLI)

| ID | Requirement |
| --- | --- |
| FR-E1 | [Fava](https://github.com/beancount/fava) or [rustfava](https://github.com/rustledger/rustfava) serves as a read-only desktop view, including monthly income and expenses per category and net worth over time. |
| FR-E2 | `lentil status` lists unreviewed transactions, failed balance assertions and import runs without a balance check. |
| FR-E3 | Suggestion rules live in `rules.toml` and are edited directly; rules from the app's "Always book like this" go to the top. |
| FR-E6 | Rare complex corrections (splitting, rebooking, several transactions at once) are made in a text editor; `lentil check` validates them before commit. |

### F. Synchronization

| ID | Requirement |
| --- | --- |
| FR-F1 | Every client works on a local clone; Git is transport and change log. |
| FR-F2 | Clients store changes as intents and re-apply them to the new state when a push is rejected (operation replay). |
| FR-F3 | History stays linear; clients never create merge commits. |
| FR-F4 | A conflict is detected and reported; the local change is kept. |
| FR-F5 | Commits carry the user as author, name the client and device, and describe the change in readable form. |
| FR-F6 | Lentil works with any Git remote: the CLI over HTTPS with the credentials of Git's credential helper or over SSH with the keys of the SSH agent, the Android app over HTTPS with a user name and an access token. |
| FR-F7 | The Android app connects to an existing repository: remote URL, user name and access token, the token encrypted with a key kept in the Android Keystore, and a device ID created once on first start. |

## Non-functional requirements

Correct numbers come before everything else; a client that is unsure shows nothing rather than something wrong.

| Area | Requirement |
| --- | --- |
| Correctness | All clients produce identical balances. On real ledgers, tested locally and never in CI, there is no deviation from the reference implementation. |
| Data ownership | Everything in the repository is readable and usable without Lentil; no lock-in. |
| Privacy | No telemetry. Network traffic only to the Git remote. |
| Security | The Git token never lives in the repository; on Android it is encrypted with a key kept in the system keystore. Importers only create new files in their own areas, and validation before push enforces this. |
| Traceability | Every change is a commit naming client, device and a readable description. |
| Format preservation | Changes touch only the affected element, such as a flag, an account name or a string; the rest of the file stays byte-identical, and files are never reformatted. |
| Offline | Triage and tagging on Android work without a network connection. |
| Performance | A synthetic ledger covering ten years with 50,000 transactions loads in under 2 seconds on a mid-range smartphone. So far it is measured on the desktop only. |
| Operation | No server beyond the Git remote: the CLI, the Android app and Fava work directly on clones. |
| Language | English for code, comments, commit messages, spec, documentation and UI strings; German is the first UI translation. Internationalization is built in. |
| License | Lentil is licensed under GPL-3.0-only, like its core [rustledger](https://docs.rs/crate/rustledger-core/latest/source/README.md). |
| Platforms | CLI: Linux amd64 and arm64, macOS arm64. Android: Android 13 (API 33) or newer, arm64-v8a; x86\_64 only for emulator tests. Packaging: a Nix flake; other formats on demand. CI tests the core on Linux and macOS, and builds, lints and unit-tests the Android app on Linux. Windows is not supported. |
| Distribution | There is no release yet. Android is to ship as an APK for F-Droid, signed locally with a key that never enters CI or the repository. |

Engineering standards for testing, dependencies, CI and releases are in [CONTRIBUTING.md](../CONTRIBUTING.md).
