//! `lentil rules apply` (PRD FR-C19, FR-C21).
//!
//! The command commits through [`reapply::sync`], which plans anew on every
//! fetched state. A dry run plans on the local files instead and needs no
//! Git, so a rule can be tried before it is committed; it validates the
//! result as the sync would.
//!
//! With `--stale-tags`, it first lists the rule tags the rules no longer
//! yield, on the fetched state, and asks per tag in a terminal. Only the
//! confirmed ones become `remove-tag` intents, which the sync replays as
//! they are and never plans again (spec section 16).

use std::path::Path;
use std::process::ExitCode;

use jiff::civil::Date;
use lentil_core::intent::Outcome;
use lentil_core::ledger;
use lentil_core::reapply::{self, Field, ReapplyError, ReapplyOptions, RuleChange, TagRemoval};
use lentil_core::snapshot::Snapshot;
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{Credentials, GitBackend, ThreadSleeper};
use lentil_core::validate::{self, Context, Role};

use crate::i18n::{self, Lang, Msg};
use crate::{CliError, CliResult};
use crate::{context, prompt};

/// The arguments of `lentil rules apply`.
pub struct Args {
    /// Which transactions and fields.
    pub options: ReapplyOptions,
    /// Plan on the local files and save nothing.
    pub dry_run: bool,
    /// Also list stale rule tags and ask whether to remove each.
    pub stale_tags: bool,
    /// Never ask: remove no tag.
    pub yes: bool,
}

pub fn apply(lang: Lang, repo: Option<&Path>, args: &Args) -> CliResult {
    let root = context::find_root(lang, repo)?;
    if args.dry_run {
        return preview(lang, &root, args);
    }
    let git_err = |e| CliError(i18n::git_error(lang, &e));
    let mut git = Git2Backend::open(&root, Credentials::Default).map_err(git_err)?;
    git.ensure_no_local_changes().map_err(git_err)?;
    let author = context::author(lang, &git)?;
    let identity = context::identity(Role::Editor)?;
    let removals = if args.stale_tags {
        // Ask on the fetched state; the sync then replays the answers.
        let head = git.fetch().map_err(git_err)?;
        let current = git.snapshot(head.as_ref()).map_err(git_err)?;
        let reasons = validate::read_only_reasons(&current);
        if !reasons.is_empty() {
            eprintln!("{}", i18n::text(lang, Msg::ReadOnly));
            context::print_findings(lang, &reasons);
            return Ok(ExitCode::FAILURE);
        }
        let candidates = candidates(lang, &current, &args.options)?;
        if !args.yes && prompt::interactive() && !candidates.is_empty() {
            print_stale_header(lang, &candidates);
            confirm_removals(
                lang,
                &candidates,
                &mut |line| println!("{line}"),
                &mut prompt::ask,
            )
        } else {
            list_stale(lang, &candidates);
            if !candidates.is_empty() {
                println!("{}", i18n::text(lang, Msg::StaleTagsKept));
            }
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let outcome = reapply::sync(
        &mut git,
        &mut ThreadSleeper,
        &identity,
        &author,
        &args.options,
        &removals,
    )
    .map_err(|e| match e {
        ReapplyError::Sync(e) => context::sync_error(lang, e),
        ReapplyError::Config(e) => context::config_error(lang, &e),
        ReapplyError::Rules(e) => context::rules_error(lang, &e),
        ReapplyError::Taxonomy(e) => context::taxonomy_error(lang, &e),
    })?;
    let skipped = report(
        lang,
        &outcome.result.removals,
        &outcome.result.changes,
        false,
    );
    context::print_findings(lang, &outcome.warnings);
    Ok(exit_code(skipped))
}

/// The removal candidates of `snapshot` (spec section 16).
fn candidates(
    lang: Lang,
    snapshot: &Snapshot,
    options: &ReapplyOptions,
) -> Result<Vec<TagRemoval>, CliError> {
    let config = context::config(lang, snapshot)?;
    let rules = context::rules(lang, snapshot, &config)?;
    let taxonomy = context::taxonomy(lang, snapshot)?;
    let ledger = ledger::load(snapshot, &config.main_file);
    Ok(reapply::removal_candidates(
        &ledger,
        &config,
        &rules,
        taxonomy.as_ref(),
        options,
    ))
}

fn print_stale_header(lang: Lang, candidates: &[TagRemoval]) {
    let count = candidates.len().to_string();
    println!(
        "{}",
        i18n::msg(lang, Msg::StaleTagsHeader, &[("count", &count)])
    );
}

/// Lists `candidates` without asking, or says there are none.
fn list_stale(lang: Lang, candidates: &[TagRemoval]) {
    if candidates.is_empty() {
        println!("{}", i18n::text(lang, Msg::StaleTagsNone));
        return;
    }
    print_stale_header(lang, candidates);
    let mut current = None;
    for candidate in candidates {
        if current != Some((&candidate.file, candidate.line)) {
            current = Some((&candidate.file, candidate.line));
            println!("{}", removal_transaction(lang, candidate));
        }
        println!(
            "{}",
            i18n::msg(lang, Msg::StaleTag, &[("tag", &candidate.tag)])
        );
    }
}

/// Asks per candidate whether to remove its tag and returns the confirmed
/// ones. Before the first question about a transaction, `show` gets its
/// line. The end of input ends the questions and keeps the rest.
fn confirm_removals(
    lang: Lang,
    candidates: &[TagRemoval],
    show: &mut dyn FnMut(&str),
    ask: &mut dyn FnMut(&str) -> Option<String>,
) -> Vec<TagRemoval> {
    let mut confirmed = Vec::new();
    let mut current = None;
    for candidate in candidates {
        if current != Some((&candidate.file, candidate.line)) {
            current = Some((&candidate.file, candidate.line));
            show(&removal_transaction(lang, candidate));
        }
        let question = i18n::msg(lang, Msg::PromptRemoveTag, &[("tag", &candidate.tag)]);
        match ask(&question) {
            None => break,
            Some(answer) if prompt::is_yes(&answer) => confirmed.push(candidate.clone()),
            Some(_) => {}
        }
    }
    confirmed
}

/// The dry run: plans and replays on the working tree, validates the result
/// as an editor commit and saves nothing.
fn preview(lang: Lang, root: &Path, args: &Args) -> CliResult {
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
    if args.stale_tags {
        list_stale(lang, &candidates(lang, &before, &args.options)?);
    }
    let applied = reapply::apply(
        &before,
        &config,
        &rules,
        taxonomy.as_ref(),
        &args.options,
        &[],
    );
    let skipped = report(lang, &applied.removals, &applied.changes, true);
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

/// The reason of a conflicting outcome, translated.
fn conflict_reason(lang: Lang, outcome: &Outcome) -> Option<&'static str> {
    match outcome {
        Outcome::Conflict(conflict) => Some(i18n::conflict(lang, conflict.code)),
        Outcome::Applied | Outcome::AlreadyApplied => None,
    }
}

/// Prints the applied removals and changes to stdout and the skipped ones to
/// stderr, each grouped by transaction, and returns whether any was skipped.
fn report(
    lang: Lang,
    removals: &[(TagRemoval, Outcome)],
    changes: &[(RuleChange, Outcome)],
    dry_run: bool,
) -> bool {
    let removed: Vec<(&TagRemoval, Option<&str>)> = removals
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::Applied)
        .map(|(removal, _)| (removal, None))
        .collect();
    let applied: Vec<(&RuleChange, Option<&str>)> = changes
        .iter()
        .filter(|(_, outcome)| *outcome == Outcome::Applied)
        .map(|(change, _)| (change, None))
        .collect();
    let skipped_removals: Vec<(&TagRemoval, Option<&str>)> = removals
        .iter()
        .filter_map(|(removal, outcome)| Some((removal, Some(conflict_reason(lang, outcome)?))))
        .collect();
    let skipped: Vec<(&RuleChange, Option<&str>)> = changes
        .iter()
        .filter_map(|(change, outcome)| Some((change, Some(conflict_reason(lang, outcome)?))))
        .collect();

    if !removed.is_empty() {
        let count = removed.len().to_string();
        println!(
            "{}",
            i18n::msg(lang, Msg::TagsRemoved, &[("count", &count)])
        );
        for line in removal_lines(lang, &removed) {
            println!("{line}");
        }
    }
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
    let any_skipped = !skipped_removals.is_empty() || !skipped.is_empty();
    if any_skipped {
        let header = if dry_run {
            Msg::RulesWouldSkip
        } else {
            Msg::RulesSkipped
        };
        let count = (skipped_removals.len() + skipped.len()).to_string();
        eprintln!("{}", i18n::msg(lang, header, &[("count", &count)]));
        for line in removal_lines(lang, &skipped_removals)
            .into_iter()
            .chain(lines(lang, &skipped))
        {
            eprintln!("{line}");
        }
    }
    if removed.is_empty() && applied.is_empty() && !any_skipped {
        println!("{}", i18n::text(lang, Msg::RulesNothing));
    }
    any_skipped
}

/// The line that introduces a transaction: date, payee and narration, and
/// its location.
fn transaction_line(lang: Lang, date: Date, text: &str, file: &str, line: u32) -> String {
    let date = date.to_string();
    let line = line.to_string();
    i18n::msg(
        lang,
        Msg::RulesTransaction,
        &[
            ("date", &date),
            ("text", text),
            ("file", file),
            ("line", &line),
        ],
    )
}

fn removal_transaction(lang: Lang, removal: &TagRemoval) -> String {
    transaction_line(
        lang,
        removal.date,
        &removal.text,
        &removal.file,
        removal.line,
    )
}

/// One line per transaction followed by one line per removed tag, with the
/// reason if it was skipped.
fn removal_lines(lang: Lang, removals: &[(&TagRemoval, Option<&str>)]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = None;
    for &(removal, reason) in removals {
        if current != Some((&removal.file, removal.line)) {
            current = Some((&removal.file, removal.line));
            lines.push(removal_transaction(lang, removal));
        }
        lines.push(tag_line(lang, "-", &removal.tag, reason));
    }
    lines
}

/// One line per transaction (date, payee and narration, location) followed
/// by one line per change, with the reason if it was skipped.
fn lines(lang: Lang, changes: &[(&RuleChange, Option<&str>)]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = None;
    for &(change, reason) in changes {
        if current != Some((&change.file, change.line)) {
            current = Some((&change.file, change.line));
            lines.push(transaction_line(
                lang,
                change.date,
                &change.text,
                &change.file,
                change.line,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, line: u32, tag: &str) -> TagRemoval {
        TagRemoval {
            id: id.to_owned(),
            file: "imports/giro/2026/x.beancount".to_owned(),
            line,
            date: "2026-10-02".parse().unwrap(),
            text: format!("Shop {id}"),
            tag: tag.to_owned(),
        }
    }

    /// Runs [`confirm_removals`] with `answers` and returns what it confirmed,
    /// showed and asked.
    fn confirm(
        candidates: &[TagRemoval],
        answers: &[Option<&str>],
    ) -> (Vec<TagRemoval>, Vec<String>, Vec<String>) {
        let mut shown = Vec::new();
        let mut asked = Vec::new();
        let mut answers = answers.iter();
        let confirmed = confirm_removals(
            Lang::En,
            candidates,
            &mut |line| shown.push(line.to_owned()),
            &mut |question| {
                asked.push(question.to_owned());
                answers.next().copied().flatten().map(ToOwned::to_owned)
            },
        );
        (confirmed, shown, asked)
    }

    #[test]
    fn confirm_removals_asks_per_tag_and_keeps_the_rest() {
        let candidates = [
            candidate("t1", 3, "a"),
            candidate("t1", 3, "b"),
            candidate("t2", 9, "c"),
        ];
        let (confirmed, shown, asked) = confirm(&candidates, &[Some("y"), Some(""), Some("ja")]);
        assert_eq!(
            confirmed,
            vec![candidates[0].clone(), candidates[2].clone()]
        );
        assert_eq!(
            shown,
            vec![
                "  2026-10-02 Shop t1  (imports/giro/2026/x.beancount:3)",
                "  2026-10-02 Shop t2  (imports/giro/2026/x.beancount:9)",
            ]
        );
        assert_eq!(
            asked,
            vec![
                "    Remove #a? [y/N] ",
                "    Remove #b? [y/N] ",
                "    Remove #c? [y/N] ",
            ]
        );

        let (confirmed, shown, asked) = confirm(&candidates, &[None]);
        assert_eq!(confirmed, Vec::new());
        assert_eq!(shown.len(), 1);
        assert_eq!(asked, vec!["    Remove #a? [y/N] "]);
    }

    #[test]
    fn removed_tags_are_listed_per_transaction_with_their_reason() {
        let (a, b, c) = (
            candidate("t1", 3, "a"),
            candidate("t1", 3, "b"),
            candidate("t2", 9, "c"),
        );
        assert_eq!(
            removal_lines(
                Lang::En,
                &[
                    (&a, None),
                    (&b, None),
                    (&c, Some("the transaction was dismissed"))
                ]
            ),
            vec![
                "  2026-10-02 Shop t1  (imports/giro/2026/x.beancount:3)",
                "    - #a",
                "    - #b",
                "  2026-10-02 Shop t2  (imports/giro/2026/x.beancount:9)",
                "    - #c: the transaction was dismissed",
            ]
        );
    }
}
