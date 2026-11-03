//! `lentil rules apply` (PRD FR-C19).
//!
//! The command commits through [`reapply::sync`], which plans anew on every
//! fetched state. A dry run plans on the local files instead and needs no
//! Git, so a rule can be tried before it is committed; it validates the
//! result as the sync would.

use std::path::Path;
use std::process::ExitCode;

use lentil_core::intent::Outcome;
use lentil_core::reapply::{self, Field, ReapplyError, ReapplyOptions, RuleChange};
use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{Credentials, ThreadSleeper};
use lentil_core::validate::{self, Context, Role};

use crate::context;
use crate::i18n::{self, Lang, Msg};
use crate::{CliError, CliResult};

pub fn apply(lang: Lang, repo: Option<&Path>, options: ReapplyOptions, dry_run: bool) -> CliResult {
    let root = context::find_root(lang, repo)?;
    if dry_run {
        return preview(lang, &root, &options);
    }
    let git_err = |e| CliError(i18n::git_error(lang, &e));
    let mut git = Git2Backend::open(&root, Credentials::Default).map_err(git_err)?;
    git.ensure_no_local_changes().map_err(git_err)?;
    let author = context::author(lang, &git)?;
    let identity = context::identity(Role::Editor)?;
    let outcome = reapply::sync(
        &mut git,
        &mut ThreadSleeper,
        &identity,
        &author,
        &options,
        &[],
    )
    .map_err(|e| match e {
        ReapplyError::Sync(e) => context::sync_error(lang, e),
        ReapplyError::Config(e) => context::config_error(lang, &e),
        ReapplyError::Rules(e) => context::rules_error(lang, &e),
        ReapplyError::Taxonomy(e) => context::taxonomy_error(lang, &e),
    })?;
    let skipped = report(lang, &outcome.result.changes, false);
    context::print_findings(lang, &outcome.warnings);
    Ok(exit_code(skipped))
}

/// The dry run: plans and replays on the working tree, validates the result
/// as an editor commit and saves nothing.
fn preview(lang: Lang, root: &Path, options: &ReapplyOptions) -> CliResult {
    let before = Snapshot::from_dir(root).map_err(|e| CliError(e.to_string()))?;
    let reasons = validate::read_only_reasons(&before);
    if !reasons.is_empty() {
        eprintln!("{}", i18n::text(lang, Msg::ReadOnly));
        context::print_findings(lang, &reasons);
        return Ok(ExitCode::FAILURE);
    }
    let config = context::config(lang, &before)?;
    let rules = context::rules(lang, &before, &config)?;
    let taxonomy = context::taxonomy(lang, &before)?;
    let applied = reapply::apply(&before, &config, &rules, taxonomy.as_ref(), options, &[]);
    let skipped = report(lang, &applied.changes, true);
    let mut blocked = false;
    if applied.snapshot != before {
        let device = context::device_id()?;
        let validation = validate::validate(
            &applied.snapshot,
            &Context {
                role: Role::Editor,
                device: Some(&device),
                before: Some(&before),
            },
        );
        blocked = validation.is_blocking();
        if blocked {
            eprintln!("{}", i18n::text(lang, Msg::RulesWouldBlock));
        }
        context::print_findings(lang, &validation.findings);
    }
    println!("{}", i18n::text(lang, Msg::RulesDryRun));
    Ok(exit_code(skipped || blocked))
}

fn exit_code(failed: bool) -> ExitCode {
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Prints the applied changes to stdout and the skipped ones to stderr, each
/// grouped by transaction, and returns whether any change was skipped.
fn report(lang: Lang, changes: &[(RuleChange, Outcome)], dry_run: bool) -> bool {
    let applied: Vec<(&RuleChange, Option<&str>)> = changes
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::Applied)
        .map(|(change, _)| (change, None))
        .collect();
    let skipped: Vec<(&RuleChange, Option<&str>)> = changes
        .iter()
        .filter_map(|(change, outcome)| match outcome {
            Outcome::Conflict(conflict) => {
                Some((change, Some(i18n::conflict(lang, conflict.code))))
            }
            Outcome::Applied | Outcome::AlreadyApplied => None,
        })
        .collect();

    if !applied.is_empty() {
        let header = if dry_run {
            Msg::RulesWouldChange
        } else {
            Msg::RulesChanged
        };
        let counts = [
            (Field::Payee, Msg::FieldPayee),
            (Field::Account, Msg::FieldAccount),
            (Field::Tag, Msg::FieldTags),
        ]
        .into_iter()
        .filter_map(|(field, label)| {
            let count = applied.iter().filter(|(c, _)| c.field == field).count();
            (count > 0).then(|| format!("{}: {count}", i18n::text(lang, label)))
        })
        .collect::<Vec<_>>()
        .join(", ");
        println!("{}", i18n::msg(lang, header, &[("counts", &counts)]));
        for line in lines(lang, &applied) {
            println!("{line}");
        }
    }
    if !skipped.is_empty() {
        let header = if dry_run {
            Msg::RulesWouldSkip
        } else {
            Msg::RulesSkipped
        };
        let count = skipped.len().to_string();
        eprintln!("{}", i18n::msg(lang, header, &[("count", &count)]));
        for line in lines(lang, &skipped) {
            eprintln!("{line}");
        }
    }
    if applied.is_empty() && skipped.is_empty() {
        println!("{}", i18n::text(lang, Msg::RulesNothing));
    }
    !skipped.is_empty()
}

/// One line per transaction (date, payee and narration, location) followed
/// by one line per change, with the reason if it was skipped.
fn lines(lang: Lang, changes: &[(&RuleChange, Option<&str>)]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = None;
    for &(change, reason) in changes {
        if current != Some((&change.file, change.line)) {
            current = Some((&change.file, change.line));
            let date = change.date.to_string();
            let line = change.line.to_string();
            lines.push(i18n::msg(
                lang,
                Msg::RulesTransaction,
                &[
                    ("date", &date),
                    ("text", &change.text),
                    ("file", &change.file),
                    ("line", &line),
                ],
            ));
        }
        let (field, quoted) = match change.field {
            Field::Payee => (i18n::text(lang, Msg::FieldPayee), true),
            Field::Account => (i18n::text(lang, Msg::FieldAccount), false),
            Field::Tag => {
                lines.push(tag_line(lang, "+", &change.new, reason));
                continue;
            }
        };
        let shown = |value: Option<&str>| match value {
            Some(value) if quoted => format!("{value:?}"),
            Some(value) => value.to_owned(),
            None => i18n::text(lang, Msg::NoPayee).to_owned(),
        };
        let old = shown(change.old.as_deref());
        let new = shown(Some(&change.new));
        let mut args = vec![("field", field), ("old", &old), ("new", &new)];
        lines.push(match reason {
            Some(reason) => {
                args.push(("reason", reason));
                i18n::msg(lang, Msg::RulesConflict, &args)
            }
            None => i18n::msg(lang, Msg::RulesChange, &args),
        });
    }
    lines
}

/// One added (`+`) or removed (`-`) tag, with the reason if it was skipped.
fn tag_line(lang: Lang, sign: &str, tag: &str, reason: Option<&str>) -> String {
    let mut args = vec![("sign", sign), ("tag", tag)];
    match reason {
        Some(reason) => {
            args.push(("reason", reason));
            i18n::msg(lang, Msg::RulesTagConflict, &args)
        }
        None => i18n::msg(lang, Msg::RulesTag, &args),
    }
}
