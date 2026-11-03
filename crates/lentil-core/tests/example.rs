//! The example ledger in `examples/ledger/`: a fictional repository after one
//! import and a review, so readers see exactly what Lentil writes.
//!
//! This file generates it from the core in the three steps a user goes
//! through: the admin adds a source to the files of `init`, the importer
//! writes one run from `examples/checking.csv`, and an editor reviews part of
//! it. The tests check each step and fail when the files in `examples/ledger/`
//! differ from what the core writes now. Set `LENTIL_UPDATE_EXAMPLE` (or run
//! `just example`) to rewrite them instead.

// Test code may panic on unexpected input, like the unit tests (clippy.toml).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use jiff::Timestamp;
use jiff::civil::{Date, date};
use lentil_core::config::{CONFIG_FILE, Config};
use lentil_core::import::{
    BalanceOutcome, ImportPlan, ImportRequest, Mapping, Rules, SuggestionKind, UserBalance,
    opening_id, plan_import, read_statement,
};
use lentil_core::intent::{Intent, Outcome, PendingIntent, ReplayResult, replay};
use lentil_core::layout::{InitOptions, init_files};
use lentil_core::ledger;
use lentil_core::snapshot::Snapshot;
use lentil_core::status;
use lentil_core::tags::{TAGS_FILE, tag_warnings};
use lentil_core::validate::{Context, Role, validate};

const UPDATE_VAR: &str = "LENTIL_UPDATE_EXAMPLE";
const SKIP_VAR: &str = "LENTIL_SKIP_DIFFERENTIAL";

/// The bank export of the import run.
const EXPORT: &[u8] = include_bytes!("../../../examples/checking.csv");
/// The CLI's `Lentil-Client` value, which the run header carries; both crates
/// share the workspace version.
const CLIENT: &str = concat!("lentil-cli/", env!("CARGO_PKG_VERSION"));
/// The reviewing editor's device.
const DEVICE: &str = "k7p2q9xd";

const SOURCE: &str = r#"[[source]]
id = "checking"
account = "Assets:Bank:Checking"
currency = "EUR"
importer = "csv"
mapping = "sources/checking.toml"
"#;

const ACCOUNTS: &str = "2026-01-01 open Assets:Bank:Checking EUR
2026-01-01 open Income:Salary
2026-01-01 open Expenses:Housing:Rent
";

const RULES: &str = r#"[[rule]]
match = { purpose = "(?i)^salary" }
account = "Income:Salary"

[[rule]]
match = { counterparty = "(?i)^example housing co-op$" }
payee = "Housing Co-op"
account = "Expenses:Housing:Rent"
add_tags = ["recurring"]

[[rule]]
match = { counterparty = "(?i)^city utilities$" }
add_tags = ["recurring"]
"#;

const TAGS: &str = "tags = [\"recurring\", \"irregular\", \"friends\"]\nfamilies = [\"trip-\"]\nexclusive = [[\"recurring\", \"irregular\"]]\n";

const MAPPING_PATH: &str = "sources/checking.toml";
const MAPPING: &str = r#"# Fictional checking account export: UTF-8, oldest first, a running balance per row.

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
"#;

const RUN_FILE: &str = "imports/checking/2026/20261101T080000Z.beancount";
const UNCATEGORIZED: &str = "Expenses:Uncategorized";
const GROCERIES: &str = "Expenses:Food:Groceries";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example_dir() -> PathBuf {
    workspace_root().join("examples/ledger")
}

fn config(files: &Snapshot) -> Config {
    Config::parse(files.text(CONFIG_FILE).unwrap()).unwrap()
}

/// Appends `text` to a file of `init`, after one blank line.
fn append(files: &mut Snapshot, path: &str, text: &str) {
    let existing = files.text(path).unwrap().to_owned();
    files.insert(path, format!("{existing}\n{text}"));
}

/// The files of `init` plus the source, its accounts, rules, mapping and tag
/// taxonomy.
fn admin() -> Snapshot {
    let mut files = init_files(&InitOptions {
        title: "Example Ledger".into(),
        currency: "EUR".into(),
    });
    append(&mut files, CONFIG_FILE, SOURCE);
    append(&mut files, "accounts.beancount", ACCOUNTS);
    append(&mut files, "rules.toml", RULES);
    files.insert(MAPPING_PATH, MAPPING);
    files.insert(TAGS_FILE, TAGS);
    files
}

/// The import run of `examples/checking.csv`, added to `admin`.
fn import_run(admin: &Snapshot) -> (Snapshot, ImportPlan) {
    let config = config(admin);
    let source = config.source("checking").unwrap();
    let mapping = Mapping::parse(admin.text(MAPPING_PATH).unwrap()).unwrap();
    let rules = Rules::load(admin, &config).unwrap();
    let statement = read_statement(EXPORT, &mapping, &source.currency).unwrap();
    let ledger = ledger::load(admin, &config.main_file);
    let plan = plan_import(&ImportRequest {
        config: &config,
        source,
        rules: &rules,
        ledger: &ledger,
        statement: &statement,
        started: "2026-11-01T08:00:00Z".parse::<Timestamp>().unwrap(),
        run_date: date(2026, 11, 1),
        balance: UserBalance::NotAsked,
        accepted_duplicates: &BTreeSet::new(),
        client: CLIENT,
    });
    let run = plan.run_file.clone().expect("the run writes a file");
    let mut files = admin.clone();
    files.insert(run.path, run.content);
    (files, plan)
}

/// The `lentil_id` of the one planned transaction booked on `day`.
fn id_on(plan: &ImportPlan, day: Date) -> String {
    let ids: Vec<&str> = plan
        .transactions
        .iter()
        .filter(|t| t.date == day)
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids.len(), 1, "one transaction on {day}: {ids:?}");
    ids[0].to_owned()
}

/// The review: confirm what a rule suggested, move two grocery payments to a
/// new account, tag one of them as shared with friends, leave the rest open.
/// As in the app, each account change is followed by a separate `confirm`,
/// because any edit reviews the transaction.
fn intents(plan: &ImportPlan) -> Vec<PendingIntent> {
    let grocery = |day| Intent::SetAccount {
        id: id_on(plan, day),
        posting: 1,
        old: UNCATEGORIZED.into(),
        new: GROCERIES.into(),
    };
    let confirm = |id: String| Intent::Confirm { id };
    [
        confirm(opening_id("checking")),
        confirm(id_on(plan, date(2026, 10, 1))),
        confirm(id_on(plan, date(2026, 10, 15))),
        Intent::OpenAccount {
            account: GROCERIES.into(),
            date: date(2026, 10, 1),
        },
        grocery(date(2026, 10, 2)),
        confirm(id_on(plan, date(2026, 10, 2))),
        grocery(date(2026, 10, 9)),
        Intent::AddTag {
            id: id_on(plan, date(2026, 10, 9)),
            tag: "friends".into(),
        },
        confirm(id_on(plan, date(2026, 10, 9))),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, intent)| PendingIntent {
        key: (i + 1).to_string(),
        intent,
    })
    .collect()
}

fn review(imported: &Snapshot, intents: &[PendingIntent]) -> ReplayResult {
    replay(imported, &config(imported), intents)
}

/// The example ledger as the core writes it now.
fn example() -> Snapshot {
    let (imported, plan) = import_run(&admin());
    review(&imported, &intents(&plan)).snapshot
}

#[test]
fn import_writes_one_run_with_opening_and_assertion() {
    let (_, plan) = import_run(&admin());
    assert_eq!(plan.transactions.len(), 7);
    assert_eq!(
        plan.opening.as_ref().unwrap().amount,
        "1204.35".parse().unwrap()
    );
    assert_eq!(
        plan.balance,
        BalanceOutcome::Assertion {
            date: date(2026, 11, 1),
            amount: "2534.00".parse().unwrap(),
            currency: "EUR".into(),
        }
    );
    let suggestions: Vec<SuggestionKind> = plan.transactions.iter().map(|t| t.suggestion).collect();
    assert_eq!(
        suggestions,
        [
            SuggestionKind::Rule,
            SuggestionKind::Fallback,
            SuggestionKind::Fallback,
            SuggestionKind::Fallback,
            SuggestionKind::Rule,
            SuggestionKind::Fallback,
            SuggestionKind::Fallback,
        ]
    );
    assert_eq!(plan.run_file.unwrap().path, RUN_FILE);
}

#[test]
fn every_review_intent_applies() {
    let (imported, plan) = import_run(&admin());
    let result = review(&imported, &intents(&plan));
    assert!(
        result.outcomes.iter().all(|(_, o)| *o == Outcome::Applied),
        "{:?}",
        result.outcomes
    );
}

#[test]
fn each_step_is_valid_for_its_role() {
    let admin = admin();
    let (imported, plan) = import_run(&admin);
    let reviewed = review(&imported, &intents(&plan)).snapshot;
    let steps = [
        (&admin, Role::Admin, None, None),
        (&imported, Role::Importer, None, Some(&admin)),
        (&reviewed, Role::Editor, Some(DEVICE), Some(&imported)),
    ];
    for (after, role, device, before) in steps {
        let validation = validate(
            after,
            &Context {
                role,
                device,
                before,
            },
        );
        assert!(
            !validation.is_blocking(),
            "{role:?}: {:?}",
            validation.findings
        );
    }
    let last = validate(
        &reviewed,
        &Context {
            role: Role::Editor,
            device: Some(DEVICE),
            before: Some(&imported),
        },
    );
    assert!(last.findings.is_empty(), "{:?}", last.findings);
}

#[test]
fn example_has_three_open_transactions_and_nothing_else_to_report() {
    let example = example();
    let config = config(&example);
    let ledger = ledger::load(&example, &config.main_file);
    let report = status::status(&config, &ledger, &example);
    assert_eq!(report.open.len(), 3, "{:?}", report.open);
    assert!(report.failed_assertions.is_empty());
    assert!(report.unchecked_runs.is_empty());
    let flags: Vec<char> = ledger.transactions.iter().map(|t| t.flag).collect();
    assert_eq!(flags.iter().filter(|&&f| f == '*').count(), 5, "{flags:?}");
    assert_eq!(flags.iter().filter(|&&f| f == '!').count(), 3, "{flags:?}");
}

#[test]
fn example_tags_follow_the_taxonomy() {
    let example = example();
    let config = config(&example);
    let ledger = ledger::load(&example, &config.main_file);
    assert_eq!(tag_warnings(&example, &config, &ledger), Vec::new());
    let run = example.text(RUN_FILE).unwrap();
    for header in [
        "2026-10-01 * \"Housing Co-op\" \"Rent October\" #recurring\n",
        "2026-10-06 ! \"City Utilities\" \"Electricity instalment 10/2026\" #recurring\n",
        "2026-10-09 * \"Corner Grocery\" \"Card payment\" #friends\n",
    ] {
        assert!(run.contains(header), "{header}{run}");
    }
}

#[test]
fn example_passes_bean_check() {
    if std::env::var_os(SKIP_VAR).is_some() {
        eprintln!("bean-check skipped: {SKIP_VAR} is set");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in example().files() {
        let target = dir.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let output = Command::new("uv")
        .args(["run", "--frozen", "--project"])
        .arg(workspace_root())
        .args(["bean-check", "--no-cache", "main.beancount"])
        .current_dir(dir.path())
        .output()
        .expect("uv must be on PATH (set LENTIL_SKIP_DIFFERENTIAL to skip)");
    assert!(
        output.status.success(),
        "bean-check failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn example_is_up_to_date() {
    let example = example();
    let dir = example_dir();
    let on_disk = Snapshot::from_dir(&dir).unwrap_or_default();
    if std::env::var_os(UPDATE_VAR).is_some() {
        for (path, content) in example.files() {
            let target = dir.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, content).unwrap();
        }
        for stale in on_disk.paths().filter(|p| !example.contains(p)) {
            std::fs::remove_file(dir.join(stale)).unwrap();
        }
        return;
    }
    assert!(
        on_disk == example,
        "examples/ledger differs from what the core writes: {:?}; run `just example` to regenerate it",
        example.changes_from(&on_disk)
    );
}
