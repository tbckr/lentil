# lentil

Lentil makes [Beancount](https://github.com/beancount/beancount/) practical for everyday use: you import your bank's CSV exports, every transaction arrives with a suggested account, and you review it with one tap on your phone. Your ledger stays a Git repository of standard Beancount files that you own.

> [!WARNING]
> Lentil is an early, unreleased draft of v0.1. The convention its clients follow is at `spec_version` 0 and may still change incompatibly until version 1, so try Lentil on a test ledger first.

## Why Lentil

Lentil cuts the work per transaction down to one confirmation, without taking your data out of your hands.

- **Review means confirming.** Every imported transaction arrives open, with flag `!` and a suggested counter account from your rules or from transactions you reviewed before. Confirming sets `*`. The goal is under 15 minutes of review per month.
- **No server, no lock-in.** The single source of truth is a Git repository on any remote you choose. Everything Lentil writes is standard Beancount, which Fava, `bean-check` and your text editor read as well.
- **Exact balances.** Every import run checks the bank's balance with zero tolerance. A gap is reported, never hidden or corrected automatically.
- **Your files stay as they are.** A change touches only the affected characters, such as a flag or an account name. Lentil never reformats a file and keeps your comments.
- **Several devices, no merge conflicts.** Each client writes only to its own area of the repository and re-applies its changes to the latest state, so the history stays linear. A real conflict is shown to you, never dropped.
- **Private by default.** No telemetry. Lentil talks only to your Git remote, and from v1 on to the ECB for exchange rates.

## Who it is for

Lentil is built for technically skilled people who want to own their financial data and are comfortable with a Git remote.

- **Primary:** you run or choose a Git remote and want little everyday effort, reliable numbers and no lock-in.
- **Existing Beancount users:** served on a best-effort basis. Adopting an existing ledger (`lentil adopt`) comes with v1. A ledger that uses anything outside the [supported profile](docs/SPEC.md#14-supported-profile-v0) is shown read-only instead of being computed approximately.
- **Not for:** people who cannot or will not use a Git remote. `lentil init` sets up the repository, but Git does not disappear entirely. Several people or households sharing one ledger are not supported in v1.

## How it works

Three clients share one Git repository, and each of them does one job.

| Client | Runs on | What you do |
| --- | --- | --- |
| `lentil` CLI | Linux, macOS | Set up the repository, import CSV exports, list what is still open |
| Android app | Your phone | Confirm or correct open transactions, also offline |
| [Fava](https://github.com/beancount/fava) or [rustfava](https://github.com/rustledger/rustfava) | Your browser | Read the reports: income and expenses per category, net worth over time |

- Every client computes with the same core, built on [rustledger](https://github.com/rustledger/rustledger), so balances are identical everywhere.
- A declarative mapping per bank account describes its CSV export. A changed export format aborts the import instead of being guessed.
- Importing the same export twice adds nothing: transactions carry a stable ID, and known ones are skipped.

## Status

The v0.1 draft covers the core loop of import and mobile triage; the features for everyday use follow in v1.

| Release | Content |
| --- | --- |
| v0.1 (draft) | `lentil init`, `import`, `status` and `check`; the Android app confirms transactions, changes the counter account, opens a missing account and dismisses duplicates; Fava as the read-only view |
| v1 | Cash and one-off entries on Android, a balance overview, month-end close, tax export, foreign currencies, conflict resolution in the app, adopting an existing ledger, drafting a CSV mapping from a sample export |
| Later | Automatic bank access via FinTS, an own web UI, a desktop app |

The CLI and the app speak English and German; the CLI follows `LANG`.

## Install

The flake packages the `lentil` CLI for Linux (x86_64 and arm64) and macOS (arm64); there are no release binaries yet.

- Install it with `nix profile install github:tbckr/lentil`, or run it without installing with `nix run github:tbckr/lentil -- <command>`.
- On NixOS, add the flake as an input of the system configuration and its package to the system:

  ```nix
  {
    inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    inputs.lentil.url = "github:tbckr/lentil";

    outputs =
      { nixpkgs, lentil, ... }:
      {
        nixosConfigurations.example = nixpkgs.lib.nixosSystem {
          modules = [
            ./configuration.nix
            (
              { pkgs, ... }:
              {
                environment.systemPackages = [ lentil.packages.${pkgs.stdenv.hostPlatform.system}.default ];
              }
            )
          ];
        };
      };
  }
  ```

- With the overlay, the module sets `nixpkgs.overlays = [ lentil.overlays.default ];` and adds `pkgs.lentil` instead.
- For the desktop view, install [Fava](https://github.com/beancount/fava) or [rustfava](https://github.com/rustledger/rustfava) as their projects describe.

## Quick start

This walkthrough creates a ledger, adds one bank account and imports its CSV export.

1. Create the repository. `--remote` uploads it to an empty Git repository, which the Android app needs; without it, the repository stays local.

   ```sh
   lentil init ledger --title "My Ledger" --currency EUR --remote https://git.example.com/me/ledger.git
   cd ledger
   ```

   The CLI signs in to the remote with your SSH agent or Git's credential helper.

2. Add the bank account as a source. Open its account in `accounts.beancount`, together with the income account for the rule below:

   ```beancount
   2026-01-01 open Assets:Bank:Checking EUR
   2026-01-01 open Income:Salary
   ```

   `lentil init` opened `Equity:OpeningBalances`, `Assets:Transfer`, `Expenses:Uncategorized` and `Expenses:Unattributed` already. Leave them out when you copy an existing chart of accounts, because opening an account twice is an error.

   Add the source to `lentil.toml`:

   ```toml
   [[source]]
   id = "checking"
   account = "Assets:Bank:Checking"
   currency = "EUR"
   importer = "csv"
   mapping = "sources/checking.toml"
   ```

   Describe the export in `sources/checking.toml`. This one has the columns `Date,Payee,Purpose,Amount,Balance`; [Spec section 15](docs/SPEC.md#15-import-mappings) lists every option, such as encodings, decimal commas, rows to skip and balances in a footer line:

   ```toml
   [file]
   header = ["Date", "Amount"]

   [columns]
   booking_date = { column = "Date", format = "%Y-%m-%d" }
   amount = { column = "Amount" }
   counterparty = ["Payee"]
   purpose = ["Purpose"]

   [balance]
   from = "column"
   column = "Balance"
   ```

   Optionally, add a rule to `rules.toml`. A rule matches the bank's counterparty, purpose or booking type and sets the account, the payee or both. Without a rule for the account, Lentil learns from the transactions you have reviewed, and only then falls back to `Expenses:Uncategorized`:

   ```toml
   [[rule]]
   match = { purpose = "(?i)^salary" }
   account = "Income:Salary"
   ```

   Commit the changes and push them; leave out the push without a remote. `lentil import` only runs when the local copy has nothing left to upload.

   ```sh
   git add -A
   git commit -m "Add the checking account"
   git push origin main
   ```

3. Import an export. `lentil import` writes a new file, commits it and, with a remote, pushes it:

   ```console
   $ lentil import checking ~/Downloads/checking.csv
   2 new transactions from checking.
   Balance check: 1476.50 EUR at the start of 2026-10-03.
   Saved as imports/checking/2026/20261031T214315Z.beancount.
   ```

   The new file is plain Beancount. Each transaction is open and carries a suggested account, and the run ends with the bank's balance as an assertion:

   ```beancount
   2026-10-01 ! "Example Employer" "Salary October"
     lentil_id: "checking-87c56a8da0fc"
     lentil_counterparty: "Example Employer"
     lentil_purpose: "Salary October"
     Assets:Bank:Checking   1500.00 EUR
     Income:Salary

   2026-10-02 ! "Corner Grocery" "Card payment"
     lentil_id: "checking-f035a3a427ef"
     lentil_counterparty: "Corner Grocery"
     lentil_purpose: "Card payment"
     Assets:Bank:Checking    -23.50 EUR
     Expenses:Uncategorized

   2026-10-03 balance Assets:Bank:Checking  1476.50 ~ 0.00 EUR
   ```

   If an export holds no balance, `lentil import` asks for the balance shown in online banking.

4. Review the open transactions in the Android app, or in a text editor by setting their flag to `*`. `lentil status` lists what is still open: unreviewed transactions, failing balance assertions and imports without a balance check. `fava main.beancount` shows the ledger, with sidebar links to the open transactions and the uncategorized expenses.

After a change in a text editor, `lentil check` validates the repository, and `lentil check --reference` also compares it with Python Beancount's `bean-check`. The README that `lentil init` writes into the repository explains its layout and roles and how to add further sources.

## Example

[`examples/ledger`](examples/ledger) is a fictional ledger after one import and a review, generated from the current code, so it shows exactly what Lentil writes.

- [`imports/checking/2026/20261101T080000Z.beancount`](examples/ledger/imports/checking/2026/20261101T080000Z.beancount) is the import run: the opening balance, reviewed transactions with `*`, open ones with `!` and the balance assertion.
- The review left an account change behind, and the `open` directive it needed at the end of `accounts.beancount`.
- [`examples/checking.csv`](examples/checking.csv) is the bank export it came from, and `sources/checking.toml` its mapping.
- `fava examples/ledger/main.beancount` shows it in Fava, as does `just fava` in the development shell. `lentil status` and `lentil check` work in the directory too. `lentil import` does not, because the ledger is not a Git repository of its own.

## Android app

The app is the triage client: it shows a queue of open transactions, newest first, and one tap confirms one.

- It changes a counter account through search and recently used accounts, and opens a missing account directly.
- It dismisses a duplicate, for example a transaction imported again after the bank changed its wording. The transaction stays in its file as a comment and is never imported again.
- It works offline, keeps your changes on the device and syncs them later, without Git terms in its UI.
- It needs Android 13 or newer on an arm64 device, and a remote it can reach over HTTPS with a user name and an access token. The token stays in the device's keystore.

There is no APK yet; F-Droid is the target. Until then, build a debug APK from source in the flake's development shell and install it with `adb`:

```sh
nix develop
cd android
./gradlew :app:assembleDebug
adb install app/build/outputs/apk/debug/app-debug.apk
```

## Non-goals

Feature requests are filtered through these non-goals; the reasons are in the [PRD](docs/PRD.md#non-goals-for-v1).

- Budgets; investments with lots and cost basis; receipts from a document archive (after v1)
- Matching manual entries with later imports; third-party plugins and importers (after v1)
- Own web UI, desktop app and automatic bank access via FinTS (later)
- iOS; several people or households; end-to-end encrypted remote; commercial bank aggregators (open)
- Hosted service (never)

## Documentation

The design documents describe Lentil in full.

- [Product requirements](docs/PRD.md): principles, goals, requirements and release plan.
- [Convention spec](docs/SPEC.md): the rules every client follows, including the format of CSV mappings and suggestion rules.
- [Contributing](CONTRIBUTING.md): engineering standards for development.

## License

GPL-3.0-only, see [LICENSE](LICENSE).
