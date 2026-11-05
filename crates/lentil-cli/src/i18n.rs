//! The CLI's message catalog: English, with German as the first translation.
//!
//! The core reports typed codes; this module turns them and the CLI's own
//! messages into text. Templates use `{name}` placeholders.

use lentil_core::finding::{Finding, FindingCode, Severity};
use lentil_core::import::{ImportError, InputProblem, MappingError, RulesError};
use lentil_core::intent::ConflictCode;
use lentil_core::sync::GitError;
use lentil_core::tags::{TagWarning, TaxonomyError};

/// A UI language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    /// English, the default.
    En,
    /// German.
    De,
}

impl Lang {
    /// The language from `LC_ALL`, `LC_MESSAGES` or `LANG`, English by default.
    pub fn from_env() -> Self {
        let value = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .unwrap_or_default();
        Self::from_locale(&value)
    }

    /// The language of a locale string such as `de_DE.UTF-8`.
    pub fn from_locale(locale: &str) -> Self {
        if locale.starts_with("de") {
            Self::De
        } else {
            Self::En
        }
    }
}

/// Every message the CLI prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    InitDone,
    InitExists,
    InitNotEmpty,
    InitPushed,
    NoRepository,
    ConfigMissing,
    UnknownSource,
    MappingMissing,
    MappingInvalid,
    RulesInvalid,
    TagsInvalid,
    RuleAccountNotOpen,
    ExportUnreadable,
    ReadOnly,
    Blocked,
    TooManyAttempts,
    NoAuthor,
    ImportSummary,
    ImportSkippedExisting,
    ImportSkippedRunDate,
    ImportSkippedRows,
    ImportOpening,
    ImportNothingNew,
    ImportCommitted,
    BalanceAssertion,
    BalanceNeedsInput,
    BalanceUnchecked,
    BalanceSuspect,
    BalanceExists,
    BalanceExistsDiffers,
    PromptBalance,
    PromptBalanceDate,
    InvalidAmount,
    InvalidDate,
    DuplicateHeader,
    DuplicateNew,
    DuplicateExisting,
    PromptImportAnyway,
    DuplicateSkipped,
    RulesChanged,
    RulesWouldChange,
    RulesTransaction,
    RulesChange,
    RulesSkipped,
    RulesWouldSkip,
    RulesConflict,
    RulesTag,
    RulesTagConflict,
    StaleTagsHeader,
    StaleTag,
    PromptRemoveTag,
    StaleTagsKept,
    StaleTagsNone,
    StaleTagsNeedTags,
    TagsRemoved,
    RulesNothing,
    RulesWouldBlock,
    RulesDryRun,
    FieldPayee,
    FieldAccount,
    FieldTags,
    NoPayee,
    StatusOpen,
    StatusOpenMore,
    StatusFailed,
    StatusUnchecked,
    StatusClean,
    CheckOk,
    CheckFindings,
    ReferenceAgree,
    ReferenceDisagree,
    ReferenceOnlyCore,
    ReferenceOnlyReference,
    ReferenceFailed,
    Warning,
    Error,
}

/// The template of a message.
#[allow(clippy::too_many_lines)] // A flat catalog reads best in one place.
pub fn text(lang: Lang, msg: Msg) -> &'static str {
    use Lang::{De, En};
    use Msg::*;
    match (msg, lang) {
        (InitDone, En) => "Created a Lentil repository in {dir}.",
        (InitDone, De) => "Lentil-Repository in {dir} angelegt.",
        (InitExists, En) => "{dir} exists already.",
        (InitExists, De) => "{dir} existiert bereits.",
        (InitNotEmpty, En) => {
            "The repository at {dir} or its remote has commits already; init needs an empty one."
        }
        (InitNotEmpty, De) => {
            "Das Repository in {dir} oder sein Remote hat bereits Commits; init braucht ein leeres."
        }
        (InitPushed, En) => "Uploaded to {remote}.",
        (InitPushed, De) => "Nach {remote} hochgeladen.",
        (NoRepository, En) => "No Lentil repository found at {dir} or above.",
        (NoRepository, De) => "Kein Lentil-Repository in {dir} oder darüber gefunden.",
        (ConfigMissing, En) => "The repository has no valid lentil.toml: {detail}",
        (ConfigMissing, De) => "Das Repository hat keine gültige lentil.toml: {detail}",
        (UnknownSource, En) => "lentil.toml has no source {source}.",
        (UnknownSource, De) => "lentil.toml enthält keine Quelle {source}.",
        (MappingMissing, En) => "Mapping file {path} not found.",
        (MappingMissing, De) => "Mapping-Datei {path} nicht gefunden.",
        (MappingInvalid, En) => "Mapping file {path} is invalid: {detail}",
        (MappingInvalid, De) => "Mapping-Datei {path} ist ungültig: {detail}",
        (RulesInvalid, En) => "rules.toml is invalid: {detail}",
        (RulesInvalid, De) => "rules.toml ist ungültig: {detail}",
        (TagsInvalid, En) => "tags.toml is invalid: {detail}",
        (TagsInvalid, De) => "tags.toml ist ungültig: {detail}",
        (RuleAccountNotOpen, En) => {
            "rules.toml: rule {rule} books to {account}, which is not open today."
        }
        (RuleAccountNotOpen, De) => {
            "rules.toml: Regel {rule} bucht auf {account}, das heute nicht eröffnet ist."
        }
        (ExportUnreadable, En) => "Cannot read the export: {detail}",
        (ExportUnreadable, De) => "Der Export lässt sich nicht lesen: {detail}",
        (ReadOnly, En) => "The repository is read-only for this command:",
        (ReadOnly, De) => "Das Repository ist für diesen Befehl schreibgeschützt:",
        (Blocked, En) => "Nothing was saved, because the result would be invalid:",
        (Blocked, De) => "Nichts wurde gespeichert, weil das Ergebnis ungültig wäre:",
        (TooManyAttempts, En) => "Others kept changing the repository; try again later.",
        (TooManyAttempts, De) => {
            "Andere haben das Repository laufend geändert; bitte später erneut versuchen."
        }
        (NoAuthor, En) => "Set your name and e-mail first: git config user.name / user.email",
        (NoAuthor, De) => "Bitte zuerst Name und E-Mail setzen: git config user.name / user.email",
        (ImportSummary, En) => "{count} new transactions from {source}.",
        (ImportSummary, De) => "{count} neue Buchungen aus {source}.",
        (ImportSkippedExisting, En) => "{count} already imported, skipped.",
        (ImportSkippedExisting, De) => "{count} bereits importiert, übersprungen.",
        (ImportSkippedRunDate, En) => "{count} from today or later, left for the next import.",
        (ImportSkippedRunDate, De) => {
            "{count} von heute oder später, bleiben für den nächsten Import."
        }
        (ImportSkippedRows, En) => "{count} rows skipped by the mapping.",
        (ImportSkippedRows, De) => "{count} Zeilen laut Mapping übersprungen.",
        (ImportOpening, En) => "First import of this account: opening balance {amount}.",
        (ImportOpening, De) => "Erster Import dieses Kontos: Anfangsbestand {amount}.",
        (ImportNothingNew, En) => "Nothing new to import.",
        (ImportNothingNew, De) => "Nichts Neues zu importieren.",
        (ImportCommitted, En) => "Saved as {file}.",
        (ImportCommitted, De) => "Gespeichert als {file}.",
        (BalanceAssertion, En) => "Balance check: {amount} at the start of {date}.",
        (BalanceAssertion, De) => "Saldoprüfung: {amount} zu Beginn des {date}.",
        (BalanceNeedsInput, En) => "The export contains no balance.",
        (BalanceNeedsInput, De) => "Der Export enthält keinen Saldo.",
        (BalanceUnchecked, En) => {
            "No balance given: lentil status reports this import as unchecked."
        }
        (BalanceUnchecked, De) => {
            "Kein Saldo angegeben: lentil status meldet diesen Import als ungeprüft."
        }
        (BalanceSuspect, En) => {
            "The running balance in the export does not add up; the mapping may be wrong. No balance check written."
        }
        (BalanceSuspect, De) => {
            "Der laufende Saldo im Export geht nicht auf; das Mapping ist womöglich falsch. Keine Saldoprüfung geschrieben."
        }
        (BalanceExists, En) => "A balance check for {date} exists already.",
        (BalanceExists, De) => "Für den {date} gibt es bereits eine Saldoprüfung.",
        (BalanceExistsDiffers, En) => {
            "A balance check for {date} exists already and differs by {difference}."
        }
        (BalanceExistsDiffers, De) => {
            "Für den {date} gibt es bereits eine Saldoprüfung; sie weicht um {difference} ab."
        }
        (PromptBalance, En) => "Balance shown in online banking (empty to skip): ",
        (PromptBalance, De) => "Saldo laut Online-Banking (leer zum Überspringen): ",
        (PromptBalanceDate, En) => "Date of that balance [{date}]: ",
        (PromptBalanceDate, De) => "Datum dieses Saldos [{date}]: ",
        (InvalidAmount, En) => "Not an amount: {value}",
        (InvalidAmount, De) => "Kein Betrag: {value}",
        (InvalidDate, En) => "Not a date (YYYY-MM-DD): {value}",
        (InvalidDate, De) => "Kein Datum (JJJJ-MM-TT): {value}",
        (DuplicateHeader, En) => {
            "Possible duplicate: the export no longer contains this transaction, but a new one with the same date and amount:"
        }
        (DuplicateHeader, De) => {
            "Mögliches Duplikat: Der Export enthält diese Buchung nicht mehr, aber eine neue mit gleichem Datum und Betrag:"
        }
        (DuplicateExisting, En) => "  existing: {date} {amount} {desc} ({file}:{line})",
        (DuplicateExisting, De) => "  vorhanden: {date} {amount} {desc} ({file}:{line})",
        (DuplicateNew, En) => "  new:      {date} {amount} {desc} (export line {line})",
        (DuplicateNew, De) => "  neu:       {date} {amount} {desc} (Exportzeile {line})",
        (PromptImportAnyway, En) => "Import the new one anyway? [y/N] ",
        (PromptImportAnyway, De) => "Die neue trotzdem importieren? [j/N] ",
        (DuplicateSkipped, En) => "{count} possible duplicates not imported.",
        (DuplicateSkipped, De) => "{count} mögliche Duplikate nicht importiert.",
        (RulesChanged, En) => "The rules changed these transactions ({counts}):",
        (RulesChanged, De) => "Die Regeln haben diese Buchungen geändert ({counts}):",
        (RulesWouldChange, En) => "The rules would change these transactions ({counts}):",
        (RulesWouldChange, De) => "Die Regeln würden diese Buchungen ändern ({counts}):",
        (RulesTransaction, En | De) => "  {date} {text}  ({file}:{line})",
        (RulesChange, En | De) => "    {field}: {old} → {new}",
        (RulesSkipped, En) => "These changes were skipped ({count}):",
        (RulesSkipped, De) => "Diese Änderungen wurden übersprungen ({count}):",
        (RulesWouldSkip, En) => "These changes would be skipped ({count}):",
        (RulesWouldSkip, De) => "Diese Änderungen würden übersprungen ({count}):",
        (RulesConflict, En | De) => "    {field}: {old} → {new}: {reason}",
        (RulesTag, En | De) => "    {sign} #{tag}",
        (RulesTagConflict, En | De) => "    {sign} #{tag}: {reason}",
        (StaleTagsHeader, En) => "The rules no longer yield these tags ({count}):",
        (StaleTagsHeader, De) => "Diese Tags ergeben die Regeln nicht mehr ({count}):",
        (StaleTag, En | De) => "    #{tag}",
        (PromptRemoveTag, En) => "    Remove #{tag}? [y/N] ",
        (PromptRemoveTag, De) => "    #{tag} entfernen? [j/N] ",
        (StaleTagsKept, En) => {
            "Kept: tags are removed only after a confirmation in a terminal, without --yes."
        }
        (StaleTagsKept, De) => {
            "Behalten: Tags werden nur nach einer Bestätigung im Terminal entfernt, ohne --yes."
        }
        (StaleTagsNone, En) => "No tag to remove.",
        (StaleTagsNone, De) => "Kein Tag zu entfernen.",
        (StaleTagsNeedTags, En) => {
            "--stale-tags looks at tags, so it cannot be combined with --only payee or --only account"
        }
        (StaleTagsNeedTags, De) => {
            "--stale-tags betrifft Tags und lässt sich daher nicht mit --only payee oder --only account verbinden"
        }
        (TagsRemoved, En) => "These tags were removed ({count}):",
        (TagsRemoved, De) => "Diese Tags wurden entfernt ({count}):",
        (RulesNothing, En) => "The rules change nothing.",
        (RulesNothing, De) => "Die Regeln ändern nichts.",
        (RulesWouldBlock, En) => {
            "A real run would save nothing, because the result would be invalid:"
        }
        (RulesWouldBlock, De) => {
            "Ein echter Lauf würde nichts speichern, weil das Ergebnis ungültig wäre:"
        }
        (RulesDryRun, En) => "Dry run on the local files: nothing was saved.",
        (RulesDryRun, De) => "Probelauf mit den lokalen Dateien: Nichts wurde gespeichert.",
        (FieldPayee, En) => "payee",
        (FieldPayee, De) => "Empfänger",
        (FieldAccount, En) => "account",
        (FieldAccount, De) => "Konto",
        (FieldTags, En) => "tags",
        (FieldTags, De) => "Tags",
        (NoPayee, En) => "(none)",
        (NoPayee, De) => "(keiner)",
        (StatusOpen, En) => "Open transactions: {count}",
        (StatusOpen, De) => "Offene Buchungen: {count}",
        (StatusOpenMore, En) => "  … and {count} more",
        (StatusOpenMore, De) => "  … und {count} weitere",
        (StatusFailed, En) => "Failed balance checks: {count}",
        (StatusFailed, De) => "Fehlgeschlagene Saldoprüfungen: {count}",
        (StatusUnchecked, En) => "Imports without a balance check: {count}",
        (StatusUnchecked, De) => "Importe ohne Saldoprüfung: {count}",
        (StatusClean, En) => "Nothing open.",
        (StatusClean, De) => "Nichts offen.",
        (CheckOk, En) => "The ledger is valid.",
        (CheckOk, De) => "Das Ledger ist gültig.",
        (CheckFindings, En) => "{blocking} errors, {warnings} warnings.",
        (CheckFindings, De) => "{blocking} Fehler, {warnings} Warnungen.",
        (ReferenceAgree, En) => "bean-check agrees.",
        (ReferenceAgree, De) => "bean-check stimmt überein.",
        (ReferenceDisagree, En) => "bean-check disagrees with the core:",
        (ReferenceDisagree, De) => "bean-check weicht vom Core ab:",
        (ReferenceOnlyCore, En) => "  only the core reports {file}:{line}",
        (ReferenceOnlyCore, De) => "  nur der Core meldet {file}:{line}",
        (ReferenceOnlyReference, En) => "  only bean-check reports {file}:{line}: {detail}",
        (ReferenceOnlyReference, De) => "  nur bean-check meldet {file}:{line}: {detail}",
        (ReferenceFailed, En) => "Cannot run {command}: {detail}",
        (ReferenceFailed, De) => "{command} lässt sich nicht ausführen: {detail}",
        (Warning, En) => "warning",
        (Warning, De) => "Warnung",
        (Error, En) => "error",
        (Error, De) => "Fehler",
    }
}

/// Fills `{name}` placeholders.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// A message with arguments.
pub fn msg(lang: Lang, msg: Msg, args: &[(&str, &str)]) -> String {
    fill(text(lang, msg), args)
}

/// The template for a finding code. Arguments come from [`Finding::args`].
#[allow(clippy::too_many_lines)]
pub fn finding_template(lang: Lang, code: FindingCode) -> &'static str {
    use FindingCode::*;
    use Lang::{De, En};
    match (code, lang) {
        (ConfigInvalid, En) => "lentil.toml is missing or invalid: {detail}",
        (ConfigInvalid, De) => "lentil.toml fehlt oder ist ungültig: {detail}",
        (SpecVersionUnsupported, En) => {
            "The repository uses spec version {found}; this client supports {supported}."
        }
        (SpecVersionUnsupported, De) => {
            "Das Repository nutzt Spec-Version {found}; dieser Client unterstützt {supported}."
        }
        (ParseError, En) => "Cannot read the file: {detail}",
        (ParseError, De) => "Datei nicht lesbar: {detail}",
        (TransactionUnbalanced, En) => "A transaction does not balance: {detail}",
        (TransactionUnbalanced, De) => "Eine Buchung ist nicht ausgeglichen: {detail}",
        (AccountNotOpen, En) => "Account {account} is not open on that date.",
        (AccountNotOpen, De) => "Konto {account} ist an diesem Datum nicht eröffnet.",
        (DuplicateLentilId, En) => "Two transactions share the ID {id}.",
        (DuplicateLentilId, De) => "Zwei Buchungen haben dieselbe ID {id}.",
        (OutsideWriteArea, En) => "The {role} role must not change {path} ({reason}).",
        (OutsideWriteArea, De) => "Die Rolle {role} darf {path} nicht ändern ({reason}).",
        (OutsideProfile, En) => "Not supported by Lentil yet: {construct}",
        (OutsideProfile, De) => "Von Lentil noch nicht unterstützt: {construct}",
        (FavaFileMismatch, En) => "fava.beancount may only hold Fava options and sidebar links.",
        (FavaFileMismatch, De) => {
            "fava.beancount darf nur Fava-Optionen und Seitenleisten-Links enthalten."
        }
        (MainFileMismatch, En) => "main.beancount differs from the convention: {detail}",
        (MainFileMismatch, De) => "main.beancount weicht von der Konvention ab: {detail}",
        (IncludeOutsideRepository, En) => "An include points outside the repository: {include}",
        (IncludeOutsideRepository, De) => "Ein include zeigt aus dem Repository heraus: {include}",
        (MissingZeroTolerance, En) => {
            "The new balance check for {account} lacks the zero tolerance (~ 0.00)."
        }
        (MissingZeroTolerance, De) => {
            "Der neuen Saldoprüfung für {account} fehlt die Nulltoleranz (~ 0.00)."
        }
        (ManualPostingOnSourceAccount, En) => {
            "A manual entry posts to the imported account {account}."
        }
        (ManualPostingOnSourceAccount, De) => {
            "Eine manuelle Buchung bucht auf das importierte Konto {account}."
        }
        (PadOnSourceAccount, En) => "A pad posts to the imported account {account}.",
        (PadOnSourceAccount, De) => "Ein pad bucht auf das importierte Konto {account}.",
        (ProtectedMetadataChanged, En) => "{key} of transaction {id} must not change.",
        (ProtectedMetadataChanged, De) => "{key} der Buchung {id} darf sich nicht ändern.",
        (ProtectedTransactionChanged, En) => {
            "Date, amount or bank posting of transaction {id} must not change."
        }
        (ProtectedTransactionChanged, De) => {
            "Datum, Betrag oder Bankbuchung der Buchung {id} dürfen sich nicht ändern."
        }
        (StrayLedgerFile, En) => "{path} is not picked up by main.beancount.",
        (StrayLedgerFile, De) => "{path} wird von main.beancount nicht erfasst.",
        (InvalidPath, En) => "{path} breaks the naming rules (lowercase a-z, 0-9, -).",
        (InvalidPath, De) => "{path} verletzt die Namensregeln (klein a-z, 0-9, -).",
        (LoadCacheCommitted, En) => "The load cache {path} must not be committed.",
        (LoadCacheCommitted, De) => "Der Lade-Cache {path} darf nicht committet werden.",
        (CoreError, En) => "{code}: {detail}",
        (CoreError, De) => "{code}: {detail}",
        (BalanceAssertionFailed, En) => {
            "Balance check failed: {account} on {date} expected {asserted} {currency}, difference {difference} {currency}."
        }
        (BalanceAssertionFailed, De) => {
            "Saldoprüfung fehlgeschlagen: {account} am {date} erwartet {asserted} {currency}, Differenz {difference} {currency}."
        }
    }
}

/// A finding as one line: `file:line: error: message`.
pub fn finding(lang: Lang, finding: &Finding) -> String {
    let args: Vec<(&str, &str)> = finding
        .args
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let text = fill(finding_template(lang, finding.code), &args);
    let severity = match finding.severity {
        Severity::Blocking => self::text(lang, Msg::Error),
        Severity::Warning => self::text(lang, Msg::Warning),
    };
    match (&finding.file, finding.line) {
        (Some(file), Some(line)) => format!("{file}:{line}: {severity}: {text}"),
        (Some(file), None) => format!("{file}: {severity}: {text}"),
        _ => format!("{severity}: {text}"),
    }
}

/// An import error as text.
pub fn import_error(lang: Lang, error: &ImportError) -> String {
    use ImportError::*;
    let (en, de, args): (&str, &str, Vec<(&str, String)>) = match error {
        Encoding(enc) => (
            "the file is not valid {encoding}",
            "die Datei ist kein gültiges {encoding}",
            vec![("encoding", enc.clone())],
        ),
        HeaderNotFound => (
            "the header row was not found; the export no longer matches the mapping",
            "die Kopfzeile wurde nicht gefunden; der Export passt nicht mehr zum Mapping",
            vec![],
        ),
        ColumnNotFound(column) => (
            "column {column} not found",
            "Spalte {column} nicht gefunden",
            vec![("column", column.clone())],
        ),
        RowInvalid { line, field, value } => (
            "line {line}: cannot read {field} from {value}",
            "Zeile {line}: {field} lässt sich aus {value} nicht lesen",
            vec![
                ("line", line.to_string()),
                ("field", field.clone()),
                ("value", format!("{value:?}")),
            ],
        ),
        Malformed { line } => (
            "line {line}: malformed CSV",
            "Zeile {line}: fehlerhaftes CSV",
            vec![("line", line.to_string())],
        ),
        DuplicateReference {
            line,
            first_line,
            reference,
        } => (
            "line {line}: reference {reference} appears already in line {first}",
            "Zeile {line}: Referenz {reference} steht bereits in Zeile {first}",
            vec![
                ("line", line.to_string()),
                ("first", first_line.to_string()),
                ("reference", reference.clone()),
            ],
        ),
        BalanceNotFound => (
            "the balance text was not found",
            "der Saldotext wurde nicht gefunden",
            vec![],
        ),
    };
    let template = match lang {
        Lang::En => en,
        Lang::De => de,
    };
    let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    fill(template, &args)
}

/// Why `rules.toml` is rejected, as text. A syntax error keeps the TOML
/// parser's own message.
pub fn rules_error(lang: Lang, error: &RulesError) -> String {
    use RulesError::*;
    let (en, de, args): (&str, &str, Vec<(&str, String)>) = match error {
        Syntax(detail) => ("{detail}", "{detail}", vec![("detail", detail.clone())]),
        NotUtf8 => (
            "the file is not UTF-8 text",
            "die Datei ist kein UTF-8-Text",
            vec![],
        ),
        Regex(pattern) => (
            "invalid regular expression {pattern}",
            "ungültiger regulärer Ausdruck {pattern}",
            vec![("pattern", format!("{pattern:?}"))],
        ),
        NoCondition(rule) => (
            "rule {rule} has neither a regular expression nor tags in match",
            "Regel {rule} hat in match weder einen regulären Ausdruck noch Tags",
            vec![("rule", rule.to_string())],
        ),
        NoOutput(rule) => (
            "rule {rule} sets no payee, account or add_tags",
            "Regel {rule} setzt weder payee noch account noch add_tags",
            vec![("rule", rule.to_string())],
        ),
        EmptyTags(rule) => (
            "rule {rule} has an empty list of tags",
            "Regel {rule} hat eine leere Liste von Tags",
            vec![("rule", rule.to_string())],
        ),
        InvalidTag { rule, tag } => (
            "rule {rule} has the tag {tag}, which is not a valid tag",
            "Regel {rule} hat den Tag {tag}, der kein gültiger Tag ist",
            vec![("rule", rule.to_string()), ("tag", format!("{tag:?}"))],
        ),
        TagMatchSetsField(rule) => (
            "rule {rule} matches tags, so it may only add tags",
            "Regel {rule} prüft Tags und darf daher nur Tags hinzufügen",
            vec![("rule", rule.to_string())],
        ),
        EmptyPayee(rule) => (
            "rule {rule} has an empty payee",
            "Regel {rule} hat einen leeren payee",
            vec![("rule", rule.to_string())],
        ),
        InvalidAccount(account) => (
            "{account} is not a valid account name",
            "{account} ist kein gültiger Kontoname",
            vec![("account", account.clone())],
        ),
        SourceAccount(account) => (
            "{account} is a bank account and cannot be a rule's account",
            "{account} ist ein Bankkonto und kann nicht das Konto einer Regel sein",
            vec![("account", account.clone())],
        ),
        UnknownSource { rule, id } => (
            "rule {rule} names the source {source}, which lentil.toml does not define",
            "Regel {rule} nennt die Quelle {source}, die lentil.toml nicht enthält",
            vec![("rule", rule.to_string()), ("source", format!("{id:?}"))],
        ),
    };
    let template = match lang {
        Lang::En => en,
        Lang::De => de,
    };
    let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    fill(template, &args)
}

/// Why `tags.toml` is rejected, as text. A syntax error keeps the TOML
/// parser's own message.
pub fn taxonomy_error(lang: Lang, error: &TaxonomyError) -> String {
    match (error, lang) {
        (TaxonomyError::Syntax(detail), _) => detail.clone(),
        (TaxonomyError::NotUtf8, Lang::En) => "the file is not UTF-8 text".to_owned(),
        (TaxonomyError::NotUtf8, Lang::De) => "die Datei ist kein UTF-8-Text".to_owned(),
        (TaxonomyError::InvalidTag(tag), Lang::En) => format!("{tag:?} is not a valid tag"),
        (TaxonomyError::InvalidTag(tag), Lang::De) => format!("{tag:?} ist kein gültiger Tag"),
    }
}

/// A rejected mapping file as text. Only the TOML parser's own message stays
/// untranslated.
pub fn mapping_error(lang: Lang, error: &MappingError) -> String {
    use MappingError::*;
    let (en, de, args): (&str, &str, Vec<(&str, String)>) = match error {
        Syntax(detail) => ("{detail}", "{detail}", vec![("detail", detail.clone())]),
        NotUtf8 => (
            "the file is not UTF-8 text",
            "die Datei ist kein UTF-8-Text",
            vec![],
        ),
        Regex(pattern) => (
            "invalid regular expression {pattern}",
            "ungültiger regulärer Ausdruck {pattern}",
            vec![("pattern", format!("{pattern:?}"))],
        ),
        Encoding(encoding) => (
            "unsupported encoding {encoding}",
            "nicht unterstützte Kodierung {encoding}",
            vec![("encoding", format!("{encoding:?}"))],
        ),
        DateFormat(format) => (
            "unsupported date format {format}",
            "nicht unterstütztes Datumsformat {format}",
            vec![("format", format!("{format:?}"))],
        ),
        Delimiter => (
            "delimiter must be one ASCII character",
            "delimiter muss ein einzelnes ASCII-Zeichen sein",
            vec![],
        ),
        EmptyHeader => (
            "header needs at least one non-empty cell",
            "header braucht mindestens eine nicht leere Zelle",
            vec![],
        ),
        NotOneCharacter(key) => (
            "{key} must be one character",
            "{key} muss ein einzelnes Zeichen sein",
            vec![("key", (*key).to_owned())],
        ),
        SameSeparator => (
            "thousands and decimal must differ",
            "thousands und decimal müssen sich unterscheiden",
            vec![],
        ),
        DigitOrSign(key) => (
            "{key} must not be a digit or sign",
            "{key} darf keine Ziffer und kein Vorzeichen sein",
            vec![("key", (*key).to_owned())],
        ),
        AmountColumns => (
            "amount needs either column or both debit and credit",
            "amount braucht entweder column oder debit und credit",
            vec![],
        ),
        SkipCondition => (
            "skip needs either equals or matches",
            "skip braucht entweder equals oder matches",
            vec![],
        ),
        EmptyColumnList(key) => (
            "{key} needs at least one column",
            "{key} braucht mindestens eine Spalte",
            vec![("key", (*key).to_owned())],
        ),
        BalanceAmountGroup => (
            "balance pattern needs a group named amount",
            "balance pattern braucht eine Gruppe namens amount",
            vec![],
        ),
        BalanceDateFormatMissing => (
            "balance group date needs date_format",
            "die balance-Gruppe date braucht date_format",
            vec![],
        ),
        BalanceDateGroupMissing => (
            "balance date_format needs a group named date",
            "balance date_format braucht eine Gruppe namens date",
            vec![],
        ),
        BalanceColumnKeys => (
            "balance from column takes exactly the key column",
            "balance from column nimmt genau den Schlüssel column",
            vec![],
        ),
        BalanceTextKeys => (
            "balance from text takes pattern and date_format only",
            "balance from text nimmt nur pattern und date_format",
            vec![],
        ),
        BalanceNoneKeys => (
            "balance from none takes no other key",
            "balance from none nimmt keinen weiteren Schlüssel",
            vec![],
        ),
    };
    let template = match lang {
        Lang::En => en,
        Lang::De => de,
    };
    let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    fill(template, &args)
}

/// A problem with `rules.toml` or a mapping file, naming the file.
pub fn input_problem(lang: Lang, problem: &InputProblem) -> String {
    match problem {
        InputProblem::Rules(error) => msg(
            lang,
            Msg::RulesInvalid,
            &[("detail", &rules_error(lang, error))],
        ),
        InputProblem::RuleAccountNotOpen { rule, account } => msg(
            lang,
            Msg::RuleAccountNotOpen,
            &[("rule", &rule.to_string()), ("account", account)],
        ),
        InputProblem::Taxonomy(error) => msg(
            lang,
            Msg::TagsInvalid,
            &[("detail", &taxonomy_error(lang, error))],
        ),
        InputProblem::MappingMissing(path) => msg(lang, Msg::MappingMissing, &[("path", path)]),
        InputProblem::Mapping { path, error } => msg(
            lang,
            Msg::MappingInvalid,
            &[("path", path), ("detail", &mapping_error(lang, error))],
        ),
    }
}

/// A tag outside `tags.toml` as a warning, located like a finding.
pub fn tag_warning(lang: Lang, warning: &TagWarning) -> String {
    let severity = text(lang, Msg::Warning);
    match (warning, lang) {
        (TagWarning::UnknownTag { file, line, tag }, Lang::En) => {
            format!("{file}:{line}: {severity}: #{tag} is not in tags.toml.")
        }
        (TagWarning::UnknownTag { file, line, tag }, Lang::De) => {
            format!("{file}:{line}: {severity}: #{tag} steht nicht in tags.toml.")
        }
        (TagWarning::ExclusiveTags { file, line, tags }, _) => {
            let tags = tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(", ");
            match lang {
                Lang::En => {
                    format!("{file}:{line}: {severity}: {tags} exclude each other in tags.toml.")
                }
                Lang::De => format!(
                    "{file}:{line}: {severity}: {tags} schließen sich laut tags.toml gegenseitig aus."
                ),
            }
        }
        (TagWarning::UnknownRuleTag { rule, tag }, Lang::En) => {
            format!("rules.toml: {severity}: Rule {rule} uses #{tag}, which is not in tags.toml.")
        }
        (TagWarning::UnknownRuleTag { rule, tag }, Lang::De) => {
            format!(
                "rules.toml: {severity}: Regel {rule} nutzt #{tag}, der nicht in tags.toml steht."
            )
        }
    }
}

/// A conflict code as text.
pub fn conflict(lang: Lang, code: ConflictCode) -> &'static str {
    use ConflictCode::*;
    use Lang::{De, En};
    match (code, lang) {
        (TransactionNotFound, En) => "the transaction no longer exists",
        (TransactionNotFound, De) => "die Buchung existiert nicht mehr",
        (TransactionDismissed, En) => "the transaction was dismissed",
        (TransactionDismissed, De) => "die Buchung wurde verworfen",
        (AccountChanged, En) => "the account was changed elsewhere",
        (AccountChanged, De) => "das Konto wurde anderswo geändert",
        (PayeeChanged, En) => "the payee was changed elsewhere",
        (PayeeChanged, De) => "der Empfänger wurde anderswo geändert",
        (PostingNotFound, En) => "the posting no longer exists",
        (PostingNotFound, De) => "die Teilbuchung existiert nicht mehr",
        (SourcePosting, En) => {
            "a bank posting cannot be changed, and no posting can move onto a bank account"
        }
        (SourcePosting, De) => {
            "eine Bankbuchung lässt sich nicht ändern, und keine Buchung kann auf ein Bankkonto wechseln"
        }
        (AccountNotOpen, En) => "the account is not open",
        (AccountNotOpen, De) => "das Konto ist nicht eröffnet",
        (NotEditable, En) => "the transaction cannot be edited",
        (NotEditable, De) => "die Buchung lässt sich nicht bearbeiten",
        (FileExists, En) => "the file exists already",
        (FileExists, De) => "die Datei existiert bereits",
        (InvalidAccount, En) => "the account name is invalid",
        (InvalidAccount, De) => "der Kontoname ist ungültig",
        (InvalidTag, En) => "the tag is not a valid tag",
        (InvalidTag, De) => "der Tag ist kein gültiger Tag",
        (InvalidTrip, En) => "the trip's name or ID is invalid",
        (InvalidTrip, De) => "Name oder ID der Reise sind ungültig",
    }
}

/// A Git error in words without Git terminology.
pub fn git_error(lang: Lang, error: &GitError) -> String {
    let (en, de) = match error {
        GitError::Network(_) => ("Cannot reach the server.", "Server nicht erreichbar."),
        GitError::Auth => (
            "Sign-in to the server failed.",
            "Anmeldung am Server fehlgeschlagen.",
        ),
        GitError::NotFound(_) => (
            "The repository was not found on the server.",
            "Das Repository wurde auf dem Server nicht gefunden.",
        ),
        GitError::LocalChanges(_) => (
            "The local copy has changes that are not uploaded yet; commit and push them first.",
            "Die lokale Kopie hat noch nicht hochgeladene Änderungen; bitte erst committen und pushen.",
        ),
        GitError::Other(_) => ("Git failed.", "Git ist fehlgeschlagen."),
    };
    let detail = match error {
        GitError::Network(d)
        | GitError::NotFound(d)
        | GitError::LocalChanges(d)
        | GitError::Other(d) => {
            format!(" ({d})")
        }
        GitError::Auth => String::new(),
    };
    match lang {
        Lang::En => format!("{en}{detail}"),
        Lang::De => format!("{de}{detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_german_from_locale() {
        assert_eq!(Lang::from_locale("de_DE.UTF-8"), Lang::De);
        assert_eq!(Lang::from_locale("en_US.UTF-8"), Lang::En);
        assert_eq!(Lang::from_locale(""), Lang::En);
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(
            msg(
                Lang::En,
                Msg::ImportSummary,
                &[("count", "3"), ("source", "giro")]
            ),
            "3 new transactions from giro."
        );
    }

    #[test]
    fn translates_mapping_errors() {
        assert_eq!(
            mapping_error(Lang::En, &MappingError::DigitOrSign("decimal")),
            "decimal must not be a digit or sign"
        );
        assert_eq!(
            mapping_error(
                Lang::En,
                &MappingError::Syntax("missing field `balance`".into())
            ),
            "missing field `balance`"
        );
        assert_eq!(
            mapping_error(Lang::En, &MappingError::Encoding("utf-16".into())),
            "unsupported encoding \"utf-16\""
        );
        let de = mapping_error(Lang::De, &MappingError::EmptyColumnList("purpose"));
        assert!(de.contains("purpose"), "{de}");
        assert_ne!(
            de,
            mapping_error(Lang::En, &MappingError::EmptyColumnList("purpose"))
        );
    }

    #[test]
    fn translates_rules_errors() {
        assert_eq!(
            msg(
                Lang::De,
                Msg::RulesInvalid,
                &[("detail", &rules_error(Lang::De, &RulesError::NoOutput(2)))]
            ),
            "rules.toml ist ungültig: Regel 2 setzt weder payee noch account noch add_tags"
        );
        let invalid = RulesError::InvalidTag {
            rule: 2,
            tag: "#x".into(),
        };
        assert_eq!(
            rules_error(Lang::En, &invalid),
            "rule 2 has the tag \"#x\", which is not a valid tag"
        );
        assert_eq!(
            rules_error(Lang::De, &invalid),
            "Regel 2 hat den Tag \"#x\", der kein gültiger Tag ist"
        );
        assert_eq!(
            rules_error(Lang::En, &RulesError::EmptyTags(3)),
            "rule 3 has an empty list of tags"
        );
        assert_eq!(
            rules_error(Lang::En, &RulesError::TagMatchSetsField(2)),
            "rule 2 matches tags, so it may only add tags"
        );
        assert_eq!(
            rules_error(Lang::De, &RulesError::TagMatchSetsField(2)),
            "Regel 2 prüft Tags und darf daher nur Tags hinzufügen"
        );
        assert_eq!(
            rules_error(Lang::En, &RulesError::Regex("(?<=x)".into())),
            "invalid regular expression \"(?<=x)\""
        );
        assert_eq!(
            rules_error(Lang::En, &RulesError::Syntax("unknown field `desc`".into())),
            "unknown field `desc`"
        );
        assert_eq!(
            rules_error(
                Lang::En,
                &RulesError::UnknownSource {
                    rule: 2,
                    id: "girro".into()
                }
            ),
            "rule 2 names the source \"girro\", which lentil.toml does not define"
        );
        assert_eq!(
            rules_error(Lang::En, &RulesError::NotUtf8),
            "the file is not UTF-8 text"
        );
    }

    #[test]
    fn translates_tag_warnings_and_taxonomy_errors() {
        let unknown = TagWarning::UnknownTag {
            file: "imports/giro/2026/x.beancount".into(),
            line: 7,
            tag: "grocery".into(),
        };
        let clash = TagWarning::ExclusiveTags {
            file: "manual/k7p2q9xd/2026-10.beancount".into(),
            line: 3,
            tags: vec!["recurring".into(), "irregular".into()],
        };
        let rule = TagWarning::UnknownRuleTag {
            rule: 2,
            tag: "recuring".into(),
        };
        assert_eq!(
            tag_warning(Lang::En, &unknown),
            "imports/giro/2026/x.beancount:7: warning: #grocery is not in tags.toml."
        );
        assert_eq!(
            tag_warning(Lang::De, &unknown),
            "imports/giro/2026/x.beancount:7: Warnung: #grocery steht nicht in tags.toml."
        );
        assert_eq!(
            tag_warning(Lang::En, &clash),
            "manual/k7p2q9xd/2026-10.beancount:3: warning: #recurring, #irregular exclude each other in tags.toml."
        );
        assert_eq!(
            tag_warning(Lang::De, &clash),
            "manual/k7p2q9xd/2026-10.beancount:3: Warnung: #recurring, #irregular schließen sich laut tags.toml gegenseitig aus."
        );
        assert_eq!(
            tag_warning(Lang::En, &rule),
            "rules.toml: warning: Rule 2 uses #recuring, which is not in tags.toml."
        );
        assert_eq!(
            tag_warning(Lang::De, &rule),
            "rules.toml: Warnung: Regel 2 nutzt #recuring, der nicht in tags.toml steht."
        );
        let broken = InputProblem::Taxonomy(TaxonomyError::InvalidTag("#x".into()));
        assert_eq!(
            input_problem(Lang::En, &broken),
            "tags.toml is invalid: \"#x\" is not a valid tag"
        );
        assert_eq!(
            input_problem(Lang::De, &broken),
            "tags.toml ist ungültig: \"#x\" ist kein gültiger Tag"
        );
    }

    #[test]
    fn formats_findings_with_location() {
        let f = Finding::warning(FindingCode::BalanceAssertionFailed)
            .at("imports/giro/2026/x.beancount", Some(7))
            .arg("account", "Assets:Bank:Giro")
            .arg("date", "2026-10-20")
            .arg("asserted", "10.00")
            .arg("difference", "0.01")
            .arg("currency", "EUR");
        assert_eq!(
            finding(Lang::De, &f),
            "imports/giro/2026/x.beancount:7: Warnung: Saldoprüfung fehlgeschlagen: Assets:Bank:Giro am 2026-10-20 erwartet 10.00 EUR, Differenz 0.01 EUR."
        );
    }
}
