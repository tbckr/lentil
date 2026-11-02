//! End-to-end: `init`, a CSV import through a local remote, a repeated import,
//! `status` and `check`, run as the real binary.

#![allow(clippy::unwrap_used)] // Test helpers outside #[test] functions.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MAPPING: &str = r#"[file]
delimiter = ";"
header = ["Date", "Amount"]

[numbers]
decimal = ","

[columns]
booking_date = { column = "Date", format = "%d.%m.%Y" }
amount = { column = "Amount" }
counterparty = ["Payee"]
purpose = ["Purpose"]

[balance]
from = "none"
"#;

const EXPORT: &str = "Date;Payee;Purpose;Amount
02.10.2026;Corner Grocery;Card payment 4711;-42,17
05.10.2026;Example Employer;Salary October;2500,00
07.10.2026;City Utilities;Electricity 10/2026;-64,00
";

const SOURCE: &str = r#"
[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"
"#;

const RULES: &str = r#"
[[rule]]
match = { counterparty = "(?i)^example employer$", purpose = "(?i)^salary" }
payee = "Employer"
account = "Income:Salary"
"#;

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    remote: PathBuf,
    repo: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".gitconfig"),
            "[user]\n\tname = Test User\n\temail = test@example.org\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        let remote = dir.path().join("remote.git");
        let repo = dir.path().join("ledger");
        let env = Self {
            home,
            remote,
            repo,
            _dir: dir,
        };
        env.git(
            env.home.as_path(),
            &["init", "--bare", "-b", "main", env.remote.to_str().unwrap()],
        );
        env
    }

    fn lentil(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lentil"))
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LANG", "C")
            .env_remove("LC_ALL")
            .env_remove("LC_MESSAGES")
            .output()
            .unwrap()
    }

    /// Adds the source `giro` and a rule by hand, as the README explains,
    /// pushes them and returns the path of a matching export.
    fn add_giro_source(&self) -> PathBuf {
        let repo = self.repo.to_str().unwrap();
        let config = std::fs::read_to_string(self.repo.join("lentil.toml")).unwrap();
        std::fs::write(self.repo.join("lentil.toml"), format!("{config}{SOURCE}")).unwrap();
        let rules = std::fs::read_to_string(self.repo.join("rules.toml")).unwrap();
        std::fs::write(self.repo.join("rules.toml"), format!("{rules}{RULES}")).unwrap();
        std::fs::create_dir_all(self.repo.join("sources")).unwrap();
        std::fs::write(self.repo.join("sources/giro.toml"), MAPPING).unwrap();
        let accounts = std::fs::read_to_string(self.repo.join("accounts.beancount")).unwrap();
        std::fs::write(
            self.repo.join("accounts.beancount"),
            format!(
                "{accounts}2000-01-01 open Assets:Bank:Giro EUR\n2000-01-01 open Income:Salary\n"
            ),
        )
        .unwrap();
        ok(&self.lentil(&["-C", repo, "check"]));
        self.git(&self.repo, &["add", "-A"]);
        self.git(
            &self.repo,
            &["commit", "-q", "-m", "admin: add source giro"],
        );
        self.git(&self.repo, &["push", "-q", "origin", "main"]);
        let export = self.home.join("export.csv");
        std::fs::write(&export, EXPORT).unwrap();
        export
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn ok(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

#[test]
fn init_import_status_check() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();

    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let log = env.git(&env.remote, &["log", "--format=%s%n%b", "main"]);
    assert!(log.starts_with("admin: init repository"), "{log}");
    assert!(log.contains("Lentil-Role: admin"), "{log}");
    assert!(ok(&env.lentil(&["-C", repo, "check"])).contains("valid"));

    let export = env.add_giro_source();
    let export = export.to_str().unwrap();
    let out = ok(&env.lentil(&[
        "-C",
        repo,
        "import",
        "giro",
        export,
        "--run-date",
        "2026-10-20",
        "--balance",
        "2393,83",
        "--balance-date",
        "2026-10-19",
    ]));
    assert!(out.contains("3 new transactions"), "{out}");

    let subject = env.git(&env.remote, &["log", "-1", "--format=%s%n%b", "main"]);
    assert!(subject.starts_with("importer: giro run "), "{subject}");
    assert!(subject.contains(", 3 transactions"), "{subject}");
    assert!(subject.contains("Lentil-Role: importer"), "{subject}");
    assert!(subject.contains("Lentil-Client: lentil-cli/"), "{subject}");

    // The working tree follows the pushed commit, and the rule set the
    // payee and the account.
    let runs: Vec<_> = std::fs::read_dir(env.repo.join("imports/giro/2026"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert_eq!(runs.len(), 1);
    assert!(
        runs[0].contains("2026-10-05 ! \"Employer\" \"Salary October\"\n"),
        "{}",
        runs[0]
    );
    assert!(
        runs[0].contains("  lentil_counterparty: \"Example Employer\"\n"),
        "{}",
        runs[0]
    );
    assert!(runs[0].contains("  Income:Salary\n"), "{}", runs[0]);

    // A repeated import finds nothing new and writes no commit.
    let before = env.git(&env.remote, &["rev-parse", "main"]);
    let out = ok(&env.lentil(&[
        "-C",
        repo,
        "import",
        "giro",
        export,
        "--run-date",
        "2026-10-20",
        "--yes",
    ]));
    assert!(out.contains("Nothing new"), "{out}");
    assert_eq!(before, env.git(&env.remote, &["rev-parse", "main"]));

    let status = ok(&env.lentil(&["-C", repo, "status"]));
    assert!(status.contains("Open transactions: 3"), "{status}");
    assert!(ok(&env.lentil(&["-C", repo, "check"])).contains("valid"));

    // History stays linear.
    let parents = env.git(&env.remote, &["log", "--format=%p", "main"]);
    assert!(parents.lines().all(|p| !p.contains(' ')), "{parents}");
}

#[test]
fn first_checked_import_books_the_opening_balance() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    let export = export.to_str().unwrap();

    // The first import skips the balance.
    let import = |extra: &[&str]| {
        let mut args = vec![
            "-C",
            repo,
            "import",
            "giro",
            export,
            "--run-date",
            "2026-10-20",
        ];
        args.extend(extra);
        ok(&env.lentil(&args))
    };
    import(&["--yes"]);
    let status = ok(&env.lentil(&["-C", repo, "status"]));
    assert!(
        status.contains("Imports without a balance check: 1"),
        "{status}"
    );

    // A later balance of 2500.00 explains 106.17 before the first transaction.
    let out = import(&["--balance", "2500,00", "--balance-date", "2026-10-19"]);
    assert!(out.contains("opening balance 106.17 EUR"), "{out}");
    let runs: Vec<String> = std::fs::read_dir(env.repo.join("imports/giro/2026"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    let second = runs.iter().find(|r| r.contains("Opening balance")).unwrap();
    assert!(
        second.contains("2026-10-02 ! \"Opening balance\""),
        "{second}"
    );
    assert!(
        second.contains("Assets:Bank:Giro       106.17 EUR"),
        "{second}"
    );
    assert!(
        second.contains("2026-10-20 balance Assets:Bank:Giro  2500.00 ~ 0.00 EUR"),
        "{second}"
    );

    let status = ok(&env.lentil(&["-C", repo, "status"]));
    assert!(
        !status.contains("Imports without a balance check"),
        "{status}"
    );
    assert!(!status.contains("Failed balance checks"), "{status}");
    assert!(ok(&env.lentil(&["-C", repo, "check"])).contains("valid"));
}

#[test]
fn import_rejects_rules_in_the_old_format() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    std::fs::write(
        env.repo.join("rules.toml"),
        "[[rule]]\ndesc = \"(?i)salary\"\naccount = \"Income:Salary\"\n",
    )
    .unwrap();
    env.git(&env.repo, &["commit", "-q", "-am", "admin: old rules"]);
    env.git(&env.repo, &["push", "-q", "origin", "main"]);
    let before = env.git(&env.remote, &["rev-parse", "main"]);
    let out = env.lentil(&[
        "-C",
        repo,
        "import",
        "giro",
        export.to_str().unwrap(),
        "--run-date",
        "2026-10-20",
        "--yes",
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rules.toml is invalid"), "{stderr}");
    assert!(stderr.contains("desc"), "{stderr}");
    assert_eq!(before, env.git(&env.remote, &["rev-parse", "main"]));
}

#[test]
fn check_agrees_with_bean_check() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo]));
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let command = format!("uv run --project {} bean-check", project.display());
    if std::env::var_os("LENTIL_SKIP_DIFFERENTIAL").is_some() {
        return;
    }
    let out = ok(&env.lentil(&["-C", repo, "check", "--reference", "--bean-check", &command]));
    assert!(out.contains("bean-check agrees"), "{out}");
}

#[test]
fn init_fills_an_existing_repository_without_commits() {
    let env = Env::new();
    std::fs::create_dir_all(&env.repo).unwrap();
    env.git(&env.repo, &["init", "-q", "-b", "master"]);
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo]));
    assert!(env.repo.join("lentil.toml").is_file());
    assert!(ok(&env.lentil(&["-C", repo, "check"])).contains("valid"));
    assert_eq!(
        env.git(&env.repo, &["branch", "--show-current"]).trim(),
        "main"
    );
}

#[test]
fn init_refuses_a_remote_with_commits() {
    let env = Env::new();
    let first = env.home.join("first");
    ok(&env.lentil(&[
        "init",
        first.to_str().unwrap(),
        "--remote",
        env.remote.to_str().unwrap(),
    ]));
    let out = env.lentil(&[
        "init",
        env.repo.to_str().unwrap(),
        "--remote",
        env.remote.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("has commits already"));
}
