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
sources/
  giro.toml                        import mapping for source "giro" (section 15)
imports/
  giro/
    2026/
      20261020T071502Z.beancount   one import run (section 8)
manual/
  k7p2q9xd/
    2026-10.beancount              manual entries and balance snapshots (section 9)
prices/
  2026/
    20261020.beancount             exchange rates, one file per fetch (section 8)
status/
  bot-1.json                       bot status (section 17, later)
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

- Include paths are relative to the file that contains them. Glob patterns use at most one `*` per path segment, with no `**` and no character classes. Python Beancount 2.3.6, rustledger and go-beancount resolve these patterns identically.
- Every `.beancount` file matched by these patterns is part of the ledger. Clients MUST NOT create `.beancount` files elsewhere.
- `fava.beancount` contains only `custom "fava-option"` and `custom "fava-sidebar-link"` directives, for example links to the open transactions and to `Expenses:Uncategorized`. Other tools ignore them.
- `init` creates a `.gitignore` containing `.*.cache` and `.*.picklecache`. rustledger and Python Beancount write load caches next to `main.beancount` by default; caches MUST NOT be committed.

## 4. Configuration (`lentil.toml`)

One TOML file at the repository root holds everything clients need beyond the ledger itself; it never holds secrets.

```toml
spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"
price_currencies = ["USD"]        # v1: ECB rates fetched by lentil prices

[[source]]                        # imported account
id = "giro"                       # [a-z0-9-], used in paths and IDs
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"                  # "csv"; "fints" may follow later
mapping = "sources/giro.toml"     # CSV mapping (section 15)

[[snapshot]]                      # balance-tracked account (v1)
account = "Assets:PaymentService"
currency = "EUR"
```

- `spec_version`, `main_file`, `fallback_account` and `unattributed_account` are required.
- Each `[[source]]` describes one bank account or card that is imported. `id` MUST be unique and MUST NOT change once transactions exist, because it is part of every imported ID.
- Each `[[snapshot]]` describes one balance-tracked account (section 9). An account MUST NOT be both a source account and a snapshot account.
- `fallback_account` is the counter account when no suggestion exists; `unattributed_account` receives differences between balance snapshots. Both MUST be opened in `accounts.beancount`.
- `price_currencies` lists the currencies for which lentil prices fetches ECB reference rates (v1).
- Credentials, PINs and tokens MUST NOT appear in `lentil.toml` or anywhere in the repository (PRD: Security).
- Mapping files under `sources/` follow section 15. Only the admin role creates or changes them.

## 5. Entry identity

Every transaction Lentil writes carries a stable `lentil_id`; all edits, deduplication and conflict detection address transactions by this ID.

| Key | Required | Value | Example |
| --- | --- | --- | --- |
| `lentil_id` | Yes, on every transaction Lentil creates | Unique, never changes, never reused | `"giro-3f9a1c2e7b4d"` |
| `lentil_ref` | No | The bank's own reference, copied verbatim | `"2026101912345"` |
| `lentil_desc` | Yes, on every imported transaction | The bank's description (counterparty and purpose) as delivered, inner whitespace collapsed; input for learned suggestions | `"CORNER GROCERY 4711 CARD PAYMENT"` |

**Imported transactions.** The ID is deterministic, so a repeated import of the same data produces the same ID:

- If the source provides a stable unique reference, `lentil_id` = `<source-id>-` + the first 12 hex characters of SHA-256 over `<source-id>|<reference>`.
- Otherwise the hash input is `<source-id>|<booking date>|<amount>|<currency>|<counterparty>|<purpose>|<n>`. `n` is the 0-based occurrence index among transactions with identical other fields in the same export, in the order the bank lists them.
- Fields are normalized before hashing: dates as `YYYY-MM-DD`, amounts as plain decimals with a dot, text trimmed with inner whitespace collapsed.

**Manual transactions.** `lentil_id` = `<device-id>-` + a lowercase ULID.

**Rules.**

- `lentil_id` MUST be unique among active and commented-out transactions (section 10, `dismiss`).
- No client may change or remove `lentil_id`, `lentil_ref` or `lentil_desc`. Users rename payees and narrations freely; `lentil_desc` keeps the bank's wording for suggestions and for finding the original statement line.
- Transactions without `lentil_id` (for example written by hand in an editor) are valid but read-only for Lentil until the admin role assigns IDs.

## 6. Review state

Review state lives only in the standard transaction flag, so any Beancount tool shows it without knowing Lentil.

| Flag | Meaning | Set by |
| --- | --- | --- |
| `!` | Open: needs review | Importers, on every transaction they write |
| `*` | Reviewed | Editors, through `confirm` or any other edit by the user; manual entries start as `*` |

- Importers MUST write `!`. Editors MUST NOT set `!` back once a transaction is `*`, except through an explicit user action.
- The review queue is the set of active transactions with flag `!`, across all files.
- Clients MUST read the review state from the transaction flag of the parsed directive, not from query results. In go-beancount's query language the `flag` column returns a posting's own flag when one is set.
- Lentil does not use posting-level flags. Clients MUST preserve them but MUST NOT give them meaning.

## 7. Roles and write areas

Every client acts in one of three roles, and each role may only touch its own areas; this is what keeps concurrent writers from colliding.

| Role | Used by | May create | May change | MUST NOT |
| --- | --- | --- | --- | --- |
| Importer | CLI import; later a bot | New run files under `imports/<source-id>/`; price files under `prices/` (v1); `status/<bot-id>.json` | Nothing that already exists | Change, move or remove existing transactions |
| Editor | Android; a local web UI if built | Files under `manual/<device-id>/`, including balance snapshots (section 9); `open` directives in `accounts.beancount` | Flag, payee, narration, tags, links, metadata other than `lentil_id`, `lentil_ref` and `lentil_desc`, and the account of postings that are not on the source account | Change dates, the source posting, `lentil_id`, `lentil_ref` or `lentil_desc`; delete transactions; write manual transactions or pads that post to a source account |
| Admin | CLI (`init`, `adopt`, config commands) | Anything | Anything | Break any rule of this spec in the resulting state |

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
  lentil_desc: "CORNER GROCERY 4711 CARD PAYMENT"
  Assets:Bank:Giro        -42.17 EUR
  Expenses:Food:Groceries

2026-10-19 ! "City Utilities" "Electricity, monthly instalment"
  lentil_id: "giro-8c01d4e9a2f7"
  lentil_ref: "2026101912345"
  lentil_desc: "CITY UTILITIES ELECTRICITY INSTALMENT 10/2026"
  Assets:Bank:Giro        -64.00 EUR
  Expenses:Uncategorized

2026-10-20 balance Assets:Bank:Giro  1234.56 ~ 0.00 EUR
```

**File.** The run file is `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount`, named after the run's start time in UTC. The include pattern of section 3 picks it up; no other file changes. A run with nothing new writes no file and no commit.

**Transactions.**

- Importers MUST import only booked transactions, never pending ones.
- Each transaction has exactly two postings: the source posting with the amount as reported by the bank, and a counter posting with elided amount on the suggested account, or on `fallback_account` if there is no suggestion.
- Each transaction carries `lentil_id` and `lentil_desc` (section 5), and `lentil_ref` if the bank provides one.
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

**Exchange rates (v1).** lentil prices fetches the ECB reference rates for the currencies in `price_currencies` at most once per business day, run by hand or from a timer. It writes them as `price` directives into a new file `prices/<YYYY>/<YYYYMMDD>.beancount`; a day without new rates writes no file. Prices entered by hand go into the same layout under the admin role.

## 9. Manual entries and balance snapshots

Manual entries go into files that only one device writes, so two devices can never collide on them (PRD: FR-D8, v1).

- Each device has a `device-id`: 8 random lowercase base32 characters, created once on first start and kept on the device.
- Entries go to `manual/<device-id>/<YYYY-MM>.beancount`, by the entry's date, and are appended at the end of the file. The include pattern of section 3 picks up new files.
- Manual entries carry a `lentil_id` (section 5) and start with flag `*`.
- In v1, a manual transaction MUST NOT post to a source account: the import brings those transactions anyway, and Lentil cannot yet match a manual entry to the imported one. `lentil status` reports manual postings on source accounts as possible duplicates.
- A device MUST NOT write into another device's directory. Edits to existing manual entries use the edit operations of section 10 like any other transaction.

**Balance snapshots (v1).** A balance-tracked account (`[[snapshot]]` in section 4) has no transactions from a bank. Its user enters the balance once or twice a month, and the difference to the computed balance is booked to `unattributed_account`. This keeps net worth exact without importing every transaction.

- The user enters the balance at the start of a day, before that day's transactions.
- The editor computes the account's balance at that point with the core. If it matches, the editor writes only the balance assertion. If it differs, it also writes a `pad` dated the previous day, from the account to `unattributed_account`; the first snapshot of an account pads from `Equity:Opening-Balances`.
- An editor MUST NOT write a `pad` without a difference: Python Beancount and rustledger report unused pads as errors.
- Balance assertions carry `~ 0.00` like those of importers (section 8).
- Snapshots are written only for snapshot accounts. A `pad` on a source account is never written, because it would hide import gaps.

```beancount
2026-10-31 pad Assets:PaymentService Expenses:Unattributed
2026-11-01 balance Assets:PaymentService  80.00 ~ 0.00 EUR
```

## 10. Edit operations

Clients never ship text diffs; they record intents that name a transaction by ID and carry the value the user saw, so they can be re-applied safely to a newer state.

| Operation | Arguments | Precondition | Effect | Role | Release |
| --- | --- | --- | --- | --- | --- |
| `confirm` | id | Transaction is active | Flag becomes `*` | Editor | v0.1 |
| `set-account` | id, posting index, old, new | Posting has account `old` and is not the source posting; `new` is open | Account becomes `new` | Editor | v0.1 |
| `dismiss` | id | Transaction is active | Transaction is commented out in place | Editor | v0.1 |
| `open-account` | account, date | Account is not open | `<date> open <account>` appended to `accounts.beancount` | Editor, Admin | v0.1 |
| `add-import-run` | source, file, content | File does not exist | New run file written | Importer | v0.1 |
| `set-payee`, `set-narration` | id, old, new | Field equals `old` | Field becomes `new` | Editor | v1 |
| `set-meta` | id, key, old, new | Key is not `lentil_id`, `lentil_ref` or `lentil_desc`; value equals `old` | Value becomes `new` | Editor | v1 |
| `add-entry` | device file, entry | File belongs to this device | Entry appended | Editor | v1 |
| `add-snapshot` | account, date, amount | Account is a snapshot account; no balance assertion for it on that date | Balance assertion and, only if needed, a pad appended to the device's month file (section 9) | Editor | v1 |

**Applying an intent.** For each intent, in creation order:

1. Find the transaction by `lentil_id`. If it does not exist, the intent is a conflict.
2. If the current value already equals the new value, the intent is done and is not applied again.
3. If the current value equals the expected old value, apply it.
4. Otherwise the intent is a conflict.

**Rules.**

- Applying an intent changes only the bytes of the affected element: the flag character, the account name or the string. Everything else in the file stays byte-identical (PRD: principle 7).
- `dismiss` prepends ` ;  ` to every line of the transaction. It is used for duplicates, for example a transfer imported from both accounts. Commented-out IDs still count for deduplication (section 8), so a dismissed transaction is never imported again.
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
| Amounts in any currency, price annotations `@` and `@@` | Supported | Needed for foreign currencies (v1) |
| Cost annotations `{...}` | Outside | Investments come after v1 |
| Posting with elided amount | Supported | At most one per transaction |
| `balance`, `pad`, `price` | Supported | `pad` only on balance-tracked accounts (section 9); `price` from lentil prices or written by hand |
| `note`, `document`, `event`, `query`, `custom` | Supported | Passed through unchanged; `custom "budget"` is used after v1 |
| `pushtag`, `poptag` | Outside |  |
| Posting flags | Supported | Preserved, no meaning (section 6) |

The profile widens with `adopt` in v1, so existing ledgers can be taken over without rewriting them.

## 15. Import mappings

An importer reads a bank export through an adapter that turns it into statement records. Everything after that works on records only and knows nothing about the bank: IDs, duplicate checks, suggestions, balance assertions and the run file (section 8). v0 defines one adapter, CSV with a declarative mapping file. Adapters added later, for example for camt.053, MUST produce the same records.

**Statement records.**

| Field | Required | Meaning |
| --- | --- | --- |
| `booking_date` | Yes | Date the bank booked the transaction; becomes the transaction date |
| `amount` | Yes | Signed decimal, negative when money leaves the account |
| `currency` | Yes | From a column, otherwise the source's `currency` (section 4) |
| `counterparty` | No | The other party, as the bank writes it |
| `purpose` | No | Purpose or booking text, as the bank writes it |
| `reference` | No | The bank's unique reference; becomes `lentil_ref` and the ID input (section 5) |
| `balance_after` | No | Running balance after this booking |

An export may also state one balance and the date it refers to. `lentil_desc` is `counterparty` and `purpose` joined by one space and normalized as in section 5.

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
counterparty = ["Zahlungsempfänger"]   # one or more columns
purpose = ["Verwendungszweck"]         # one or more columns
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

- Mapping files are data. They MUST NOT name programs, scripts or shell commands, because any device with write access to the repository could otherwise run code on the machine that imports. A converter that runs a program, if one is added later, is configured outside the repository.
- The format is versioned with `spec_version`. A client MUST reject a mapping with an unknown key or a missing required key. New keys are added only when a real export needs them, together with a test case.
- The header row is the first row that contains every cell listed in `header`, compared after trimming. Rows before it are ignored. If no row matches, the import aborts, because the export no longer matches the mapping.
- Columns are referenced by header name only, never by position. A reordered export still imports; a renamed column aborts the import and names the column.
- The table ends at the first line matching `end`, or at the end of the file. Empty rows are ignored.
- Every other row MUST parse or match a `[[skip]]` entry. A row that does not parse aborts the import with its line number; rows are never dropped silently. The import summary counts skipped rows.
- If an export contains pending transactions, the mapping MUST skip them with a `[[skip]]` entry (section 8: booked transactions only). `equals` compares the trimmed cell exactly; `matches` takes a regular expression in the syntax of section 16.
- Dates use strftime codes (`%d`, `%m`, `%Y`, `%y`). Amounts drop `thousands`, read `decimal` as the decimal point and accept a leading `+` or `-`. With `debit` and `credit`, exactly one of the two cells holds a number: the amount is the credit, or the debit made negative, whatever sign the file gives it. `invert = true` flips the sign after that.
- `counterparty` and `purpose` join their non-empty cells with one space.
- `from = "column"`: in booking order, each row's balance MUST equal the previous row's balance plus its amount. This check also shows whether the bank lists rows oldest or newest first. If neither order fits, the importer writes no assertion and reports the mapping as suspect. Otherwise it uses the balance after the last booking before the run date.
- `from = "text"`: `pattern` is matched against the lines before the header row and from the `end` line on. The named group `amount` is required; `date` is optional and read with `date_format`. Without `date`, the balance refers to the run date.
- `from = "none"`: the CLI asks for the balance (section 8).
- Lentil may later ship ready-made mappings for common banks. Each comes with an anonymized sample export and its expected records as a test.

## 16. Suggestion rules

Explicit rules live in `rules.toml` and always win; learned suggestions are derived from the ledger's history and are never stored.

```toml
[[rule]]
source = "giro"                        # optional
desc = "(?i)corner grocery"            # regular expression on lentil_desc
account = "Expenses:Food:Groceries"
```

- A rule matches on `desc` (the bank's original description in `lentil_desc`), `payee` and/or `narration`, optionally limited to one `source`. `account` is required. `desc` is preferred, because it never changes.
- Regular expressions use RE2 syntax without lookaround, so implementations in different languages behave the same.
- Rules are evaluated in file order; the first match wins. Only if no rule matches does the importer use a learned suggestion, and only then `fallback_account`.
- Learned suggestions are computed from reviewed (`*`) transactions. Their input is `lentil_desc` and the source account, never payee or narration, which users may edit (PRD: FR-C8). They need no file of their own, so any client can recompute them.
- In v0.1 only the admin role edits `rules.toml`. In v1 editors add rules through an `add-rule` operation (PRD: FR-C11).

## 17. Bot status file (later)

Deferred until automatic bank access exists. A bot then reports its health through a small JSON file in the repository, so clients learn about errors and expiring bank access at their next sync without extra infrastructure (PRD: FR-C15).

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

Two questions are settled; six remain, each with the current proposal where there is one.

**Settled**

- [x] **Includes:** glob patterns replace index files (section 3). Python Beancount, rustledger and go-beancount resolve them identically, also on a 10-year ledger.
- [x] **Dismissing:** keep commenting out. All three parsers ignore commented-out transactions, so balances stay correct without plugins. Deduplication scans comment lines for `lentil_id`; rustledger's parse result exposes standalone comments with their source spans.

**Open**

- [ ] **Partial days:** if a bank export cuts a day in half, identical same-day transactions can get different occurrence indexes across runs (section 5). Option A: import only days before the run date, which is deterministic but delays same-day transactions by one day. Option B: accept the rare duplicate and rely on `dismiss`. Proposal: A.
- [ ] **Metadata keys:** `lentil_id`, `lentil_ref` and `lentil_desc` are long but avoid collisions with other tools. Proposal: keep.
- [ ] **Shared accounts file:** editors append `open` directives to `accounts.beancount`. This is conflict-free through `open-account`, but it is the only shared file editors write. Alternative: one accounts file per device. Proposal: keep the shared file.
- [ ] **Transfers (v1):** how a merged transfer keeps the IDs of both sides, so neither side is imported again.
- [ ] **Commit size:** one commit per sync (proposal) or one per intent.
- [ ] **Matching manual and imported entries (after v1):** match a manual entry to a later import by account, amount and a date within a few days, as beancount-import does with up to 5 days. Until then, section 9 forbids manual postings on source accounts.
