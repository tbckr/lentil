//! The `lentil` command line client: `init`, `import`, `status` and `check`.

mod check;
mod context;
mod i18n;
mod import;
mod init;
mod prompt;
mod status;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jiff::civil::Date;

use crate::i18n::Lang;

/// Name and version for the `Lentil-Client` trailer and run headers.
pub const CLIENT: &str = concat!("lentil-cli/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Parser)]
#[command(
    name = "lentil",
    version,
    about = "Everyday Beancount on a Git repository"
)]
struct Cli {
    /// Repository directory (default: the current directory or the nearest
    /// parent with a lentil.toml).
    #[arg(short = 'C', long = "repo", global = true)]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new, empty Lentil repository.
    Init {
        /// Directory to create (default: the current directory).
        dir: Option<PathBuf>,
        /// Ledger title.
        #[arg(long, default_value = "Ledger")]
        title: String,
        /// Operating currency.
        #[arg(long, default_value = "EUR")]
        currency: String,
        /// Git remote to upload the new repository to (must be empty).
        #[arg(long)]
        remote: Option<String>,
    },
    /// Import a bank's CSV export for one source.
    Import {
        /// Source ID from lentil.toml.
        source: String,
        /// The CSV export.
        file: PathBuf,
        /// Balance shown in online banking, if the export has none.
        #[arg(long, allow_hyphen_values = true)]
        balance: Option<String>,
        /// Date the balance refers to (default: today).
        #[arg(long, requires = "balance")]
        balance_date: Option<Date>,
        /// Never ask: skip the balance and possible duplicates.
        #[arg(long)]
        yes: bool,
        /// Treat this date as today (for testing and back-dated imports).
        #[arg(long, hide = true)]
        run_date: Option<Date>,
    },
    /// List open transactions, failed balance checks and unchecked imports.
    Status {
        /// List every open transaction.
        #[arg(long)]
        all: bool,
    },
    /// Validate the working tree, optionally against Python Beancount.
    Check {
        /// Also run the reference implementation and compare.
        #[arg(long)]
        reference: bool,
        /// Command that runs the reference check.
        #[arg(long, default_value = "bean-check")]
        bean_check: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let lang = Lang::from_env();
    let result = match cli.command {
        Command::Init {
            dir,
            title,
            currency,
            remote,
        } => init::run(
            lang,
            &dir.or(cli.repo).unwrap_or_else(|| PathBuf::from(".")),
            &title,
            &currency,
            remote.as_deref(),
        ),
        Command::Import {
            source,
            file,
            balance,
            balance_date,
            yes,
            run_date,
        } => import::run(
            lang,
            cli.repo.as_deref(),
            &import::Args {
                source,
                file,
                balance,
                balance_date,
                yes,
                run_date,
            },
        ),
        Command::Status { all } => status::run(lang, cli.repo.as_deref(), all),
        Command::Check {
            reference,
            bean_check,
        } => check::run(
            lang,
            cli.repo.as_deref(),
            reference.then_some(bean_check.as_str()),
        ),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("lentil: {err}");
            ExitCode::FAILURE
        }
    }
}

/// An error already translated for the user.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

/// Result of a command.
pub type CliResult = Result<ExitCode, CliError>;
