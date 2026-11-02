//! `lentil import` (PRD N2, FR-C1–C8).
//!
//! The questions (balance, possible duplicates) are asked once on the state
//! fetched first; the sync loop then re-plans on every fresh state with the
//! same answers, so a retry after a rejected push never asks again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jiff::civil::Date;
use jiff::{Timestamp, Zoned};
use lentil_core::config::{Config, SourceConfig};
use lentil_core::finding::FindingCode;
use lentil_core::import::csv::read_statement;
use lentil_core::import::plan::{BalanceOutcome, ImportPlan, ImportRequest, UserBalance};
use lentil_core::import::{Mapping, Rules, Statement, plan_import};
use lentil_core::ledger;
use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{Credentials, GitBackend, ThreadSleeper, Work, run_sync};
use lentil_core::validate::{self, Role};

use crate::context;
use crate::i18n::{self, Lang, Msg};
use crate::prompt;
use crate::{CLIENT, CliError, CliResult};

/// Command-line arguments of `lentil import`.
#[derive(Debug)]
pub struct Args {
    /// Source ID.
    pub source: String,
    /// The CSV export.
    pub file: PathBuf,
    /// Balance from online banking, as typed.
    pub balance: Option<String>,
    /// Date of that balance.
    pub balance_date: Option<Date>,
    /// Never ask.
    pub yes: bool,
    /// Overrides today.
    pub run_date: Option<Date>,
}

/// Everything read from the repository and the export that stays fixed for a run.
struct Inputs {
    config: Config,
    source: SourceConfig,
    rules: Rules,
    statement: Statement,
}

/// The user's answers, reused on every attempt.
struct Answers {
    balance: UserBalance,
    accepted: BTreeSet<String>,
}

pub fn run(lang: Lang, repo: Option<&Path>, args: &Args) -> CliResult {
    let root = context::find_root(lang, repo)?;
    let git_err = |e| CliError(i18n::git_error(lang, &e));
    let mut git = Git2Backend::open(&root, Credentials::Default).map_err(git_err)?;
    git.ensure_no_local_changes().map_err(git_err)?;
    let export = std::fs::read(&args.file).map_err(|e| {
        CliError(i18n::msg(
            lang,
            Msg::ExportUnreadable,
            &[("detail", &e.to_string())],
        ))
    })?;
    let started = Timestamp::now();
    let run_date = args.run_date.unwrap_or_else(|| Zoned::now().date());

    let head = git.fetch().map_err(git_err)?;
    let current = git.snapshot(head.as_ref()).map_err(git_err)?;
    let reasons = validate::read_only_reasons(&current);
    if !reasons.is_empty() {
        eprintln!("{}", i18n::text(lang, Msg::ReadOnly));
        context::print_findings(lang, &reasons);
        return Ok(ExitCode::FAILURE);
    }
    let inputs = Inputs::read(lang, &current, &args.source, &export)?;

    let mut answers = Answers {
        balance: initial_balance(lang, args, run_date)?,
        accepted: BTreeSet::new(),
    };
    let mut plan = plan(&inputs, &answers, &current, started, run_date);
    // Ask for the balance only if the run brings something new; a repeated
    // import of the same export asks nothing.
    let anything_new = !plan.transactions.is_empty() || !plan.possible_duplicates.is_empty();
    if plan.balance == BalanceOutcome::NeedsInput {
        if anything_new {
            println!("{}", i18n::text(lang, Msg::BalanceNeedsInput));
        }
        answers.balance = if args.yes || !prompt::interactive() || !anything_new {
            UserBalance::Skipped
        } else {
            ask_balance(lang, run_date)
        };
        plan = self::plan(&inputs, &answers, &current, started, run_date);
    }
    // Possible duplicates are always shown; only an interactive run may
    // import them after confirmation (spec section 8).
    let ask = !args.yes && prompt::interactive();
    if !plan.possible_duplicates.is_empty() {
        for dup in &plan.possible_duplicates {
            println!("{}", i18n::text(lang, Msg::DuplicateHeader));
            let amount = format!("{} {}", dup.amount, dup.currency);
            let date = dup.date.to_string();
            println!(
                "{}",
                i18n::msg(
                    lang,
                    Msg::DuplicateExisting,
                    &[
                        ("date", &date),
                        ("amount", &amount),
                        ("desc", dup.existing_desc.as_deref().unwrap_or("")),
                        ("file", &dup.existing_file),
                        ("line", &dup.existing_line.to_string()),
                    ],
                )
            );
            println!(
                "{}",
                i18n::msg(
                    lang,
                    Msg::DuplicateNew,
                    &[
                        ("date", &date),
                        ("amount", &amount),
                        ("desc", &dup.desc),
                        ("line", &dup.line.to_string()),
                    ],
                )
            );
            if ask
                && prompt::ask(i18n::text(lang, Msg::PromptImportAnyway))
                    .is_some_and(|a| prompt::is_yes(&a))
            {
                answers.accepted.insert(dup.new_id.clone());
            }
        }
        plan = self::plan(&inputs, &answers, &current, started, run_date);
    }
    print_plan(lang, &inputs.source.id, &plan);
    if plan.run_file.is_none() {
        println!("{}", i18n::text(lang, Msg::ImportNothingNew));
        return Ok(ExitCode::SUCCESS);
    }

    let author = context::author(lang, &git)?;
    let identity = context::identity(Role::Importer)?;
    let source_id = inputs.source.id.clone();
    let outcome = run_sync(&mut git, &mut ThreadSleeper, &identity, &author, |fresh| {
        let plan = self::plan(&inputs, &answers, fresh, started, run_date);
        match plan.run_file.clone() {
            None => Work::Nothing(plan),
            Some(run) => {
                let mut after = fresh.clone();
                after.insert(run.path.clone(), run.content.clone());
                let stamp = Path::new(&run.path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let count = plan.transactions.len() + usize::from(plan.opening.is_some());
                Work::Commit {
                    after,
                    subject: format!("{source_id} run {stamp}, {count} transactions"),
                    result: plan,
                }
            }
        }
    })
    .map_err(|e| context::sync_error(lang, e))?;

    match &outcome.result.run_file {
        Some(run) if outcome.commit.is_some() => {
            println!(
                "{}",
                i18n::msg(lang, Msg::ImportCommitted, &[("file", &run.path)])
            );
        }
        _ => println!("{}", i18n::text(lang, Msg::ImportNothingNew)),
    }
    context::print_findings(lang, &outcome.warnings);
    let failed = outcome
        .warnings
        .iter()
        .any(|w| w.code == FindingCode::BalanceAssertionFailed);
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

impl Inputs {
    fn read(
        lang: Lang,
        snapshot: &Snapshot,
        source_id: &str,
        export: &[u8],
    ) -> Result<Self, CliError> {
        let config = context::config(lang, snapshot)?;
        let source = config.source(source_id).cloned().ok_or_else(|| {
            CliError(i18n::msg(
                lang,
                Msg::UnknownSource,
                &[("source", source_id)],
            ))
        })?;
        let mapping_path = source.mapping.clone().unwrap_or_default();
        let mapping = Mapping::load(snapshot, &mapping_path)
            .ok_or_else(|| {
                CliError(i18n::msg(
                    lang,
                    Msg::MappingMissing,
                    &[("path", &mapping_path)],
                ))
            })?
            .map_err(|e| {
                CliError(i18n::msg(
                    lang,
                    Msg::MappingInvalid,
                    &[
                        ("path", &mapping_path),
                        ("detail", &i18n::mapping_error(lang, &e)),
                    ],
                ))
            })?;
        let rules = context::rules(lang, snapshot, &config)?;
        let statement = read_statement(export, &mapping, &source.currency).map_err(|e| {
            CliError(i18n::msg(
                lang,
                Msg::ExportUnreadable,
                &[("detail", &i18n::import_error(lang, &e))],
            ))
        })?;
        Ok(Self {
            config,
            source,
            rules,
            statement,
        })
    }
}

fn plan(
    inputs: &Inputs,
    answers: &Answers,
    snapshot: &Snapshot,
    started: Timestamp,
    run_date: Date,
) -> ImportPlan {
    let ledger = ledger::load(snapshot, &inputs.config.main_file);
    plan_import(&ImportRequest {
        config: &inputs.config,
        source: &inputs.source,
        rules: &inputs.rules,
        ledger: &ledger,
        statement: &inputs.statement,
        started,
        run_date,
        balance: answers.balance,
        accepted_duplicates: &answers.accepted,
        client: CLIENT,
    })
}

fn initial_balance(lang: Lang, args: &Args, run_date: Date) -> Result<UserBalance, CliError> {
    match &args.balance {
        Some(text) => {
            let amount = prompt::parse_amount(text)
                .ok_or_else(|| CliError(i18n::msg(lang, Msg::InvalidAmount, &[("value", text)])))?;
            Ok(UserBalance::Given {
                amount,
                date: args.balance_date.unwrap_or(run_date),
            })
        }
        None => Ok(UserBalance::NotAsked),
    }
}

fn ask_balance(lang: Lang, run_date: Date) -> UserBalance {
    loop {
        let Some(answer) = prompt::ask(i18n::text(lang, Msg::PromptBalance)) else {
            return UserBalance::Skipped;
        };
        if answer.is_empty() {
            return UserBalance::Skipped;
        }
        let Some(amount) = prompt::parse_amount(&answer) else {
            eprintln!(
                "{}",
                i18n::msg(lang, Msg::InvalidAmount, &[("value", &answer)])
            );
            continue;
        };
        let default = run_date.to_string();
        loop {
            let question = i18n::msg(lang, Msg::PromptBalanceDate, &[("date", &default)]);
            let answer = prompt::ask(&question).unwrap_or_default();
            if answer.is_empty() {
                return UserBalance::Given {
                    amount,
                    date: run_date,
                };
            }
            match prompt::parse_date(&answer) {
                Some(date) => return UserBalance::Given { amount, date },
                None => eprintln!(
                    "{}",
                    i18n::msg(lang, Msg::InvalidDate, &[("value", &answer)])
                ),
            }
        }
    }
}

fn print_plan(lang: Lang, source: &str, plan: &ImportPlan) {
    let count = |n: usize| n.to_string();
    println!(
        "{}",
        i18n::msg(
            lang,
            Msg::ImportSummary,
            &[
                ("count", &count(plan.transactions.len())),
                ("source", source)
            ],
        )
    );
    for (n, m) in [
        (plan.skipped_existing, Msg::ImportSkippedExisting),
        (plan.skipped_run_date, Msg::ImportSkippedRunDate),
        (plan.skipped_rows, Msg::ImportSkippedRows),
        (plan.possible_duplicates.len(), Msg::DuplicateSkipped),
    ] {
        if n > 0 {
            println!("{}", i18n::msg(lang, m, &[("count", &count(n))]));
        }
    }
    if let Some(opening) = &plan.opening {
        let amount = format!("{} {}", opening.amount, opening.currency);
        println!(
            "{}",
            i18n::msg(lang, Msg::ImportOpening, &[("amount", &amount)])
        );
    }
    match &plan.balance {
        BalanceOutcome::Assertion {
            date,
            amount,
            currency,
        } => {
            let amount = format!("{amount} {currency}");
            println!(
                "{}",
                i18n::msg(
                    lang,
                    Msg::BalanceAssertion,
                    &[("amount", &amount), ("date", &date.to_string())],
                )
            );
        }
        BalanceOutcome::NeedsInput | BalanceOutcome::Unchecked => {
            println!("{}", i18n::text(lang, Msg::BalanceUnchecked));
        }
        BalanceOutcome::Suspect => println!("{}", i18n::text(lang, Msg::BalanceSuspect)),
        BalanceOutcome::AlreadyExists { date, difference } => match difference {
            None => println!(
                "{}",
                i18n::msg(lang, Msg::BalanceExists, &[("date", &date.to_string())])
            ),
            Some(diff) => eprintln!(
                "{}",
                i18n::msg(
                    lang,
                    Msg::BalanceExistsDiffers,
                    &[
                        ("date", &date.to_string()),
                        ("difference", &diff.to_string())
                    ],
                )
            ),
        },
    }
}
