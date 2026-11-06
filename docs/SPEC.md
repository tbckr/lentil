# Lentil Convention Spec

## 1. Status and scope

This spec defines how Lentil clients share one Git repository of Beancount files without a central server. It is a draft for v0. The chosen core is rustledger, but the spec stays independent of any core library.

- **Covers:** repository layout, configuration, entry identity, review state, write areas per client role, import runs, manual entries, edit operations, synchronization, commits, validation and the supported Beancount profile.
- **Does not cover:** user interfaces, the choice of core library, FinTS details and suggestion algorithms. These are implementation choices as long as the rules here hold.
- **Related:** the Lentil PRD defines goals and requirements (FR and NFR IDs); this spec defines the rules that make clients interchangeable.

## 2. Conformance and versioning

A client is conformant if every commit it pushes follows this spec; the repository declares which spec version it uses.

- The key words MUST, MUST NOT, SHOULD and MAY are used as defined in RFC 2119.
- The repository declares `spec_version` in `lentil.toml` (section 4). This draft is version `0`.
- A client MUST NOT write to a repository whose `spec_version` is higher than the client supports. It MAY read it and MUST then show the repository as read-only.
- A client MUST NOT change `spec_version`; only the admin role does that, as part of a documented migration.
- Until version 1, any spec change may be breaking. From version 1 on, breaking changes increase `spec_version`.

## 3. Repository layout

Every file has exactly one kind of writer, and fixed glob patterns in `main.beancount` pick up new files, so writers only ever create files and never maintain include lists.

```
lentil.toml                        configuration (section 4)
.gitignore                         keeps implementation caches out of Git
main.beancount                     entry point: options and includes
fava.beancount                     Fava options and sidebar links, written by init
accounts.beancount                 open, close and commodity directives
rules.toml                         suggestion rules (section 16)
tags.toml                          tag taxonomy (section 16)
sources/
  giro.toml                        import mapping for source "giro" (section 15)
imports/
  giro/
    2026/
      20261020T071502Z.beancount   one import run (section 8)
manual/
  k7p2q9xd/
    2026-10.beancount              manual entries, balance snapshots and trips (section 9)
prices/
  2026/
    20261020.beancount             exchange rates, one file per fetch (section 8)
status/
  bot-1.json                       bot status (section 17)
README.md                          explains the convention
```

- Directory and file names MUST use lowercase `[a-z0-9-]`, apart from the run timestamps and the fixed names above.
- `main.beancount` holds options and exactly these includes. Clients other than the admin role MUST NOT change it.

```beancount
option "title" "Ledger"
option "operating_currency" "EUR"

include "fava.beancount"
include "accounts.beancount"
include "imports/*/*/*.beancount"
include "manual/*/*.beancount"
include "prices/*/*.beancount"
```

- Include paths are relative to the file that contains them. Glob patterns use at most one `*` per path segment, with no `**` and no character classes. Python Beancount 3.2.3, rustledger and go-beancount resolve these patterns identically.
- Every `.beancount` file matched by these patterns is part of the ledger. Clients MUST NOT create `.beancount` files elsewhere.
- Python Beancount 3.2.3 and rustledger reject an include pattern that matches no file. `init` therefore writes three placeholders that hold only comments: `imports/keep/keep/keep.beancount`, `manual/keep/keep.beancount` and `prices/keep/keep.beancount`. They stay in place, and `keep` is reserved (section 4).
- `fava.beancount` contains only `custom "fava-option"` and `custom "fava-sidebar-link"` directives, for example links to the open transactions and to `Expenses:Uncategorized`. Other tools ignore them.
- `init` creates a `.gitignore` containing `.*.cache` and `.*.picklecache`. rustledger and Python Beancount write load caches next to `main.beancount` by default; caches MUST NOT be committed.

## 4. Configuration (`lentil.toml`)

One TOML file at the repository root holds everything clients need beyond the ledger itself; it never holds secrets.

```toml
spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"
price_currencies = ["USD"]        # currencies with ECB rates (section 8)

[[source]]                        # imported account
id = "giro"                       # [a-z0-9-], used in paths and IDs
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"                  # the only importer
mapping = "sources/giro.toml"     # CSV mapping (section 15)

[[snapshot]]                      # balance-tracked account
account = "Assets:PaymentService"
currency = "EUR"
```

- `spec_version`, `main_file`, `fallback_account` and `unattributed_account` are required.
- Each `[[source]]` describes one bank account or card that is imported. `id` MUST be unique, MUST NOT be `keep` (section 3) and MUST NOT change once transactions exist, because it is part of every imported ID.
- Each `[[snapshot]]` describes one balance-tracked account (section 9). An account MUST NOT be both a source account and a snapshot account.
- `fallback_account` is the counter account when no suggestion exists; `unattributed_account` receives differences between balance snapshots. Both MUST be opened in `accounts.beancount`.
- `price_currencies` lists the currencies for which a client fetches ECB reference rates (section 8).
- Credentials, PINs and tokens MUST NOT appear in `lentil.toml` or anywhere in the repository (PRD: Security).
- Mapping files under `sources/` follow section 15. Only the admin role creates or changes them.

## 5. Entry identity

Every transaction Lentil writes carries a stable `lentil_id`; all edits, deduplication and conflict detection address transactions by this ID.

| Key | Required | Value | Example |
| --- | --- | --- | --- |
| `lentil_id` | Yes, on every transaction and trip event Lentil creates | Unique, never changes, never reused | `"giro-3f9a1c2e7b4d"` |
| `lentil_ref` | No | The bank's own reference, copied verbatim | `"2026101912345"` |
| `lentil_counterparty` | Only on imported transactions whose counterparty is not empty (section 8) | The bank's counterparty as delivered, trimmed and inner whitespace collapsed | `"CORNER GROCERY 4711"` |
| `lentil_purpose` | Only on imported transactions whose purpose is not empty (section 8) | The bank's purpose, normalized likewise | `"Card payment"` |
| `lentil_type` | Only on imported transactions whose booking type is not empty (section 8) | The bank's booking type, normalized likewise | `"Debit card"` |

**Imported transactions.** The ID is deterministic, so a repeated import of the same data produces the same ID:

- If the source provides a stable unique reference, `lentil_id` = `<source-id>-` + the first 12 hex characters of SHA-256 over `<source-id>|<reference>`. A reference that repeats within one export is not unique, so the importer MUST stop the run without writing anything.
- Otherwise the hash input is `<source-id>|<booking date>|<amount>|<currency>|<counterparty>|<purpose>|<n>`. `n` is the 0-based occurrence index among transactions with identical other fields in the same export, in the order the bank lists them.
- Fields are normalized before hashing: dates as `YYYY-MM-DD`, amounts as plain decimals with a dot and without trailing zeros (`-64.00` as `-64`, `-64.10` as `-64.1`), so a change in the bank's number format keeps the IDs, and text trimmed with inner whitespace collapsed.

**Manual transactions and trip events.** `lentil_id` = `<device-id>-` + a lowercase ULID.

**Rules.**

- `lentil_id` MUST be unique among active and commented-out transactions and trip events (section 10, `dismiss`).
- No client may change, add or remove `lentil_id`, `lentil_ref` or the original fields `lentil_counterparty`, `lentil_purpose` and `lentil_type`. Users rename payees and narrations freely; the original fields keep the bank's wording, for example for finding the original statement line.
- A transaction's original text is `lentil_counterparty` and `lentil_purpose` joined by one space, a missing field counting as empty, and normalized as above. Learned suggestions key on it, while rules match each original field on its own (section 16).
- Transactions without `lentil_id` (for example written by hand in an editor) are valid but read-only for Lentil until the admin role assigns IDs.

## 6. Review state

Review state lives only in the standard transaction flag, so any Beancount tool shows it without knowing Lentil.

| Flag | Meaning | Set by |
| --- | --- | --- |
| `!` | Open: needs review | Importers, on every transaction they write |
| `*` | Reviewed | Editors, through `confirm` or any other edit by the user, except that adding or removing a tag leaves the flag as it is (section 10), as re-applied rules do (section 16); manual entries start as `*` |

- Importers MUST write `!`. Editors MUST NOT set `!` back once a transaction is `*`, except through an explicit user action.
- The review queue is the set of active transactions with flag `!`, across all files.
- Clients MUST read the review state from the transaction flag of the parsed directive, not from query results. In go-beancount's query language the `flag` column returns a posting's own flag when one is set.
- Lentil does not use posting-level flags. Clients MUST preserve them but MUST NOT give them meaning.

## 7. Roles and write areas

Every client acts in one of three roles, and each role may only touch its own areas; this is what keeps concurrent writers from colliding.

| Role | Used by | May create | May change | MUST NOT |
| --- | --- | --- | --- | --- |
| Importer | CLI import | New run files under `imports/<source-id>/`; price files under `prices/`; `status/<bot-id>.json` | Nothing that already exists | Change, move or remove existing transactions |
| Editor | Android; CLI `rules apply` | Files under `manual/<device-id>/`, including balance snapshots and trip events (section 9); `open` directives in `accounts.beancount` | Flag, payee, narration, tags, links, metadata other than `lentil_id`, `lentil_ref` and the original fields (section 5), and the account of postings that are not on a source account | Change dates, a source posting, `lentil_id`, `lentil_ref` or an original field; move a posting onto a source account; delete transactions; write manual transactions or pads that post to a source account |
| Admin | CLI `init`; hand edits | Anything | Anything | Break any rule of this spec in the resulting state |

- Every posting on a source account is a source posting. Editors never move a posting onto a source account, and suggestions never name one (section 16), so each side of a transfer between two source accounts stays a transaction of its own (section 8).
- A client declares its role in every commit (section 12). Validation checks the commit's changes against that role (section 13).
- At most one importer per source may run at a time. Two importers running on the same source is a configuration error.
- An editor that needs a new account adds its `open` directive through the `open-account` operation (section 10).

## 8. Import runs

Each import run reads one bank export through an adapter (section 15), writes exactly one new file, never touches existing ones, and proves its completeness with a balance assertion where the bank allows it.

```beancount
; lentil import run
; source: giro, started: 2026-10-20T07:15:02Z, client: lentil-cli/0.1.0

2026-10-18 ! "Corner Grocery" "Card payment"
  lentil_id: "giro-3f9a1c2e7b4d"
  lentil_counterparty: "CORNER GROCERY 4711"
  lentil_purpose: "Card payment"
  lentil_type: "Debit card"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Food:Groceries

2026-10-19 ! "City Utilities" "Electricity, monthly instalment"
  lentil_id: "giro-8c01d4e9a2f7"
  lentil_ref: "2026101912345"
  lentil_counterparty: "CITY UTILITIES"
  lentil_purpose: "Electricity, monthly instalment"
  lentil_type: "Direct debit"
  Assets:Bank:Giro        -64.00 EUR
  Expenses:Uncategorized

2026-10-20 balance Assets:Bank:Giro  1234.56 ~ 0.00 EUR
```

**File.** The run file is `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount`, named after the run's start time in UTC. If a file of that name exists, the run takes the next free second. The include pattern of section 3 picks it up; no other file changes. A run with nothing new writes no file and no commit.

**Transactions.**

- Importers MUST import only booked transactions, never pending ones.
- Importers MUST import only transactions booked before the run date; later ones follow with a later run. An export can end in the middle of the run date, and a later export would then number identical same-day transactions differently (section 5).
- Each transaction has exactly two postings: the source posting with the amount as reported by the bank, and a counter posting with elided amount on the suggested account, or on `fallback_account` if there is no suggestion. The opening transaction (below) has the same shape.
- Each transaction carries `lentil_id` (section 5), `lentil_ref` if the bank provides one, and each original field that is not empty, in the order `lentil_counterparty`, `lentil_purpose`, `lentil_type`.
- The payee is the payee of a matching rule (section 16), otherwise the counterparty; a transaction with neither has no payee. The narration is the purpose, or empty without one. The opening transaction (below) is the exception: it has no payee, its narration is fixed, and rules do not apply to it.
- The transaction carries the tags the rules add (section 16), written in the header after the narration, in the order the rules add them. Without tags the header has none. Trips (section 9) play no part at import.
- Transactions are ordered by booking date, then in the order the bank lists them.
- Before writing, the importer collects every `lentil_id` in the ledger, including those of commented-out transactions, and skips transactions whose ID already exists.
- A transaction can get a new ID when the export describes it differently, for example after a mapping change or a change in the bank's wording. If an existing transaction of the same source lies between the export's first and last booking date and the export does not reproduce its `lentil_id`, any new transaction with that transaction's booking date, amount and currency is a possible duplicate. The importer MUST NOT write it without confirmation: the CLI shows both and asks, and an unattended import skips it and reports it.

**Balance assertions.** Beancount checks a `balance` directive at the start of its date. Therefore:

- The importer writes at most one assertion per run and source. A balance assertion holds at the start of its date, so it is dated the day after the last day the balance covers completely. The run date never counts as complete, because the bank may still book on it.
- The assertion MUST carry an explicit zero tolerance, as in `balance Assets:Bank:Giro  1234.56 ~ 0.00 EUR`. With the default tolerance, Python Beancount, rustledger and go-beancount all accept a difference of 0.01 for amounts with two decimals, which would hide a one-cent import gap.
- The mapping says where the balance comes from (section 15). If the CSV contains none, the CLI asks for the balance shown in online banking and the date it refers to, defaulting to the run date. If the user skips, the run file gets the comment `; lentil: unchecked`, and `lentil status` reports the run until a later assertion covers the account.
- A balance that refers to the run date may include run-date transactions. The importer subtracts the run-date transactions contained in the same export and dates the assertion on the run date. If it cannot do this reliably, it writes no assertion.
- If an assertion for the same account and date already exists, the importer writes none and reports a difference, if any.
- A failing assertion means an import gap or an error. It is reported, never hidden and never auto-corrected.

**Opening balance.** The run that writes the first balance assertion for a source account also writes the account's opening transaction, unless the ledger already holds it, active or dismissed.

- Its source posting books the part of the asserted balance that the source's transactions before the assertion, in the ledger and in the run, do not explain. Its counter posting is on `Equity:OpeningBalances`, which `init` opens.
- It carries flag `!`, `lentil_id` = `<source-id>-` + the first 12 hex characters of SHA-256 over `<source-id>|opening`, and `Opening balance` as narration. It carries no original fields.
- It is dated on the source's earliest transaction, but before the assertion.
- If that part is zero, or a posting on the source account has no amount, the importer writes no opening transaction.
- If earlier runs skipped the balance, an import gap among them ends up in the opening balance. The flag `!` puts it in front of the user.

**Transfers.** A transfer between two source accounts, for example paying the card bill from the giro account, is imported from both sides.

- Both transactions stay active, with their own dates and IDs. Both counter postings go to a transfer account, `Assets:Transfer` in repositories created by `init`.
- The transfer account holds the money in transit. A balance that remains on it points to a transfer whose other side is missing.
- Beancount has no dates per posting. A single transaction with postings on both source accounts would carry one date, so a balance assertion of the other account between the two booking dates would fail.

**Exchange rates.** A client that fetches exchange rates reads the ECB reference rates for the currencies in `price_currencies` at most once per business day, by hand or from a timer. It writes them as `price` directives into a new file `prices/<YYYY>/<YYYYMMDD>.beancount`; a day without new rates writes no file. Prices entered by hand go into the same layout under the admin role.

## 9. Manual entries and balance snapshots

Manual entries go into files that only one device writes, so two devices can never collide on them.

- Each device has a `device-id`: 8 random characters of `a-z` and `0-9`, created once on first start and kept on the device.
- Entries go to `manual/<device-id>/<YYYY-MM>.beancount`, by the entry's date, and are appended at the end of the file. The include pattern of section 3 picks up new files.
- Manual transactions carry a `lentil_id` (section 5) and start with flag `*`. Trip events carry a `lentil_id` too and have no flag.
- A manual transaction MUST NOT post to a source account: the import brings those transactions anyway, and Lentil does not match a manual entry to the imported one (section 18). Validation blocks such a posting (section 13).
- A device MUST NOT write into another device's directory. Edits to existing manual entries use the edit operations of section 10 like any other transaction.

**Trips.** A trip records its dates in the device's files, so a client can suggest the trip's tag when the trip's transactions arrive with a later import (PRD: FR-D13).

```beancount
2026-04-10 event "trip" "portugal-2026"
  lentil_id: "k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a"
2026-04-21 event "trip" ""
  lentil_id: "k7p2q9xd-01jcfh3k5n7p9r1s3t5v7w9y2b"
```

- `<date> event "trip" "<name>"` starts a trip and `<date> event "trip" ""` ends it. `<name>` MUST match `[a-z0-9-]+`, and the trip's tag is `trip-<name>`; a trip event whose name does not match is ignored.
- Trip events are appended to the device's month file like other manual entries, through `start-trip` and `end-trip` (section 10). They carry a `lentil_id` (section 5).
- Lentil uses the event type `"trip"` for trips; other event types pass through unchanged (section 14).
- The trip of a date follows from the trip events on or before that date, ordered by date, then by the ULID of their `lentil_id` (section 5), which orders the events of one day by the time they were created, then by file path, then by position in the file. On its date, an event without such an ID comes first. An end event that does not follow a start event ends no trip and is ignored. If the latest remaining event starts a trip, the date lies in that trip. If it ends a trip and is dated at most three days before the date, the date still counts for the trip the preceding start event began, which covers payments that the bank books after the trip. A start event without an end event ends at the next start event, so the later trip wins.
- An active transaction with flag `!` and a `lentil_id`, except a source's opening transaction (section 8), gets the tag of the trip its date lies in suggested, unless it already carries it. A transaction gets at most one trip tag suggested.
- A suggestion is never written by itself. Clients MUST NOT add a suggested tag without an explicit action of the user, and confirming a transaction MUST NOT accept it.

**Balance snapshots.** A balance-tracked account (`[[snapshot]]` in section 4) has no transactions from a bank. Its user enters the balance once or twice a month, and the difference to the computed balance is booked to `unattributed_account`. This keeps net worth exact without importing every transaction.

- The user enters the balance at the start of a day, before that day's transactions.
- The editor computes the account's balance at that point with the core. If it matches, the editor writes only the balance assertion. If it differs, it also writes a `pad` dated the previous day, from the account to `unattributed_account`; the first snapshot of an account pads from `Equity:OpeningBalances`.
- An editor MUST NOT write a `pad` without a difference: Python Beancount and rustledger report unused pads as errors.
- Balance assertions carry `~ 0.00` like those of importers (section 8).
- Snapshots are written only for snapshot accounts. A `pad` on a source account is never written, because it would hide import gaps.

```beancount
2026-10-31 pad Assets:PaymentService Expenses:Unattributed
2026-11-01 balance Assets:PaymentService  80.00 ~ 0.00 EUR
```

## 10. Edit operations

Clients never ship text diffs; they record intents that name what they change, usually a transaction by its ID, and carry the value the user saw where the change has one, so they can be re-applied safely to a newer state.

| Operation | Arguments | Precondition | Effect | Role |
| --- | --- | --- | --- | --- |
| `confirm` | id | Transaction is active | Flag becomes `*` | Editor |
| `set-account` | id, posting index, old, new | Posting has account `old` and is not a source posting; `new` is open and not a source account | Account becomes `new` | Editor |
| `set-payee` | id, old, new | Payee equals `old`; `old` is absent if the transaction has no payee | Payee becomes `new`; without a payee, `"<new>" ` is inserted before the narration | Editor |
| `dismiss` | id | Transaction is active | Transaction is commented out in place | Editor |
| `open-account` | account, date | Account is not open | `<date> open <account>` appended to `accounts.beancount` | Editor, Admin |
| `add-import-run` | source, file, content | File does not exist | New run file written | Importer |
| `add-tag` | id, tag | Transaction is active | `#<tag>` added to the header | Editor |
| `remove-tag` | id, tag | Transaction is active | `#<tag>` removed from the header | Editor |
| `start-trip` | id, date, name | — | `<date> event "trip" "<name>"` with `lentil_id` `id` appended to the device's month file (section 9) | Editor |
| `end-trip` | id, date | — | `<date> event "trip" ""` with `lentil_id` `id` appended to the device's month file (section 9) | Editor |
| `set-narration` | id, old, new | Narration equals `old` | Narration becomes `new` | Editor |
| `set-meta` | id, key, old, new | Key is not `lentil_id`, `lentil_ref` or an original field (section 5); value equals `old` | Value becomes `new` | Editor |
| `add-entry` | device file, entry | File belongs to this device | Entry appended | Editor |
| `add-snapshot` | account, date, amount | Account is a snapshot account; no balance assertion for it on that date | Balance assertion and, only if needed, a pad appended to the device's month file (section 9) | Editor |

**Applying an intent.** For each intent, in creation order:

1. Find the transaction by `lentil_id`. If it does not exist, the intent is a conflict. Intents that name no transaction (`open-account`, `add-import-run`, `start-trip`, `end-trip`) skip this step and follow their precondition and the rules below.
2. If the current value already equals the new value, the intent is done and is not applied again.
3. If the current value equals the expected old value, apply it.
4. Otherwise the intent is a conflict.

**Rules.**

- Applying an intent changes only the bytes of the affected element: the flag character, the account name or the string. Everything else in the file stays byte-identical (PRD: principle 7).
- `add-tag` and `remove-tag` carry no old value. If the tag is already present, or already absent, the intent is done under step 2, so the only conflicts are a missing or dismissed transaction. Adding inserts ` #<tag>` into the header line, before a comment on that line if there is one. Removing deletes the tag where it stands, in the header line or on a following line of the transaction that holds tags or links, together with one separating space; a tag alone on a line is deleted with its whole line. Editing tags never changes the flag (section 6).
- `start-trip` and `end-trip` get their `lentil_id` when they are created. They are done if an event with their `lentil_id` exists, so a trip that is started, ended and started again on one day keeps all three events.
- `dismiss` prepends ` ;  ` to every line of the transaction. It is used for duplicates, for example a transaction imported again after the bank changed its wording (section 8). Commented-out IDs still count for deduplication (section 8), so a dismissed transaction is never imported again.
- `add-snapshot` decides whether a pad is needed when it is applied, not when it is created, so a replay on a newer state never writes an unused pad.
- Intents are stored on the device, outside the repository, until they are pushed. A client MUST NOT drop an intent silently; conflicts are shown to the user, who re-applies or discards them.
- Splitting is not an intent. Rare complex corrections are made in a text editor and validated with `lentil check` (PRD: FR-E6).

## 11. Synchronization

A client never merges text: it rebuilds from the remote, re-applies its pending intents and pushes, and starts over if someone else pushed first.

1. Fetch `main` from the remote.
2. Create a fresh working tree at the fetched commit. Local commits that were never pushed are discarded; pending intents are the only local state.
3. Apply pending intents in creation order (section 10). Conflicting intents leave the queue and are listed for the user.
4. Validate the ledger (section 13). On blocking findings, stop, keep the intents and show the findings.
5. Create one commit (section 12).
6. Push without force. If the remote rejects the push because it moved on, start again at step 1, with exponential backoff and at most 5 attempts. On success, remove the applied intents.

Editors SHOULD sync at the end of each session and MAY sync periodically. An import runs one sync; fetching first also gives the importer the current IDs for deduplication.

## 12. Commits

History is one linear branch of small, labelled commits, so it doubles as an audit log and as the data source for the PRD's success metrics.

```
editor: confirm 12, set-account 3, dismiss 1

Lentil-Spec: 0
Lentil-Role: editor
Lentil-Client: lentil-android/0.1.0
Lentil-Device: k7p2q9xd
```

- The repository has one branch, `main`. Clients MUST NOT create merge commits and MUST NOT force-push.
- One sync produces at most one commit, and a commit contains changes of one role only.
- The subject starts with the role, followed by a readable summary, for example `importer: giro run 20261020T071502Z, 14 transactions`.
- The trailers `Lentil-Spec`, `Lentil-Role`, `Lentil-Client` and `Lentil-Device` (or `Lentil-Bot` for bots) are required.
- Author name and e-mail come from the user's client settings; the device is identified by the trailer.

## 13. Validation

A client validates the complete ledger after applying its intents and before committing; blocking findings stop the commit, warnings are reported but never block.

| Check | Severity |
| --- | --- |
| Parse error in any file reachable from `main.beancount` | Blocking |
| A transaction does not balance | Blocking |
| A posting uses an account that is not open or already closed | Blocking |
| Two transactions share a `lentil_id` | Blocking |
| A change lies outside the write areas of the commit's role (section 7) | Blocking |
| A change introduces a construct outside the supported profile (section 14) | Blocking |
| `main.beancount` differs from section 3, or an include points outside the repository | Blocking |
| A new balance assertion written by an importer has no explicit zero tolerance (section 8) | Blocking |
| A manual transaction or a pad posts to a source account (section 9) | Blocking |
| The core reports any other error, for example an unused pad | Blocking |
| A balance assertion fails | Warning |

- A dismissed transaction counts as unchanged only if its comment block holds the whole transaction, every line prefixed as in section 10. Removing a transaction, or keeping only its `lentil_id` line, is a change outside the editor's write area.
- In its own manual files an editor may append the entries of section 9: transactions with a `lentil_id`, the balance assertions and pads of snapshots, and `event "trip"` entries with a `lentil_id` and a name as in section 9. Any other directive added there, or a trip event with another name, lies outside the editor's write area.
- If the state fetched from the remote already has blocking findings or lies outside the profile, editors and importers MUST NOT write. The client shows the findings; only the admin role can repair the repository.
- Validation uses the client's core. The CLI additionally offers a check against the reference implementation, `bean-check` (PRD: FR-B4).

## 14. Supported profile v0

The v0 profile is the small part of Beancount that a repository created by `init` needs; anything outside it makes the repository read-only for editors and importers (PRD: FR-B2).

| Construct | v0 | Note |
| --- | --- | --- |
| `option "title"`, `option "operating_currency"` | Supported | Other options are outside the profile |
| `include` | Supported | Relative paths; glob patterns only as in section 3 |
| `plugin` | Outside | No plugins in v0 |
| `open`, `close`, `commodity` | Supported |  |
| Transactions with flag, payee, narration, tags, links, metadata | Supported |  |
| Amounts in any currency, price annotations `@` and `@@` | Supported | Needed for foreign currencies |
| Cost annotations `{...}` | Outside |  |
| Posting with elided amount | Supported | At most one per transaction |
| `balance`, `pad`, `price` | Supported | `pad` only on balance-tracked accounts (section 9); `price` fetched as in section 8 or written by hand |
| `note`, `document`, `event`, `query`, `custom` | Supported | Passed through unchanged, except that `event "trip"` marks trips (section 9) |
| `pushtag`, `poptag` | Outside |  |
| Posting flags | Supported | Preserved, no meaning (section 6) |

## 15. Import mappings

An importer reads a bank export through an adapter that turns it into statement records. Everything after that works on records only and knows nothing about the bank: IDs, duplicate checks, suggestions, balance assertions and the run file (section 8). v0 defines one adapter, CSV with a declarative mapping file. Any other adapter, for example for camt.053, MUST produce the same records.

**Statement records.**

| Field | Required | Meaning |
| --- | --- | --- |
| `booking_date` | Yes | Date the bank booked the transaction; becomes the transaction date |
| `amount` | Yes | Signed decimal, negative when money leaves the account |
| `currency` | Yes | From a column, otherwise the source's `currency` (section 4) |
| `counterparty` | No | The other party, as the bank writes it |
| `purpose` | No | Purpose, as the bank writes it |
| `type` | No | Booking type, such as direct debit or card payment, as the bank writes it |
| `reference` | No | The bank's unique reference; becomes `lentil_ref` and the ID input (section 5) |
| `balance_after` | No | Running balance after this booking |

An export may also state one balance and the date it refers to. `counterparty`, `purpose` and `type` become the original fields `lentil_counterparty`, `lentil_purpose` and `lentil_type`, normalized as in section 5.

**Mapping file.** Each `[[source]]` with `importer = "csv"` names a mapping file under `sources/`:

```toml
[file]
encoding = "windows-1252"              # utf-8 (default), windows-1252, iso-8859-1, iso-8859-15
delimiter = ";"                        # default ","
header = ["Buchungsdatum", "Betrag"]   # required: cells that identify the header row
end = "^Kontostand"                    # optional: regular expression for the first line after the table

[numbers]
decimal = ","                          # default "."
thousands = "."                        # optional

[columns]
booking_date = { column = "Buchungsdatum", format = "%d.%m.%Y" }
amount = { column = "Betrag" }         # or { debit = "Soll", credit = "Haben" }; add invert = true to flip the sign
currency = "Währung"                   # optional
counterparty = ["Zahlungsempfänger"]   # optional: one or more columns
purpose = ["Verwendungszweck"]         # optional: one or more columns
type = ["Buchungstext"]                # optional: one or more columns
reference = "Kundenreferenz"           # optional

[[skip]]                               # optional, repeatable
column = "Status"
equals = "Vorgemerkt"                  # or matches = "<regular expression>"

[balance]
from = "column"                        # "column", "text" or "none"
column = "Saldo"                       # with from = "column"
# pattern = 'Kontostand am (?P<date>\S+): (?P<amount>\S+)'   # with from = "text"
# date_format = "%d.%m.%Y"                                  # with from = "text"
```

**Rules.**

- Mapping files are data. They MUST NOT name programs, scripts or shell commands, because any device with write access to the repository could otherwise run code on the machine that imports. A converter that runs a program is configured outside the repository.
- The format is versioned with `spec_version`. A client MUST reject a mapping with an unknown key or a missing required key. New keys are added only when a real export needs them, together with a test case.
- The header row is the first row that contains every cell listed in `header`, compared after trimming. Rows before it are ignored. If no row matches, the import aborts, because the export no longer matches the mapping.
- Columns are referenced by header name only, never by position. A reordered export still imports; a renamed column aborts the import and names the column.
- The table ends at the first line matching `end`, or at the end of the file. Empty rows are ignored.
- Every other row MUST parse or match a `[[skip]]` entry. A row that does not parse aborts the import with its line number; rows are never dropped silently. The import summary counts skipped rows.
- If an export contains pending transactions, the mapping MUST skip them with a `[[skip]]` entry (section 8: booked transactions only). `equals` compares the trimmed cell exactly; `matches` takes a regular expression in the syntax of section 16.
- Dates use strftime codes (`%d`, `%m`, `%Y`, `%y`). Amounts drop `thousands`, read `decimal` as the decimal point and accept a leading `+` or `-`. With `debit` and `credit`, exactly one of the two cells holds a number: the amount is the credit, or the debit made negative, whatever sign the file gives it. `invert = true` flips the sign after that.
- `counterparty`, `purpose` and `type` join their non-empty cells with one space.
- `type` is not part of the ID (section 5), so adding it to a mapping keeps every ID.
- `from = "column"`: in booking order, each row's balance MUST equal the previous row's balance plus its amount. This check also shows whether the bank lists rows oldest or newest first. If neither order fits, the importer writes no assertion and reports the mapping as suspect. Otherwise it uses the balance after the last booking before the run date.
- `from = "text"`: `pattern` is matched against the lines before the header row and from the `end` line on. The named group `amount` is required; `date` is optional and read with `date_format`. Without `date`, the balance refers to the run date.
- `from = "none"`: the CLI asks for the balance (section 8).
- A ready-made mapping that Lentil ships for a bank comes with an anonymized sample export and its expected records as a test.

## 16. Suggestion rules

Explicit rules live in `rules.toml`, set the payee and the counter account from the original fields, add tags, and always win; learned suggestions are derived from the ledger's history and are never stored, and an optional `tags.toml` lists the tags a ledger uses.

```toml
[[rule]]
match = { source = "giro", counterparty = "(?i)^corner grocery" }
payee = "Corner Grocery"
account = "Expenses:Food:Groceries"

[[rule]]
match = { counterparty = "(?i)^city utilities$" }
payee = "City Utilities"

[[rule]]
match = { counterparty = "(?i)^streamflix" }
account = "Expenses:Subscriptions"
add_tags = ["recurring"]

[[rule]]
match = { counterparty = "(?i)^webhost annual" }
add_tags = ["amortize-year"]

[[rule]]
match = { tags = ["amortize-year"] }
add_tags = ["recurring"]
```

- `match` holds the conditions: regular expressions on `counterparty`, `purpose` and `type`, which are the original fields (section 5); `tags`, a list of tags the transaction must all carry; and optionally the ID of the one `source` the rule applies to. A rule MUST give at least one condition among the regular expressions and `tags`, and all of its conditions must match. A missing original field matches as the empty string.
- Rules match only the original fields and, for rules that only add tags, tags; never the payee, the narration or the account, which users may edit. A rule that sets the payee or the account therefore matches a transaction the same way at import and at any later time.
- A rule sets `payee`, `account`, `add_tags` or several of them, and MUST set at least one. A rule with a `tags` condition MUST NOT set `payee` or `account`: users edit tags, and payee and account would otherwise change with them. `payee` is normalized like the original fields and MUST NOT be empty then. `account` MUST NOT be a source account; transfers go to the transfer account (section 8). Tags in `add_tags` and in `match` are written without `#` and MUST be valid Beancount tags; `match.tags` and `add_tags` MUST NOT be empty.
- A client MUST reject a `rules.toml` with an unknown key or a rule without `match`, so a rule written for an earlier format never applies with a different meaning.
- A client MUST reject a `rules.toml` in which a rule's `source` names no source of `lentil.toml`, so a mistyped ID never disables a rule silently.
- Regular expressions use RE2 syntax without lookaround, so implementations in different languages behave the same.
- Rules are evaluated in file order. For the payee and the account, the first matching rule that sets the field wins, so a payee rule and a later account rule can both apply to one transaction (PRD: FR-C18).
- The tags of all matching rules are united (PRD: FR-C20). They are evaluated to a fixed point in rounds, starting from the transaction's tags. In each round, every rule is matched against the tags at the start of the round, and the round adds the tags of the matching rules that the transaction does not carry yet, rules in file order and each rule's tags in the order of its `add_tags`. The evaluation ends after a round that adds nothing. Rules only add, so the evaluation ends, and the set of tags it yields does not depend on rule order. Rules never remove a tag.
- A rule does not add a tag if the transaction carried another tag of the same exclusive group (below) before the evaluation. The rule still sets its payee and account and adds its other tags. Tags added during the evaluation hold no tag back, so two rules can add clashing tags; the taxonomy check reports that.
- The importer takes the payee from a rule, otherwise from the counterparty. It takes the account from a rule, otherwise from a learned suggestion, and only then uses `fallback_account`. It writes the tags of the evaluation in the order the rounds added them (section 8).
- A client MAY re-apply the rules to transactions already imported (PRD: FR-C19, FR-C21). It acts as editor (section 7) and writes only `set-payee`, `set-account`, `add-tag` and `remove-tag` operations (section 10), so the flag stays as it is. It considers open (`!`) and reviewed (`*`) active transactions with a `lentil_id` in the run files of configured sources, except a source's opening transaction (section 8), and matches `source` against the run file's source. It SHOULD change reviewed transactions only when the user asks for it.
- When re-applying, a field changes only if a rule sets it to a different value, and the account only if exactly one posting of the transaction is not on a source account. Tags of the evaluation that the transaction lacks are added. A change whose operation conflicts is shown to the user and left out; the others are committed.
- A client lists removal candidates only when the user asks for them, for example after narrowing a rule. A removal candidate is a tag on a considered transaction that appears in the `add_tags` of some rule in `rules.toml`, but that the evaluation does not yield for the transaction when it starts from the transaction's tags that appear in no rule's `add_tags`. The hold-back always uses the tags the transaction carries. Lentil cannot tell a tag a rule added from one the user added, so the client removes each candidate only after the user confirms it, through a `remove-tag` intent created at confirmation and not planned again on a newly fetched state. Tags to add are computed after the confirmed removals are applied, so a removed tag never justifies an added one.
- Learned suggestions are computed from reviewed (`*`) transactions. Their input is the original text (section 5) and the source account, never payee or narration, which users may edit (PRD: FR-C8). They suggest only the counter account, never a source account and never tags, and they need no file of their own, so any client can recompute them.
- Only the admin role edits `rules.toml`.

**Tag taxonomy (`tags.toml`).** An optional `tags.toml` at the repository root lists the tags a ledger uses, so editors can offer them and `lentil check` can find typos and clashes (PRD: FR-A8).

```toml
tags = ["recurring", "irregular", "amortize-year", "friends", "tax-relevant"]
families = ["trip-", "share-"]
exclusive = [["recurring", "irregular"]]
```

- `tags` lists known tags without `#`. `families` lists prefixes; a tag that starts with one is known too. `exclusive` lists groups of tags of which a transaction carries at most one. All three keys are optional. A tag counts as known only if it is listed or starts with a family prefix, so the list also fixes the spelling.
- A `tags.toml` does not parse if it is not TOML, has an unknown key, a value of the wrong type, or a tag or prefix that is not a valid Beancount tag. Clients MUST reject such a file, as for `rules.toml`; `lentil check` reports it as an error. Only the admin role edits it.
- Violations are not findings of section 13, so no validation before a commit computes them; `lentil check` reports them as warnings: a tag on a transaction that is not known, a transaction carrying more than one tag of an exclusive group, and a tag in `rules.toml` that is not known.
- Re-applying the rules reads `tags.toml` for its exclusive groups and stops if the file does not parse, as for a broken `rules.toml`; without the file, no group holds a tag back. Import does not read it, because a transaction being imported carries no tags before the evaluation.
- Editors read it to offer tags. If it is missing or does not parse, they offer the tags already used in the ledger and never block on it. When the user picks a tag of an exclusive group of which the transaction already carries another member, the editor says so and replaces that member: it writes `remove-tag` for it and `add-tag` for the pick.

## 17. Bot status file

A bot that fetches accounts automatically reports its health through a small JSON file in the repository, so clients learn about errors and expiring bank access at their next sync without extra infrastructure.

```json
{
  "spec_version": 0,
  "bot": "bot-1",
  "sources": [
    {
      "id": "giro",
      "last_import": "2026-10-20T07:15:38Z",
      "last_error": null,
      "last_error_at": null,
      "auth_expires": "2027-04-12"
    }
  ]
}
```

- The file is `status/<bot-id>.json`; only that bot writes it.
- To avoid a commit per run, the bot updates the file only in the same commit as an import run, or when `last_error` or `auth_expires` changes.
- `last_import` is the last run that wrote new data. `auth_expires` is the bot's estimate of when strong authentication must be renewed (last renewal plus 180 days, unless the bank reports otherwise).
- How early clients warn before `auth_expires` is a client setting.

## 18. Open questions

Seven questions are settled; one remains open.

**Settled**

- [x] **Includes:** glob patterns replace index files (section 3). Python Beancount, rustledger and go-beancount resolve them identically, also on a 10-year ledger.
- [x] **Dismissing:** keep commenting out. All three parsers ignore commented-out transactions, so balances stay correct without plugins. Deduplication scans comment lines for `lentil_id`; rustledger's parse result exposes standalone comments with their source spans.
- [x] **Partial days:** importers import only days before the run date (section 8). Same-day transactions arrive one day later, and in return the occurrence index stays deterministic.
- [x] **Metadata keys:** `lentil_id`, `lentil_ref` and the original fields `lentil_counterparty`, `lentil_purpose` and `lentil_type` keep the `lentil_` prefix. They are long but avoid collisions with other tools.
- [x] **Shared accounts file:** editors keep appending `open` directives to `accounts.beancount` through `open-account`, which is conflict-free.
- [x] **Transfers:** both sides stay transactions of their own, with their own dates and IDs, and book against a transfer account (section 8). A merged transaction would carry one date and break balance assertions between the two booking dates.
- [x] **Commit size:** one commit per sync (section 12).

**Open**

- [ ] **Matching manual and imported entries:** match a manual entry to a later import by account, amount and a date within a few days, as beancount-import does with up to 5 days. Until then, section 9 forbids manual postings on source accounts.
