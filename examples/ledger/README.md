# Example Ledger

This repository is a [Beancount](https://github.com/beancount/beancount) ledger kept with Lentil. Every file is plain Beancount or TOML, so any Beancount tool can read it, and several Lentil clients can write to it at the same time because each of them follows the Lentil Convention Spec.

## Layout

Every file has exactly one kind of writer, and fixed include patterns in `main.beancount` pick up new files.

| Path | Content | Written by |
| --- | --- | --- |
| `lentil.toml` | Configuration: sources and default accounts | Admin |
| `main.beancount` | Entry point: two options and five includes; never change the includes | Admin |
| `fava.beancount` | Fava options and sidebar links | Admin |
| `accounts.beancount` | `open`, `close` and `commodity` directives | Admin; editors append `open` directives |
| `rules.toml` | Suggestion rules for imports | Admin |
| `sources/<source-id>.toml` | CSV mapping of one source | Admin |
| `imports/<source-id>/<YYYY>/<YYYYMMDDTHHMMSSZ>.beancount` | One import run | Importer |
| `manual/<device-id>/<YYYY-MM>.beancount` | Manual entries of one device | Editor |
| `prices/<YYYY>/<YYYYMMDD>.beancount` | Exchange rates | Importer, admin |

- Directory and file names use lowercase letters, digits and hyphens, apart from the run timestamps and the fixed names above.
- `.beancount` files anywhere else are not part of the ledger and are rejected.
- The `keep` files keep the include patterns matching while a directory has no other file. Leave them in place and empty.
- Never commit the load caches `.*.cache` and `.*.picklecache`; `.gitignore` keeps them out.

## Review state

Review state lives only in the transaction flag.

- `!` means open: importers write every transaction with `!`.
- `*` means reviewed: confirming or correcting a transaction sets `*`, and manual entries start as `*`.
- A duplicate is dismissed by commenting it out in place. Its `lentil_id` still counts, so it is never imported again.

## Roles

Every change belongs to one role, and validation rejects changes outside that role's area.

- **Importer** (`lentil import`): only creates new run files under `imports/<source-id>/` and never changes existing files.
- **Editor** (the Android app): changes the flag, payee, narration, tags, links, metadata and the counter account of existing transactions, comments out duplicates, appends `open` directives to `accounts.beancount` and writes only its own `manual/<device-id>/` files.
- **Admin** (`lentil init`, a text editor followed by `lentil check`): may change anything, as long as the result still follows the convention.

## Protected metadata

Lentil addresses transactions by their metadata, so never change or remove these keys:

- `lentil_id`: the stable ID of a transaction.
- `lentil_ref`: the bank's own reference.
- `lentil_desc`: the bank's original description, from which suggestions learn.

Rename payees and narrations freely instead; `lentil_desc` keeps the bank's wording.

## Adding a source

A source is one bank account or card whose CSV exports you import.

1. Open its account in `accounts.beancount`, for example `2026-01-01 open Assets:Bank:Giro EUR`.
2. Add it to `lentil.toml`:

   ```toml
   [[source]]
   id = "giro"
   account = "Assets:Bank:Giro"
   currency = "EUR"
   importer = "csv"
   mapping = "sources/giro.toml"
   ```

3. Describe the bank's CSV export in `sources/giro.toml`. Columns are found by their header names:

   ```toml
   [file]
   encoding = "utf-8"
   delimiter = ";"
   header = ["Booking date", "Amount"]

   [numbers]
   decimal = ","
   thousands = "."

   [columns]
   booking_date = { column = "Booking date", format = "%d.%m.%Y" }
   amount = { column = "Amount" }
   counterparty = ["Payee"]
   purpose = ["Purpose"]

   [[skip]]
   column = "Status"
   equals = "Pending"

   [balance]
   from = "column"
   column = "Balance"
   ```

4. Run `lentil import` with the source ID and the export file. Every new transaction arrives with flag `!` and a suggested counter account; transactions imported before are skipped. If the export holds no balance, `lentil import` asks for the balance shown in online banking, and `lentil status` reports the run as unchecked if you skip it. The first run with a balance check also books the account's opening balance against `Equity:OpeningBalances`, with flag `!` so you can check it.

## Transfers between your own accounts

A transfer between two imported accounts, such as paying the card bill from the giro account, arrives from both sides.

- Keep both transactions and give each the counter account `Assets:Transfer`. Each side keeps its bank's date, so every balance check holds, and `Assets:Transfer` holds the money while it is on its way.
- A balance that stays on `Assets:Transfer` points to a transfer whose other side is missing.
- A rule in `rules.toml` can suggest `Assets:Transfer` for such bookings. An imported account is never a counter account.

## Viewing and checking

- `fava main.beancount` shows the ledger. The sidebar links to the open transactions and to `Expenses:Uncategorized`.
- `lentil status` lists open transactions, failing balance assertions and import runs without a balance check.
- `lentil check` validates the repository after a change in a text editor; `bean-check main.beancount` runs the reference implementation.
