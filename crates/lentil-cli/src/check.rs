//! `lentil check` (PRD FR-A5, FR-B3, FR-B4): validates the working tree,
//! checks `rules.toml` and the mappings, and optionally compares the result
//! with Python Beancount's `bean-check`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitCode};

use jiff::Zoned;
use lentil_core::config::{CONFIG_FILE, Config};
use lentil_core::finding::{Finding, FindingCode, Severity};
use lentil_core::import::check_inputs;
use lentil_core::snapshot::Snapshot;
use lentil_core::validate::{self, Context, Role};

use crate::context;
use crate::i18n::{self, Lang, Msg};
use crate::{CliError, CliResult};

pub fn run(lang: Lang, repo: Option<&Path>, reference: Option<&str>) -> CliResult {
    let root = context::find_root(lang, repo)?;
    let snapshot = Snapshot::from_dir(&root).map_err(|e| CliError(e.to_string()))?;
    let validation = validate::validate(
        &snapshot,
        &Context {
            role: Role::Admin,
            device: None,
            before: None,
        },
    );
    // Problems that would stop an import or `rules apply`; without a valid
    // configuration, validation has reported that already.
    let problems = match (&validation.config, &validation.ledger) {
        (Some(config), Some(ledger)) => {
            check_inputs(&snapshot, config, ledger, Zoned::now().date())
        }
        _ => Vec::new(),
    };
    let blocking_findings = validation
        .findings
        .iter()
        .filter(|f| f.is_blocking())
        .count();
    let warnings = validation.findings.len() - blocking_findings;
    let blocking = blocking_findings + problems.len();
    context::print_findings(lang, &validation.findings);
    for problem in &problems {
        eprintln!("{}", i18n::input_problem(lang, problem));
    }
    if validation.findings.is_empty() && problems.is_empty() {
        println!("{}", i18n::text(lang, Msg::CheckOk));
    } else {
        println!(
            "{}",
            i18n::msg(
                lang,
                Msg::CheckFindings,
                &[
                    ("blocking", &blocking.to_string()),
                    ("warnings", &warnings.to_string())
                ],
            )
        );
    }
    let mut ok = blocking == 0;

    if let Some(command) = reference {
        let main = snapshot
            .text(CONFIG_FILE)
            .and_then(|t| Config::parse(t).ok())
            .map_or_else(|| "main.beancount".to_owned(), |c| c.main_file);
        ok &= compare_with_reference(lang, &root, &main, command, &validation.findings)?;
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Findings the reference implementation can also report: those from the
/// core's own checks, not Lentil's convention rules.
fn comparable(finding: &Finding) -> bool {
    matches!(
        finding.code,
        FindingCode::ParseError
            | FindingCode::TransactionUnbalanced
            | FindingCode::AccountNotOpen
            | FindingCode::BalanceAssertionFailed
    ) || (finding.code == FindingCode::CoreError && finding.severity == Severity::Blocking)
}

fn compare_with_reference(
    lang: Lang,
    root: &Path,
    main: &str,
    command: &str,
    findings: &[Finding],
) -> Result<bool, CliError> {
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or("bean-check");
    let output = Command::new(program)
        .args(parts)
        .arg(main)
        .current_dir(root)
        .output()
        .map_err(|e| {
            CliError(i18n::msg(
                lang,
                Msg::ReferenceFailed,
                &[("command", command), ("detail", &e.to_string())],
            ))
        })?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let reference = parse_reference(&text, root);
    let core: BTreeSet<(String, u32)> = findings
        .iter()
        .filter(|f| comparable(f))
        .filter_map(|f| Some((f.file.clone()?, f.line?)))
        .collect();
    let core_failed = findings.iter().any(comparable);
    let reference_failed = !output.status.success();
    let reference_locations: BTreeSet<(String, u32)> =
        reference.iter().map(|(f, l, _)| (f.clone(), *l)).collect();

    if core_failed == reference_failed && core == reference_locations {
        println!("{}", i18n::text(lang, Msg::ReferenceAgree));
        return Ok(true);
    }
    println!("{}", i18n::text(lang, Msg::ReferenceDisagree));
    for (file, line) in core.difference(&reference_locations) {
        println!(
            "{}",
            i18n::msg(
                lang,
                Msg::ReferenceOnlyCore,
                &[("file", file), ("line", &line.to_string())]
            )
        );
    }
    for (file, line, detail) in &reference {
        if !core.contains(&(file.clone(), *line)) {
            println!(
                "{}",
                i18n::msg(
                    lang,
                    Msg::ReferenceOnlyReference,
                    &[
                        ("file", file),
                        ("line", &line.to_string()),
                        ("detail", detail)
                    ],
                )
            );
        }
    }
    Ok(false)
}

/// Error lines of `bean-check`: `path:line: message`, paths made relative to `root`.
fn parse_reference(text: &str, root: &Path) -> Vec<(String, u32, String)> {
    let root = root.to_string_lossy();
    text.lines()
        .filter_map(|line| {
            let (path, rest) = line.split_once(':')?;
            let (number, message) = rest.split_once(':')?;
            let number: u32 = number.trim().parse().ok()?;
            if number == 0 {
                return None;
            }
            let path = path
                .strip_prefix(root.as_ref())
                .map_or(path, |p| p.trim_start_matches('/'));
            Some((path.to_owned(), number, message.trim().to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bean_check_lines() {
        let text = "/r/main.beancount:12: Transaction does not balance\n\n<load>:0: glob\nnoise\n";
        assert_eq!(
            parse_reference(text, Path::new("/r")),
            vec![(
                "main.beancount".to_owned(),
                12,
                "Transaction does not balance".to_owned()
            )]
        );
    }
}
