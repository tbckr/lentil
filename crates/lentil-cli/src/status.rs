//! `lentil status` (PRD FR-E2).

use std::path::Path;
use std::process::ExitCode;

use lentil_core::config::{CONFIG_FILE, Config};
use lentil_core::finding::Finding;
use lentil_core::finding::FindingCode;
use lentil_core::ledger;
use lentil_core::snapshot::Snapshot;
use lentil_core::status;

use crate::context;
use crate::i18n::{self, Lang, Msg};
use crate::{CliError, CliResult};

/// Open items listed without `--all`.
const SHOWN: usize = 20;

pub fn run(lang: Lang, repo: Option<&Path>, all: bool) -> CliResult {
    let root = context::find_root(lang, repo)?;
    let snapshot = Snapshot::from_dir(&root).map_err(|e| CliError(e.to_string()))?;
    let config = Config::parse(snapshot.text(CONFIG_FILE).unwrap_or_default()).map_err(|e| {
        CliError(i18n::msg(
            lang,
            Msg::ConfigMissing,
            &[("detail", &e.to_string())],
        ))
    })?;
    let ledger = ledger::load(&snapshot, &config.main_file);
    let report = status::status(&config, &ledger, &snapshot);

    if report.open.is_empty()
        && report.failed_assertions.is_empty()
        && report.unchecked_runs.is_empty()
    {
        println!("{}", i18n::text(lang, Msg::StatusClean));
        return Ok(ExitCode::SUCCESS);
    }
    if !report.open.is_empty() {
        println!(
            "{}",
            i18n::msg(
                lang,
                Msg::StatusOpen,
                &[("count", &report.open.len().to_string())]
            )
        );
        let shown = if all { report.open.len() } else { SHOWN };
        for item in report.open.iter().take(shown) {
            println!(
                "  {} {}  ({}:{})",
                item.date, item.text, item.file, item.line
            );
        }
        if report.open.len() > shown {
            let more = (report.open.len() - shown).to_string();
            println!(
                "{}",
                i18n::msg(lang, Msg::StatusOpenMore, &[("count", &more)])
            );
        }
    }
    if !report.failed_assertions.is_empty() {
        println!(
            "{}",
            i18n::msg(
                lang,
                Msg::StatusFailed,
                &[("count", &report.failed_assertions.len().to_string())],
            )
        );
        for failure in &report.failed_assertions {
            let mut finding = Finding::warning(FindingCode::BalanceAssertionFailed)
                .arg("account", failure.account.clone())
                .arg("date", failure.date.to_string())
                .arg("asserted", failure.asserted.to_string())
                .arg("difference", failure.difference.to_string())
                .arg("currency", failure.currency.clone());
            if let Some(file) = &failure.file {
                finding = finding.at(file.clone(), failure.line);
            }
            println!("  {}", i18n::finding(lang, &finding));
        }
    }
    if !report.unchecked_runs.is_empty() {
        println!(
            "{}",
            i18n::msg(
                lang,
                Msg::StatusUnchecked,
                &[("count", &report.unchecked_runs.len().to_string())],
            )
        );
        for run in &report.unchecked_runs {
            println!("  {}", run.file);
        }
    }
    Ok(ExitCode::SUCCESS)
}
