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

    /// Imports `export` into `giro` with a balance, so the run is checked.
    fn import_giro(&self, export: &Path) {
        ok(&self.lentil(&[
            "-C",
            self.repo.to_str().unwrap(),
            "import",
            "giro",
            export.to_str().unwrap(),
            "--run-date",
            "2026-10-20",
            "--balance",
            "2393,83",
            "--balance-date",
            "2026-10-19",
        ]));
    }

    /// The path and content of the only run file of `giro`.
    fn giro_run(&self) -> (PathBuf, String) {
        let runs: Vec<PathBuf> = std::fs::read_dir(self.repo.join("imports/giro/2026"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let [run] = runs.as_slice() else {
            panic!("{runs:?}")
        };
        let content = std::fs::read_to_string(run).unwrap();
        (run.clone(), content)
    }

    /// Appends `text` to the repository file `path`.
    fn append(&self, path: &str, text: &str) {
        let old = std::fs::read_to_string(self.repo.join(path)).unwrap();
        std::fs::write(self.repo.join(path), format!("{old}{text}")).unwrap();
    }

    /// Commits every change as the admin, by hand, and pushes it.
    fn admin_push(&self, message: &str) {
        self.git(&self.repo, &["commit", "-q", "-am", message]);
        self.git(&self.repo, &["push", "-q", "origin", "main"]);
    }

    /// The commit `main` of the remote points to.
    fn remote_head(&self) -> String {
        self.git(&self.remote, &["rev-parse", "main"])
    }

    /// Subject and body of the remote's last commit.
    fn last_commit(&self) -> String {
        self.git(&self.remote, &["log", "-1", "--format=%s%n%b", "main"])
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
fn import_rejects_rules_for_an_unknown_source() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    std::fs::write(
        env.repo.join("rules.toml"),
        "[[rule]]\nmatch = { source = \"girro\", purpose = \"(?i)^salary\" }\naccount = \"Income:Salary\"\n",
    )
    .unwrap();
    env.admin_push("admin: rule for a mistyped source");
    let before = env.remote_head();
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
    assert!(
        stderr.contains("rules.toml is invalid: rule 1 names the source \"girro\""),
        "{stderr}"
    );
    assert_eq!(before, env.remote_head());

    // `rules apply` reads the same rules, with and without a dry run.
    for dry_run in [false, true] {
        let mut args = vec!["-C", repo, "rules", "apply"];
        if dry_run {
            args.push("--dry-run");
        }
        let out = env.lentil(&args);
        assert!(!out.status.success(), "{dry_run}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("rules.toml is invalid: rule 1 names the source \"girro\""),
            "{dry_run}: {stderr}"
        );
    }
    assert_eq!(before, env.remote_head());
}

#[test]
fn import_rejects_a_mapping_that_is_not_utf8() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    let mut mapping = MAPPING.as_bytes().to_vec();
    mapping.extend_from_slice(b"# Empf\xe4nger\n");
    std::fs::write(env.repo.join("sources/giro.toml"), mapping).unwrap();
    env.admin_push("admin: mapping in windows-1252");
    let before = env.remote_head();
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
    assert!(
        stderr.contains("Mapping file sources/giro.toml is invalid: the file is not UTF-8 text"),
        "{stderr}"
    );
    assert!(!stderr.contains("not found"), "{stderr}");
    assert_eq!(before, env.remote_head());
}

#[test]
fn check_reports_broken_rules_and_mappings() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    env.add_giro_source();
    // Fails with `error` in stderr and without the all-clear in stdout.
    let fails = |error: &str| {
        let out = env.lentil(&["-C", repo, "check"]);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{error}");
        assert!(stderr.contains(error), "{error}: {stderr}");
        assert!(!stdout.contains("The ledger is valid."), "{stdout}");
        stdout
    };

    env.append(
        "rules.toml",
        "\n[[rule]]\nmatch = { purpose = \"(?i)garden\" }\naccount = \"Expenses:Garden\"\n",
    );
    let stdout = fails("rules.toml: rule 2 books to Expenses:Garden, which is not open today.");
    assert!(stdout.contains("1 errors, 0 warnings."), "{stdout}");

    std::fs::write(
        env.repo.join("rules.toml"),
        "[[rule]]\nmatch = { source = \"girro\", purpose = \"x\" }\npayee = \"X\"\n",
    )
    .unwrap();
    fails("rules.toml is invalid: rule 1 names the source \"girro\"");

    std::fs::write(
        env.repo.join("rules.toml"),
        b"# Regel f\xfcr die B\xe4ckerei\n",
    )
    .unwrap();
    fails("rules.toml is invalid: the file is not UTF-8 text");

    std::fs::write(env.repo.join("rules.toml"), "").unwrap();
    std::fs::write(
        env.repo.join("sources/giro.toml"),
        MAPPING.replace("decimal = \",\"", "decimal = \"1\""),
    )
    .unwrap();
    fails("Mapping file sources/giro.toml is invalid: decimal must not be a digit or sign");

    std::fs::remove_file(env.repo.join("sources/giro.toml")).unwrap();
    fails("Mapping file sources/giro.toml not found.");

    std::fs::write(env.repo.join("sources/giro.toml"), MAPPING).unwrap();
    assert!(ok(&env.lentil(&["-C", repo, "check"])).contains("The ledger is valid."));
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

/// Header of the imported grocery transaction, open.
const GROCERY: &str = "2026-10-02 ! \"Corner Grocery\" \"Card payment 4711\"\n";
/// Header of the imported utilities transaction, open.
const UTILITIES: &str = "2026-10-07 ! \"City Utilities\" \"Electricity 10/2026\"\n";

#[test]
fn rules_apply_sets_payees_and_accounts_as_editor() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    env.import_giro(&export);

    // New rules after the import, and the utilities transaction reviewed.
    env.append(
        "rules.toml",
        r#"
[[rule]]
match = { counterparty = "^Corner Grocery$" }
payee = "Grocer"
account = "Expenses:Food"

[[rule]]
match = { counterparty = "^City Utilities$" }
payee = "City Power"
"#,
    );
    env.append("accounts.beancount", "2000-01-01 open Expenses:Food\n");
    let (run, content) = env.giro_run();
    let reviewed = UTILITIES.replacen(" ! ", " * ", 1);
    std::fs::write(&run, content.replace(UTILITIES, &reviewed)).unwrap();
    let run = run.strip_prefix(&env.repo).unwrap().display().to_string();
    let (_, content) = env.giro_run();
    let line = |header: &str| {
        content
            .lines()
            .position(|l| l == header.trim_end())
            .unwrap()
            + 1
    };
    let apply = |extra: &[&str]| {
        let mut args = vec!["-C", repo, "rules", "apply"];
        args.extend(extra);
        env.lentil(&args)
    };

    // A dry run previews the uncommitted rules on the local files, only for
    // open transactions, and saves nothing.
    let before = env.remote_head();
    let out = ok(&apply(&["--dry-run"]));
    let grocery = format!(
        "  2026-10-02 Corner Grocery | Card payment 4711  ({run}:{})\n",
        line(GROCERY)
    );
    assert_eq!(
        out,
        format!(
            "The rules would change these transactions (payee: 1, account: 1):\n{grocery}    payee: \"Corner Grocery\" → \"Grocer\"\n    account: Expenses:Uncategorized → Expenses:Food\nDry run on the local files: nothing was saved.\n"
        )
    );
    assert_eq!(before, env.remote_head());
    assert_eq!(env.giro_run().1, content);

    env.admin_push("admin: new rules");

    // Only the payee, as an editor commit that leaves the flag alone.
    let out = ok(&apply(&["--only", "payee"]));
    assert!(
        out.starts_with("The rules changed these transactions (payee: 1):\n"),
        "{out}"
    );
    let log = env.last_commit();
    assert!(log.starts_with("editor: set-payee 1\n"), "{log}");
    assert!(log.contains("Lentil-Role: editor\n"), "{log}");
    assert!(log.contains("Lentil-Device: "), "{log}");
    let (_, content) = env.giro_run();
    assert!(
        content.contains("2026-10-02 ! \"Grocer\" \"Card payment 4711\"\n"),
        "{content}"
    );
    assert_eq!(content.matches("  Expenses:Uncategorized\n").count(), 2);

    // Then the account.
    let out = ok(&apply(&[]));
    assert!(
        out.starts_with("The rules changed these transactions (account: 1):\n  2026-10-02 Grocer | Card payment 4711  ("),
        "{out}"
    );
    assert!(env.last_commit().starts_with("editor: set-account 1\n"));
    let (_, content) = env.giro_run();
    assert!(content.contains("  Expenses:Food\n"), "{content}");
    assert_eq!(content.matches("  Expenses:Uncategorized\n").count(), 1);
    assert!(content.contains("2026-10-02 ! \"Grocer\""), "{content}");

    // A second run finds nothing and commits nothing.
    let before = env.remote_head();
    let out = ok(&apply(&[]));
    assert_eq!(out, "The rules change nothing.\n");
    assert_eq!(before, env.remote_head());

    // Reviewed transactions change only with --reviewed, and stay reviewed.
    let out = ok(&apply(&["--reviewed"]));
    assert!(
        out.contains("    payee: \"City Utilities\" → \"City Power\"\n"),
        "{out}"
    );
    let (_, content) = env.giro_run();
    assert!(
        content.contains("2026-10-07 * \"City Power\" \"Electricity 10/2026\"\n"),
        "{content}"
    );

    assert!(ok(&env.lentil(&["-C", repo, "check"])).contains("valid"));
    let parents = env.git(&env.remote, &["log", "--format=%p", "main"]);
    assert!(parents.lines().all(|p| !p.contains(' ')), "{parents}");
}

#[test]
fn rules_apply_adds_tags_and_respects_tags_toml() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    env.import_giro(&export);
    env.append(
        "rules.toml",
        r#"
[[rule]]
match = { counterparty = "^City Utilities$" }
add_tags = ["utilities"]

[[rule]]
match = { tags = ["utilities"] }
add_tags = ["recurring"]

[[rule]]
match = { counterparty = "^Corner Grocery$" }
add_tags = ["recurring"]
"#,
    );
    std::fs::write(
        env.repo.join("tags.toml"),
        "tags = [\"recurring\", \"irregular\", \"utilities\"]\nexclusive = [[\"recurring\", \"irregular\"]]\n",
    )
    .unwrap();
    // The grocery carries the rival of `recurring`, so no rule adds it.
    let (run, content) = env.giro_run();
    let irregular = GROCERY.replacen('\n', " #irregular\n", 1);
    std::fs::write(&run, content.replace(GROCERY, &irregular)).unwrap();
    let run = run.strip_prefix(&env.repo).unwrap().display().to_string();
    let line = content
        .lines()
        .position(|l| l == UTILITIES.trim_end())
        .unwrap()
        + 1;
    let apply = |extra: &[&str]| {
        let mut args = vec!["-C", repo, "rules", "apply"];
        args.extend(extra);
        env.lentil(&args)
    };

    let out = ok(&apply(&["--dry-run"]));
    assert_eq!(
        out,
        format!(
            "The rules would change these transactions (tags: 2):\n  2026-10-07 City Utilities | Electricity 10/2026  ({run}:{line})\n    + #utilities\n    + #recurring\nDry run on the local files: nothing was saved.\n"
        )
    );

    env.git(&env.repo, &["add", "tags.toml"]);
    env.admin_push("admin: tag rules");
    assert_eq!(
        ok(&apply(&["--only", "payee"])),
        "The rules change nothing.\n"
    );

    let out = ok(&apply(&[]));
    assert!(
        out.starts_with("The rules changed these transactions (tags: 2):\n"),
        "{out}"
    );
    let log = env.last_commit();
    assert!(log.starts_with("editor: add-tag 2\n"), "{log}");
    let (_, content) = env.giro_run();
    assert!(
        content.contains(
            "2026-10-07 ! \"City Utilities\" \"Electricity 10/2026\" #utilities #recurring\n"
        ),
        "{content}"
    );
    assert!(content.contains(&irregular), "{content}");
    assert_eq!(ok(&apply(&[])), "The rules change nothing.\n");

    // A broken tags.toml stops rules apply, but never the import.
    std::fs::write(env.repo.join("tags.toml"), "tags = \"recurring\"\n").unwrap();
    env.admin_push("admin: break tags.toml");
    let out = apply(&[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("tags.toml is invalid: "), "{stderr}");
    env.import_giro(&export);
}

#[test]
fn rules_apply_skips_conflicting_changes_and_commits_the_rest() {
    let env = Env::new();
    let repo = env.repo.to_str().unwrap();
    ok(&env.lentil(&["init", repo, "--remote", env.remote.to_str().unwrap()]));
    let export = env.add_giro_source();
    env.import_giro(&export);
    // `Expenses:Home` is never opened.
    env.append(
        "rules.toml",
        r#"
[[rule]]
match = { counterparty = "^Corner Grocery$" }
account = "Expenses:Food"

[[rule]]
match = { counterparty = "^City Utilities$" }
account = "Expenses:Home"
"#,
    );
    env.append("accounts.beancount", "2000-01-01 open Expenses:Food\n");
    env.admin_push("admin: new rules");
    let (run, content) = env.giro_run();
    let run = run.strip_prefix(&env.repo).unwrap().display().to_string();
    let line = content
        .lines()
        .position(|l| l == UTILITIES.trim_end())
        .unwrap()
        + 1;
    let skipped = format!(
        "  2026-10-07 City Utilities | Electricity 10/2026  ({run}:{line})\n    account: Expenses:Uncategorized → Expenses:Home: the account is not open\n"
    );
    let apply = |extra: &[&str]| {
        let mut args = vec!["-C", repo, "rules", "apply"];
        args.extend(extra);
        let out = env.lentil(&args);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        (out.status.success(), stdout, stderr)
    };

    // The dry run already tells.
    let (success, stdout, stderr) = apply(&["--dry-run"]);
    assert!(!success);
    assert!(
        stdout.starts_with("The rules would change these transactions (account: 1):\n"),
        "{stdout}"
    );
    assert_eq!(
        stderr,
        format!("These changes would be skipped (1):\n{skipped}")
    );

    let before = env.remote_head();
    let (success, stdout, stderr) = apply(&[]);
    assert!(!success);
    assert!(
        stdout.starts_with("The rules changed these transactions (account: 1):\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("    account: Expenses:Uncategorized → Expenses:Food\n"),
        "{stdout}"
    );
    assert_eq!(
        stderr,
        format!("These changes were skipped (1):\n{skipped}")
    );
    assert!(env.last_commit().starts_with("editor: set-account 1\n"));
    let count = env.git(
        &env.remote,
        &["rev-list", "--count", &format!("{}..main", before.trim())],
    );
    assert_eq!(count.trim(), "1");
    // The account changed, the header and the flag did not.
    let (_, content) = env.giro_run();
    assert!(content.contains(GROCERY), "{content}");
    assert!(content.contains(UTILITIES), "{content}");
    assert!(content.contains("  Expenses:Food\n"), "{content}");

    // Only the conflicting change is left: no commit, and still a failure.
    let before = env.remote_head();
    let (success, stdout, stderr) = apply(&[]);
    assert!(!success);
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        format!("These changes were skipped (1):\n{skipped}")
    );
    assert_eq!(before, env.remote_head());
}
