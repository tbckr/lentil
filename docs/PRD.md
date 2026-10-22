# PRD: Lentil (working title)

## Summary

Lentil makes [Beancount](https://github.com/beancount/beancount/) practical for everyday use: a Git repository of standard Beancount files is the single source of truth, and several interchangeable clients work on it following a shared convention. In Lentil, review means confirming suggestions, not editing text.

- **Clients:** CSV import through the CLI, Android app for quick triage, Fava plus CLI for month-end close and reporting, an own web UI only if needed.
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
| Households with several people | Shared accounts, several reviewers | Permissions and conflict rules for several people | Not in v1 |

Not a target group: people who cannot or will not use a Git remote. A client can set up the repository, but Git does not disappear entirely.

## Principles

Nine principles apply to every decision; a feature that breaks one of them is not built.

1. **The Git repository is the single source of truth.** No central logic and no central state apart from a replaceable, dumb Git remote.
2. **Standard Beancount.** Everything Lentil writes is valid Beancount and readable by other tools such as Fava. No proprietary add-on format.
3. **Computable locally everywhere, identical everywhere.** All clients use the same core. Anything outside the supported profile is shown read-only, never computed wrongly.
4. **Git visible during setup and in emergencies, invisible in daily use.** No everyday step requires a commit, a merge or text-level conflict resolution.
5. **Review means confirming.** Every transaction arrives with a suggestion; the goal is one gesture per transaction.
6. **Convention before client.** The convention is documented in its own spec. Every client that follows it is a first-class client.
7. **Minimal changes.** Clients change only the affected characters of a file; formatting and comments are preserved.
8. **Privacy by default.** No telemetry, and no network traffic except to the Git remote and the ECB for exchange rates; bank access may follow later.
9. **Net worth exact, categories approximate but visible.** Balances are checked with zero tolerance. A category may stay open as `Expenses:Uncategorized` or `Expenses:Unattributed`, as long as `lentil status` shows how much is still open.

## Goals and success criteria

v1 delivers an overview of spending and net worth plus tax preparation, with under 15 minutes of review per month. Budgets are also a product goal but come after v1.

| Goal | Success criterion | Measurement (local, no telemetry) |
| --- | --- | --- |
| Low effort | Review takes under 15 minutes per month | Active time in the triage view on Android, measured on the device |
| Few decisions | Over 90 % of transactions are only confirmed | From Git history: share of transactions where only the flag changed from `!` to `*` |
| Completeness | No undetected import gaps | All balance assertions pass; every import run has a balance assertion, from the CSV or entered by the user, or `lentil status` reports the run as unchecked |
| Lasting use | Still in use after three months | Triage commits in at least 10 of the first 13 weeks |

Measurement starts with the first month of real use in v0, so suggestion quality and effort become visible early.

## Non-goals for v1

The non-goals also appear in the README; they filter feature requests from the community.

| Non-goal | Reason | Earliest |
| --- | --- | --- |
| Budgets | Product goal, but not part of v1; will use [Fava's `custom "budget"` format](https://beancount.io/docs/Tips/budgets) | After v1 |
| Investments with lots and cost basis | Booking is the most complex part of the core | After v1 |
| Receipts from a document archive | Valuable, but not needed for the core goals | After v1 |
| Own web UI | Fava plus CLI may be enough; built only when the success criteria show a gap. Saves TypeScript, npm and the security work of a local web server in v1 | Later |
| Automatic bank access (FinTS) | Needs a product registration with the Deutsche Kreditwirtschaft, renewal of strong authentication every 180 days, a second runtime and a bot container; CSV exports cover the need for now | Later |
| Matching manual entries with later imports | v1 avoids the case: manual capture is limited to accounts without import, and `lentil status` reports manual postings on imported accounts | After v1 |
| Desktop app (Tauri) | Requires the own web UI; Tauri adds per-OS packaging and a WebKitGTK test matrix on Linux, which pays off mainly for other users | Later |
| iOS | Android first; a GPL core complicates the App Store | Open |
| Several people or households | v1 supports a single user; the data model must not rule out several | Open |
| End-to-end encrypted remote | v1 relies on a Git remote the user trusts | Open |
| Commercial bank aggregators | Contracts and cost; CSV exports are enough | Open |
| Third-party plugins and importers | v1 computes only the defined profile. Later, foreign code runs only in a WASM sandbox without access to files, network or credentials, as rustledger already does for Python plugins | After v1 |
| Hosted service | Contradicts principle 1 | Never |

## Usage scenarios

v0 proves the core loop of import and mobile triage; capture, month-end close and taxes follow in v1. Fava or rustfava serves as the read-only desktop view throughout; month-end close and taxes run as CLI commands.

| ID | Scenario | Client | Release |
| --- | --- | --- | --- |
| N1 | Set up an empty repository (`init`) | CLI | v0.1 |
| N2 | Import a bank's CSV export, entering the bank balance if the CSV has none | CLI | v0.1 |
| N3 | Confirm or correct open transactions on the go | Android | v0.1 |
| N4 | Connect the Android app to an existing repository | Android | v0.1 |
| N5 | Record a cash payment or a one-off entry on an account without import | Android | v1 |
| N6 | Glance at balances and monthly spending | Android | v1 |
| N7 | Month-end close: open items, balance reconciliation, monthly overview | CLI, Fava | v1 |
| N8 | Tax preparation: mark tax-relevant postings and export them per year | CLI | v1 |
| N9 | Record a transaction in a foreign currency, e.g. while travelling | All | v1 |
| N10 | Resolve a transaction that was changed on two devices | Android | v1 |
| N11 | Adopt an existing ledger (`adopt`) | CLI | v1 |
| N12 | Record current balances of accounts without import, such as a payment service or a retirement account | Android | v1 |
| N13 | Fetch accounts automatically via FinTS | Bot | Later |
| N14 | Renew strong authentication for automatic bank access every 180 days | Bot, CLI | Later |

## System overview

Every client acts in one of three roles with separate write areas, so concurrent writers never edit the same text; conflicts are resolved per transaction by operation replay, not by a Git merge.

| Role | Clients | Writes |
| --- | --- | --- |
| Importer | CLI import, `lentil prices` | New files only: import runs and price files |
| Editor | Android; an own web UI if built | Flag, account, payee, narration and metadata of existing transactions; new accounts; manual entries and balance snapshots in its own device's files |
| Admin | CLI (`init`, `adopt`, configuration) | Anything, as long as the result follows the spec |

Fava and rustfava only read the repository.

## Functional requirements

Every requirement has an ID and a target release; how it is implemented is defined in the [convention spec](SPEC.md).

### A. Repository and convention

| ID | Requirement | Release |
| --- | --- | --- |
| FR-A1 | `init` creates an empty repository: directory layout, main file, config file, .gitignore, a Fava configuration with sidebar links to open transactions and uncategorized expenses, and a README explaining the convention. | v0.1 |
| FR-A2 | A config file in the repository defines the main file, sources and account defaults; write areas per client role are fixed by the spec. | v0.1 |
| FR-A3 | Every transaction Lentil creates carries a stable ID as metadata, derived from the bank's reference or the transaction's fields for imports and generated for manual entries. | v0.1 |
| FR-A4 | Review status uses only the standard flags: `!` for open, `*` for reviewed. | v0.1 |
| FR-A5 | Every change is validated before it is committed; invalid states are never pushed. | v0.1 |
| FR-A6 | `adopt` takes over an existing ledger without rewriting existing entries. Only new entries get IDs; migrating old entries happens only on explicit request. | v1 |
| FR-A7 | Every account with a balance is tracked in one of two ways, set in the config: by import (transactions from the bank, strict balance assertions) or by balance snapshots (periodic balances only; differences go to an unattributed account). An account is never both. | v1 |

### B. Computation

| ID | Requirement | Release |
| --- | --- | --- |
| FR-B1 | All clients compute balances with the same core and arrive at identical results. | v0.1 |
| FR-B2 | The core supports a defined profile of directives, options and plugins. Outside that profile a client switches to read-only mode and states the reason. | v0.1 |
| FR-B3 | Balance assertions are evaluated, and deviations are shown per account. | v0.1 |
| FR-B4 | A comparison against the reference implementation (`bean-check`) is available via the CLI. | v0.1 |
| FR-B5 | Foreign currencies are converted to the main currency using prices. | v1 |

### C. Import

| ID | Requirement | Release |
| --- | --- | --- |
| FR-C1 | CSV import with a declarative mapping per source ([Spec section 15](SPEC.md#15-import-mappings)): encoding, header, columns, number and date formats, sign convention, rows to skip and where the balance comes from. Mappings are data only; a changed export format aborts the import instead of being guessed. | v0.1 |
| FR-C2 | Imports are idempotent: already imported transactions are recognized by their ID and skipped. | v0.1 |
| FR-C3 | Imports only write new files to the import directory of their source and never change existing files. | v0.1 |
| FR-C4 | Every imported transaction gets a suggested counter account and the flag `!`. | v0.1 |
| FR-C5 | Where the CSV contains a balance, each import run writes it as a balance assertion. | v0.1 |
| FR-C6 | If the CSV contains no balance, the CLI asks for the balance shown in online banking and the date it refers to, and writes it as a balance assertion with zero tolerance. The user may skip; `lentil status` then reports the import as unchecked. | v0.1 |
| FR-C7 | If the export no longer reproduces the ID of an already imported transaction in its date range, for example after a mapping change, a new transaction with the same date, amount and currency is reported as a possible duplicate and imported only after confirmation. | v0.1 |
| FR-C8 | Imported transactions keep the bank's original description in protected metadata. Suggestions learn from it, not from a payee or narration the user has edited. | v0.1 |
| FR-C9 | `lentil source add` drafts a mapping from a sample export (encoding, delimiter, header, date and number formats, columns) and shows the first parsed rows for confirmation. | v1 |
| FR-C10 | Transfers between own accounts are detected and merged into one transaction. | v1 |
| FR-C11 | Corrections made during triage improve future suggestions; rules live in the repository. | v1 |
| FR-C12 | `lentil prices` fetches ECB reference rates for the currencies in use and writes them as price directives; it can run manually or from a timer. | v1 |
| FR-C13 | FinTS fetching runs in a thin Python adapter that hands transactions over as CSV or JSON; import logic stays in the core. | Later |
| FR-C14 | A bot fetches accounts unattended on a schedule and runs as a container. | Later |
| FR-C15 | The bot writes a status file to the repository (last run, errors, expiry of strong authentication), only when it changes. Android shows it after sync. | Later |
| FR-C16 | Strong authentication is renewed via an interactive CLI command with approval in the banking app (decoupled). | Later |
| FR-C17 | Optional push notifications via a generic webhook. | Later |

### D. Triage (Android)

| ID | Requirement | Release |
| --- | --- | --- |
| FR-D1 | A queue shows all open transactions, newest first. | v0.1 |
| FR-D2 | Confirming with one gesture sets the flag to `*`. | v0.1 |
| FR-D3 | The counter account can be changed via search and recently used accounts; a missing account can be opened directly. | v0.1 |
| FR-D4 | A duplicate, such as a transfer imported from both accounts, can be dismissed; it stays in its file as a comment and is never imported again. | v0.1 |
| FR-D5 | Triage works offline; changes are transferred on the next sync. | v0.1 |
| FR-D6 | The UI syncs without using Git terminology. | v0.1 |
| FR-D7 | Payee and narration can be corrected; the bank's original description stays unchanged (FR-C8). | v1 |
| FR-D8 | Cash payments and one-off entries on accounts without import can be recorded quickly; they go into one file per device. Entries on imported accounts are refused, because the import brings them anyway. | v1 |
| FR-D9 | A quick overview shows balances per account and the current month's spending per category. | v1 |
| FR-D10 | Split transactions are shown read-only; splitting happens in a text editor (FR-E6). | v1 |
| FR-D11 | For balance-tracked accounts the app asks for current balances, about once or twice a month. Lentil writes a balance assertion and, only if the balance differs, a pad to the unattributed account. | v1 |

### E. Month-end close and reporting (desktop and CLI)

| ID | Requirement | Release |
| --- | --- | --- |
| FR-E1 | [Fava](https://github.com/beancount/fava) or [rustfava](https://github.com/rustledger/rustfava) serves as a read-only desktop view, including monthly income and expenses per category and net worth over time. | v0.1 |
| FR-E2 | A first `lentil status` lists unreviewed transactions, failed balance assertions and import runs without a balance check. | v0.1 |
| FR-E3 | Suggestion rules live in `rules.toml` and are edited directly; rules added from triage corrections follow with FR-C11. | v0.1 |
| FR-E4 | `lentil status` lists open items: unreviewed transactions, failed balance assertions, accounts without a recent import, manual postings on imported accounts (possible duplicates), and the month's uncategorized and unattributed amounts. | v1 |
| FR-E5 | `lentil export tax` exports postings marked as tax-relevant per tax year; the details are decided in v1. | v1 |
| FR-E6 | Rare complex corrections (splitting, rebooking, several transactions at once) are made in a text editor; `lentil check` validates them before commit. | v1 |
| FR-E7 | A local web UI served by `lentil serve` is built only if Fava plus CLI demonstrably fall short against the success criteria. If built: a static React single-page app with a typed API client whose transport can be swapped; all logic stays in the core. | Later |
| FR-E8 | A Tauri desktop app wraps the web UI. Requires FR-E7. | Later |

### F. Synchronization

| ID | Requirement | Release |
| --- | --- | --- |
| FR-F1 | Every client works on a local clone; Git is transport and change log. | v0.1 |
| FR-F2 | Clients store changes as intents and re-apply them to the new state when a push is rejected (operation replay). | v0.1 |
| FR-F3 | History stays linear; clients never create merge commits. | v0.1 |
| FR-F4 | A conflict is detected and reported; the local change is kept. | v0.1 |
| FR-F5 | Commits carry the user as author, name the client and device, and describe the change in readable form. | v0.1 |
| FR-F6 | Lentil works with any Git remote over HTTPS or SSH. | v0.1 |
| FR-F7 | The Android app connects to an existing repository: remote URL and credentials, kept in the system keystore, and a device ID created once on first start. | v0.1 |
| FR-F8 | Conflicts are resolved per transaction in the UI. | v1 |

## Non-functional requirements

Correct numbers come before everything else; a client that is unsure shows nothing rather than something wrong.

| Area | Requirement |
| --- | --- |
| Correctness | All clients produce identical balances. On real ledgers, tested locally and never in CI, there is no deviation from the reference implementation. |
| Data ownership | Everything in the repository is readable and usable without Lentil; no lock-in. |
| Privacy | No telemetry. Network traffic only to the Git remote and the ECB for exchange rates. |
| Security | Credentials (the Git token now, bank credentials later) never live in the repository; on Android they are kept in the system keystore. Importers only create new files in their own areas, and validation before push enforces this. |
| Local web UI | If built: binds only to 127.0.0.1. Requires a random access token generated at each start. Rejects requests whose Host header is not the local address (DNS rebinding). Sends no CORS headers and protects every write against cross-site requests. |
| Traceability | Every change is a commit naming client, device and a readable description. |
| Format preservation | Changes touch only the affected element, such as a flag, an account name or a string; the rest of the file stays byte-identical, and files are never reformatted. |
| Offline | Triage and capture on Android work without a network connection. |
| Performance | A synthetic ledger covering ten years with 50,000 transactions loads in under 2 seconds on a mid-range smartphone. |
| Operation | No server beyond the Git remote: the CLI, the Android app and Fava work directly on clones. A timer may run `lentil prices`. |
| Language | English for code, comments, commit messages, spec, documentation and UI strings; German is the first UI translation. Internationalization is built in from v0.1. |
| License | Lentil is licensed under GPL-3.0-only, like its core [rustledger](https://docs.rs/crate/rustledger-core/latest/source/README.md). |
| Platforms | CLI: Linux amd64 and arm64, macOS arm64. Android: Android 13 (API 33) or newer, arm64-v8a; x86\_64 only for emulator tests. Packaging: a Nix flake; other formats on demand. CI tests the core on Linux and macOS and the Android app in an emulator on Linux. Windows comes later. |
| Distribution | Android as an APK, targeting F-Droid. The APK is signed locally with a key that never enters CI or the repository; this is set up before the first public APK release. |

Engineering standards for testing, dependencies, CI and releases are in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Release plan

v0.1 proves the core loop, and v1 makes Lentil fit for everyday use. The phases carry no dates on purpose, because implementation is agent-driven.

| Phase | Content | Scenarios | Requirements |
| --- | --- | --- | --- |
| v0.1 | Core loop: set up a repository, import CSV exports, triage on Android, read-only view in Fava | N1–N4 | FR-A1–A5, FR-B1–B4, FR-C1–C8, FR-D1–D6, FR-E1–E3, FR-F1–F7 |
| v1 | Everyday use: capture, overview, month-end close, taxes, foreign currencies, conflict resolution, adopting existing ledgers, drafting CSV mappings | N5–N12 | FR-A6, FR-A7, FR-B5, FR-C9–C12, FR-D7–D11, FR-E4–E6, FR-F8 |
| Later | Automatic bank access, own web UI, desktop app | N13, N14 | FR-C13–C17, FR-E7, FR-E8 |
