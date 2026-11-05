//! Trips: the `event "trip"` entries of spec section 9, and the trip tag an
//! open transaction gets suggested from them.

use jiff::civil::Date;

use crate::config::Config;
use crate::id::manual_ulid;
use crate::import::opening_id;
use crate::ledger::{TripEvent, Txn};
use crate::tags::is_plain_name;

/// The event type that marks trips (spec section 9).
pub const TRIP_EVENT: &str = "trip";

/// The prefix of a trip's tag: the trip `lisbon` has the tag `trip-lisbon`.
pub const TRIP_TAG_PREFIX: &str = "trip-";

/// Days after its end event that a date still counts for a trip, which
/// covers payments the bank books after the trip.
pub const GRACE_DAYS: i32 = 3;

/// The tag of the trip `name`.
#[must_use]
pub fn trip_tag(name: &str) -> String {
    format!("{TRIP_TAG_PREFIX}{name}")
}

/// The trip events of a ledger in the order of spec section 9: by date, then
/// by the ULID of their `lentil_id`, the time they were created, then by file
/// path, then by position in the file. Events whose value is neither
/// empty nor a valid trip name are left out, and so is an end event that
/// does not follow a start: it ends no trip, so it changes nothing, not even
/// the grace days of the end before it.
#[derive(Clone, Debug, Default)]
pub struct Trips {
    /// Each event's date, with the trip's name for a start and `None` for an
    /// end; every end follows a start.
    events: Vec<(Date, Option<String>)>,
}

/// The sort key of spec section 9. Within a date, events sort by the time
/// they were created; an event without a manual ID has `None` and comes first.
fn order(event: &TripEvent) -> (Date, Option<&str>, &str, usize) {
    (
        event.date,
        event.id.as_deref().and_then(manual_ulid),
        &event.file,
        event.offset,
    )
}

impl Trips {
    /// Orders `events`, such as [`crate::ledger::Ledger::trip_events`].
    #[must_use]
    pub fn new(events: &[TripEvent]) -> Self {
        let mut valid: Vec<&TripEvent> = events
            .iter()
            .filter(|e| e.name.is_empty() || is_plain_name(&e.name))
            .collect();
        valid.sort_by(|a, b| order(a).cmp(&order(b)));
        let mut kept: Vec<(Date, Option<String>)> = Vec::with_capacity(valid.len());
        for e in valid {
            let start = !e.name.is_empty();
            if start || matches!(kept.last(), Some((_, Some(_)))) {
                kept.push((e.date, start.then(|| e.name.clone())));
            }
        }
        Self { events: kept }
    }

    /// The trip `date` lies in: the latest event on or before it starts the
    /// trip, or ends it at most [`GRACE_DAYS`] days before.
    #[must_use]
    pub fn trip_on(&self, date: Date) -> Option<&str> {
        let latest = self
            .events
            .partition_point(|(d, _)| *d <= date)
            .checked_sub(1)?;
        match &self.events[latest] {
            (_, Some(name)) => Some(name),
            (end, None)
                if date
                    .since(*end)
                    .is_ok_and(|span| span.get_days() <= GRACE_DAYS) =>
            {
                // Every end follows a start, see `new`.
                self.events.get(latest.checked_sub(1)?)?.1.as_deref()
            }
            (_, None) => None,
        }
    }

    /// The running trip: the name, if the last event starts a trip.
    #[must_use]
    pub fn running(&self) -> Option<&str> {
        self.events.last()?.1.as_deref()
    }

    /// The trip tag suggested for `txn` (spec section 9): an active
    /// transaction with flag `!` and a `lentil_id`, other than a source's
    /// opening transaction, gets the tag of the trip its date lies in, unless
    /// it carries it already.
    #[must_use]
    pub fn suggestion(&self, config: &Config, txn: &Txn) -> Option<String> {
        if txn.flag != '!' {
            return None;
        }
        let id = txn.id.as_deref()?;
        if config
            .run_source(&txn.file)
            .is_some_and(|source| id == opening_id(&source.id))
        {
            return None;
        }
        let tag = trip_tag(self.trip_on(txn.date)?);
        (!txn.tags.contains(&tag)).then_some(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger;
    use crate::snapshot::Snapshot;

    fn date(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn event(file: &str, offset: usize, day: &str, name: &str) -> TripEvent {
        TripEvent {
            id: None,
            file: file.to_owned(),
            offset,
            line: 1,
            date: date(day),
            name: name.to_owned(),
        }
    }

    const FILE: &str = "manual/k7p2q9xd/2026-04.beancount";

    #[test]
    fn a_start_covers_its_date_and_every_later_one() {
        let trips = Trips::new(&[event(FILE, 0, "2026-04-10", "lisbon")]);
        assert_eq!(trips.trip_on(date("2026-04-09")), None);
        assert_eq!(trips.trip_on(date("2026-04-10")), Some("lisbon"));
        assert_eq!(trips.trip_on(date("2027-01-01")), Some("lisbon"));
        assert_eq!(trips.running(), Some("lisbon"));
    }

    #[test]
    fn an_end_extends_the_trip_by_three_days() {
        let trips = Trips::new(&[
            event(FILE, 0, "2026-04-10", "lisbon"),
            event(FILE, 50, "2026-04-21", ""),
        ]);
        assert_eq!(trips.trip_on(date("2026-04-21")), Some("lisbon"));
        assert_eq!(trips.trip_on(date("2026-04-24")), Some("lisbon"));
        assert_eq!(trips.trip_on(date("2026-04-25")), None);
        assert_eq!(trips.running(), None);
    }

    #[test]
    fn a_later_start_ends_the_running_trip() {
        let trips = Trips::new(&[
            event(FILE, 0, "2026-04-10", "a"),
            event(FILE, 50, "2026-04-15", "b"),
            event(FILE, 100, "2026-04-20", ""),
        ]);
        assert_eq!(trips.trip_on(date("2026-04-14")), Some("a"));
        assert_eq!(trips.trip_on(date("2026-04-15")), Some("b"));
        assert_eq!(trips.trip_on(date("2026-04-23")), Some("b"));
        assert_eq!(trips.trip_on(date("2026-04-24")), None);
    }

    #[test]
    fn an_end_without_a_running_trip_ends_nothing() {
        let trips = Trips::new(&[
            event(FILE, 0, "2026-04-01", "a"),
            event(FILE, 50, "2026-04-10", ""),
            event("manual/k7p2q9xd/2026-05.beancount", 0, "2026-05-01", ""),
        ]);
        assert_eq!(trips.trip_on(date("2026-04-12")), Some("a"));
        assert_eq!(trips.trip_on(date("2026-04-14")), None);
        assert_eq!(trips.trip_on(date("2026-05-02")), None);
    }

    #[test]
    fn a_second_end_keeps_the_grace_of_the_first() {
        // Both phones end the trip on its last day.
        let trips = Trips::new(&[
            event(
                "manual/aaaaaaaa/2026-04.beancount",
                0,
                "2026-04-10",
                "lisbon",
            ),
            event("manual/aaaaaaaa/2026-04.beancount", 50, "2026-04-21", ""),
            event("manual/bbbbbbbb/2026-04.beancount", 0, "2026-04-21", ""),
        ]);
        for day in ["2026-04-21", "2026-04-24"] {
            assert_eq!(trips.trip_on(date(day)), Some("lisbon"), "{day}");
        }
        assert_eq!(trips.trip_on(date("2026-04-25")), None);
        assert_eq!(trips.running(), None);

        // An offline phone ends it a day after the other one did.
        let trips = Trips::new(&[
            event(
                "manual/aaaaaaaa/2026-04.beancount",
                0,
                "2026-04-10",
                "lisbon",
            ),
            event("manual/bbbbbbbb/2026-04.beancount", 0, "2026-04-21", ""),
            event("manual/aaaaaaaa/2026-04.beancount", 50, "2026-04-22", ""),
        ]);
        for day in ["2026-04-22", "2026-04-24"] {
            assert_eq!(trips.trip_on(date(day)), Some("lisbon"), "{day}");
        }
        assert_eq!(trips.trip_on(date("2026-04-25")), None);
    }

    #[test]
    fn events_on_one_date_follow_file_path_then_position() {
        let trips = Trips::new(&[
            event("manual/bbbbbbbb/2026-04.beancount", 0, "2026-04-10", "b"),
            event("manual/aaaaaaaa/2026-04.beancount", 0, "2026-04-10", "a"),
        ]);
        assert_eq!(trips.trip_on(date("2026-04-10")), Some("b"));

        let trips = Trips::new(&[
            event(FILE, 50, "2026-04-12", ""),
            event(FILE, 0, "2026-04-12", "c"),
        ]);
        assert_eq!(trips.trip_on(date("2026-04-15")), Some("c"));
        assert_eq!(trips.trip_on(date("2026-04-16")), None);
        assert_eq!(trips.running(), None);
    }

    fn with_id(id: &str, file: &str, day: &str, name: &str) -> TripEvent {
        TripEvent {
            id: Some(id.to_owned()),
            ..event(file, 0, day, name)
        }
    }

    #[test]
    fn events_of_one_day_follow_the_time_they_were_created() {
        // B starts the trip in the morning, A ends it in the evening; A's
        // file path sorts first, its ULID last.
        let start = with_id(
            "bbbbbbbb-01jc8m2w9z4f7q3r5t6v8x0y1a",
            "manual/bbbbbbbb/2026-04.beancount",
            "2026-04-10",
            "lisbon",
        );
        let end = with_id(
            "aaaaaaaa-01jcfh3k5n7p9r1s3t5v7w9y2b",
            "manual/aaaaaaaa/2026-04.beancount",
            "2026-04-10",
            "",
        );
        let trips = Trips::new(&[end.clone(), start.clone()]);
        assert_eq!(trips.running(), None);
        assert_eq!(trips.trip_on(date("2026-04-13")), Some("lisbon"));
        assert_eq!(trips.trip_on(date("2026-04-14")), None);

        // An event without a manual ID comes first on its date.
        let hand_written = event("manual/cccccccc/2026-04.beancount", 0, "2026-04-10", "");
        let trips = Trips::new(&[start, end, hand_written]);
        assert_eq!(trips.running(), None);
        assert_eq!(trips.trip_on(date("2026-04-13")), Some("lisbon"));
    }

    #[test]
    fn trip_events_with_other_names_are_ignored() {
        let trips = Trips::new(&[
            event(FILE, 0, "2026-04-10", "lisbon"),
            event(FILE, 50, "2026-04-12", "Porto 2026"),
            event(FILE, 100, "2026-04-13", "a_b"),
        ]);
        assert_eq!(trips.trip_on(date("2026-04-20")), Some("lisbon"));
        assert_eq!(trips.running(), Some("lisbon"));

        let trips = Trips::new(&[event(FILE, 0, "2026-04-12", "Porto 2026")]);
        assert_eq!(trips.trip_on(date("2026-04-12")), None);
        assert_eq!(trips.trip_on(date("2026-04-20")), None);
        assert_eq!(trips.running(), None);
    }

    const CONFIG: &str = r#"spec_version = 0
main_file = "main.beancount"
fallback_account = "Expenses:Uncategorized"
unattributed_account = "Expenses:Unattributed"

[[source]]
id = "giro"
account = "Assets:Bank:Giro"
currency = "EUR"
importer = "csv"
mapping = "sources/giro.toml"
"#;

    const MAIN: &str = "include \"accounts.beancount\"\ninclude \"imports/*/*/*.beancount\"\ninclude \"manual/*/*.beancount\"\n";

    const ACCOUNTS: &str = "2020-01-01 open Assets:Bank:Giro EUR\n2020-01-01 open Expenses:Uncategorized\n2020-01-01 open Equity:OpeningBalances\n";

    const RUN_FILE: &str = "imports/giro/2026/20260425T000000Z.beancount";

    fn txn(day: &str, flag: char, id: Option<&str>, tags: &str) -> String {
        let id = id.map_or(String::new(), |id| format!("  lentil_id: \"{id}\"\n"));
        format!(
            "2026-04-{day} {flag} \"Shop\" \"\" {tags}\n{id}  Assets:Bank:Giro  -1.00 EUR\n  Expenses:Uncategorized\n\n"
        )
    }

    #[test]
    fn open_transactions_get_the_tag_of_their_trip_suggested() {
        let opening = opening_id("giro");
        let run = [
            txn("11", '!', Some("giro-000000000001"), ""),
            txn("12", '!', Some("giro-000000000002"), "#trip-lisbon"),
            txn("13", '*', Some("giro-000000000003"), ""),
            txn("14", '!', None, ""),
            txn("25", '!', Some("giro-000000000005"), ""),
            txn("11", '!', Some(&opening), ""),
        ]
        .concat();
        let manual = "2026-04-10 event \"trip\" \"lisbon\"\n2026-04-21 event \"trip\" \"\"\n";
        let snapshot = Snapshot::from_files([
            ("lentil.toml", CONFIG.to_owned()),
            ("main.beancount", MAIN.to_owned()),
            ("accounts.beancount", ACCOUNTS.to_owned()),
            (RUN_FILE, run),
            (FILE, manual.to_owned()),
        ]);
        let config = Config::parse(CONFIG).unwrap();
        let ledger = ledger::load(&snapshot, &config.main_file);
        assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
        let trips = Trips::new(&ledger.trip_events);

        let suggestions: Vec<Option<String>> = ledger
            .transactions
            .iter()
            .map(|txn| trips.suggestion(&config, txn))
            .collect();
        assert_eq!(
            suggestions,
            vec![Some("trip-lisbon".to_owned()), None, None, None, None, None]
        );
    }
}
