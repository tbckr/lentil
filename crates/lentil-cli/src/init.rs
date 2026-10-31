//! `lentil init` (PRD N1, FR-A1).

use std::path::Path;
use std::process::ExitCode;

use lentil_core::config::CONFIG_FILE;
use lentil_core::layout::{self, InitOptions};
use lentil_core::sync::git::Git2Backend;
use lentil_core::sync::{Credentials, GitBackend, ThreadSleeper, Work, run_sync};
use lentil_core::validate::Role;

use crate::context;
use crate::i18n::{self, Lang, Msg};
use crate::{CliError, CliResult};

/// Commits the files of a new repository as admin, checks them out into `dir`
/// and, with a remote, uploads them.
pub fn run(lang: Lang, dir: &Path, title: &str, currency: &str, remote: Option<&str>) -> CliResult {
    let shown = dir.display().to_string();
    let config = dir.join(CONFIG_FILE);
    if config.exists() {
        let config = config.display().to_string();
        return Err(CliError(i18n::msg(
            lang,
            Msg::InitExists,
            &[("dir", &config)],
        )));
    }
    std::fs::create_dir_all(dir).map_err(|e| CliError(e.to_string()))?;
    let files = layout::init_files(&InitOptions {
        title: title.to_owned(),
        currency: currency.to_owned(),
    });
    let mut git = if dir.join(".git").exists() {
        Git2Backend::open(dir, Credentials::Default)
    } else {
        Git2Backend::init(dir)
    }
    .map_err(|e| CliError(i18n::git_error(lang, &e)))?;
    git.point_unborn_head_to_main()
        .map_err(|e| CliError(i18n::git_error(lang, &e)))?;
    if let Some(url) = remote {
        git.set_remote(url)
            .map_err(|e| CliError(i18n::git_error(lang, &e)))?;
    }
    // A new repository starts empty: never overwrite files of an existing one.
    let head = git
        .fetch()
        .map_err(|e| CliError(i18n::git_error(lang, &e)))?;
    if head.is_some() {
        return Err(CliError(i18n::msg(
            lang,
            Msg::InitNotEmpty,
            &[("dir", &shown)],
        )));
    }
    if let Some(existing) = files.paths().find(|p| dir.join(p).exists()) {
        return Err(CliError(i18n::msg(
            lang,
            Msg::InitExists,
            &[("dir", existing)],
        )));
    }
    let author = context::author(lang, &git)?;
    let identity = context::identity(Role::Admin)?;
    run_sync(&mut git, &mut ThreadSleeper, &identity, &author, |before| {
        let mut after = before.clone();
        for (path, content) in files.files() {
            after.insert(path, content);
        }
        Work::Commit {
            after,
            subject: "init repository".to_owned(),
            result: (),
        }
    })
    .map_err(|e| context::sync_error(lang, e))?;

    println!("{}", i18n::msg(lang, Msg::InitDone, &[("dir", &shown)]));
    if let Some(url) = remote {
        let shown = lentil_core::sync::git::redact(url);
        println!(
            "{}",
            i18n::msg(lang, Msg::InitPushed, &[("remote", &shown)])
        );
    }
    Ok(ExitCode::SUCCESS)
}
