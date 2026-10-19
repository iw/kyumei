//! Time windows.
//!
//! Domain logic never reads the wall clock. Every timestamp arrives as data,
//! which keeps evaluation deterministic and replayable inside a durable
//! workflow.

use jiff::{SignedDuration, Timestamp, tz::TimeZone};
use serde::{Deserialize, Serialize};

use crate::DomainError;

/// A closed interval `[start, end]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub start: Timestamp,
    pub end: Timestamp,
}

impl TimeWindow {
    pub fn new(start: Timestamp, end: Timestamp) -> Result<Self, DomainError> {
        if end < start {
            return Err(DomainError::InvalidWindow { start, end });
        }
        Ok(Self { start, end })
    }

    pub fn contains(&self, at: Timestamp) -> bool {
        self.start <= at && at <= self.end
    }

    pub fn overlaps(&self, other: &TimeWindow) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    pub fn intersection(&self, other: &TimeWindow) -> Option<TimeWindow> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start <= end).then_some(TimeWindow { start, end })
    }

    /// The smallest window covering both.
    pub fn union(&self, other: &TimeWindow) -> TimeWindow {
        TimeWindow {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn duration(&self) -> SignedDuration {
        self.end.duration_since(self.start)
    }

    /// Widen the window by `before` and `after`.
    pub fn padded(&self, before: SignedDuration, after: SignedDuration) -> TimeWindow {
        let start = self.start.checked_sub(before).unwrap_or(self.start);
        let end = self.end.checked_add(after).unwrap_or(self.end);
        TimeWindow { start, end }
    }

    /// `2026-09-30 13:35–13:50 UTC`, an instant to the second, or with dates on
    /// both ends when the window spans days.
    pub fn display_utc(&self) -> String {
        DisplayZone::utc().window(self)
    }
}

/// The time zone dates and times are shown in. Kyumei stores and reasons in UTC;
/// a display zone only changes how times are written and how times typed into
/// forms are read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayZone {
    name: String,
    tz: TimeZone,
}

impl Default for DisplayZone {
    fn default() -> Self {
        Self::utc()
    }
}

impl DisplayZone {
    pub fn utc() -> Self {
        Self {
            name: "UTC".to_owned(),
            tz: TimeZone::UTC,
        }
    }

    /// An IANA zone such as `Europe/London`, or `UTC`.
    pub fn named(name: &str) -> Result<Self, DomainError> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("UTC") {
            return Ok(Self::utc());
        }
        let tz =
            TimeZone::get(name).map_err(|_| DomainError::Unknown(format!("time zone {name:?}")))?;
        Ok(Self {
            name: name.to_owned(),
            tz,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_utc(&self) -> bool {
        self.name == "UTC"
    }

    /// The zone's label at an instant: its abbreviation, such as `BST`, or its
    /// offset, such as `UTC−03:00`, when the zone has only a numeric one.
    pub fn label(&self, at: Timestamp) -> String {
        if self.is_utc() {
            return "UTC".to_owned();
        }
        let info = self.tz.to_offset_info(at);
        let abbreviation = info.abbreviation();
        if abbreviation.starts_with(['+', '-']) || abbreviation.is_empty() {
            offset_label(info.offset().seconds())
        } else {
            abbreviation.to_owned()
        }
    }

    /// The zone's offset from UTC at an instant, such as `UTC+01:00`.
    pub fn offset(&self, at: Timestamp) -> String {
        offset_label(self.offset_seconds(at))
    }

    /// The zone's offset from UTC at an instant, in seconds.
    pub fn offset_seconds(&self, at: Timestamp) -> i32 {
        self.tz.to_offset(at).seconds()
    }

    /// How the zone is offered in a selector: `Europe/London · BST (UTC+01:00)`.
    pub fn describe(&self, at: Timestamp) -> String {
        if self.is_utc() {
            return "UTC".to_owned();
        }
        let label = self.label(at);
        let offset = self.offset(at);
        if label == offset {
            format!("{} · {offset}", self.name)
        } else {
            format!("{} · {label} ({offset})", self.name)
        }
    }

    /// `14:21`, for lists that name the zone once.
    pub fn clock(&self, at: Timestamp) -> String {
        at.to_zoned(self.tz.clone()).strftime("%H:%M").to_string()
    }

    /// `14:21:05`.
    pub fn clock_seconds(&self, at: Timestamp) -> String {
        at.to_zoned(self.tz.clone())
            .strftime("%H:%M:%S")
            .to_string()
    }

    /// `14:21 BST`.
    pub fn time(&self, at: Timestamp) -> String {
        format!("{} {}", self.clock(at), self.label(at))
    }

    /// `2026-10-14 14:21 BST`.
    pub fn date_time(&self, at: Timestamp) -> String {
        format!(
            "{} {}",
            at.to_zoned(self.tz.clone()).strftime("%Y-%m-%d %H:%M"),
            self.label(at)
        )
    }

    /// `2026-10-14 14:21:05 BST`.
    pub fn date_time_seconds(&self, at: Timestamp) -> String {
        format!(
            "{} {}",
            at.to_zoned(self.tz.clone()).strftime("%Y-%m-%d %H:%M:%S"),
            self.label(at)
        )
    }

    /// `2026-10-14 13:30–14:21 BST`, an instant to the second, or with dates on
    /// both ends when the window spans days. A window across a change of offset
    /// labels both ends.
    pub fn window(&self, w: &TimeWindow) -> String {
        let start = w.start.to_zoned(self.tz.clone());
        let end = w.end.to_zoned(self.tz.clone());
        let (from, to) = (self.label(w.start), self.label(w.end));
        if w.start == w.end {
            // A point-in-time capture, such as a DescribeTaskQueue snapshot.
            format!("{} {from}", start.strftime("%Y-%m-%d %H:%M:%S"))
        } else if from != to {
            format!(
                "{} {from} – {} {to}",
                start.strftime("%Y-%m-%d %H:%M"),
                end.strftime("%Y-%m-%d %H:%M")
            )
        } else if start.date() == end.date() {
            format!(
                "{} {}–{} {to}",
                start.date(),
                start.strftime("%H:%M"),
                end.strftime("%H:%M")
            )
        } else {
            format!(
                "{} – {} {to}",
                start.strftime("%Y-%m-%d %H:%M"),
                end.strftime("%Y-%m-%d %H:%M")
            )
        }
    }

    /// An incident window as far as it is known.
    pub fn incident_window(&self, w: &IncidentWindow) -> String {
        match (w.as_window(), w.start, w.end) {
            (Some(window), _, _) => self.window(&window),
            (None, Some(start), None) => format!("from {} (end unknown)", self.date_time(start)),
            (None, None, Some(end)) => format!("until {} (start unknown)", self.date_time(end)),
            _ => match &w.description {
                Some(text) => format!("unknown (operator: “{text}”)"),
                None => "unknown".to_owned(),
            },
        }
    }

    /// The value of an HTML `datetime-local` field showing `at` in this zone.
    pub fn input_value(&self, at: Timestamp) -> String {
        at.to_zoned(self.tz.clone())
            .strftime("%Y-%m-%dT%H:%M")
            .to_string()
    }

    /// Read a `datetime-local` value (`2026-10-14T14:21`, seconds optional) as a
    /// time in this zone. A time skipped by a change of offset moves forward, and
    /// a repeated one takes the earlier instant.
    pub fn parse_input(&self, text: &str) -> Result<Timestamp, DomainError> {
        let civil: jiff::civil::DateTime = text
            .trim()
            .parse()
            .map_err(|_| DomainError::Unknown(format!("date and time {text:?}")))?;
        self.tz
            .to_ambiguous_zoned(civil)
            .compatible()
            .map(|z| z.timestamp())
            .map_err(|_| DomainError::Unknown(format!("date and time {text:?}")))
    }
}

/// `UTC+01:00`, `UTC−03:00` (with a minus sign), or `UTC` for no offset.
fn offset_label(seconds: i32) -> String {
    if seconds == 0 {
        return "UTC".to_owned();
    }
    let sign = if seconds < 0 { '−' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The period during which the symptom was present, as far as it is known.
///
/// Either bound may be unknown. Unknown bounds stay unknown: Kyumei never
/// guesses an incident window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentWindow {
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    /// The operator's own words, e.g. "since about 13:35".
    pub description: Option<String>,
}

impl IncidentWindow {
    pub fn unknown() -> Self {
        Self::default()
    }

    pub fn bounded(start: Timestamp, end: Timestamp) -> Result<Self, DomainError> {
        TimeWindow::new(start, end)?;
        Ok(Self {
            start: Some(start),
            end: Some(end),
            description: None,
        })
    }

    /// The window when both bounds are known.
    pub fn as_window(&self) -> Option<TimeWindow> {
        match (self.start, self.end) {
            (Some(start), Some(end)) if start <= end => Some(TimeWindow { start, end }),
            _ => None,
        }
    }

    pub fn is_fully_known(&self) -> bool {
        self.as_window().is_some()
    }

    /// The window in UTC, as far as it is known.
    pub fn display(&self) -> String {
        DisplayZone::utc().incident_window(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn rejects_inverted_windows() {
        assert!(TimeWindow::new(ts("2026-09-30T13:50:00Z"), ts("2026-09-30T13:35:00Z")).is_err());
    }

    #[test]
    fn overlap_and_intersection() {
        let a = TimeWindow::new(ts("2026-09-30T13:30:00Z"), ts("2026-09-30T13:45:00Z")).expect("a");
        let b = TimeWindow::new(ts("2026-09-30T13:40:00Z"), ts("2026-09-30T13:55:00Z")).expect("b");
        assert!(a.overlaps(&b));
        let i = a.intersection(&b).expect("intersection");
        assert_eq!(i.start, ts("2026-09-30T13:40:00Z"));
        assert_eq!(i.end, ts("2026-09-30T13:45:00Z"));
        assert_eq!(a.display_utc(), "2026-09-30 13:30–13:45 UTC");
    }

    #[test]
    fn unknown_window_stays_unknown() {
        let w = IncidentWindow::unknown();
        assert!(w.as_window().is_none());
        assert_eq!(w.display(), "unknown");
    }

    #[test]
    fn utc_text_is_unchanged() {
        let point =
            TimeWindow::new(ts("2026-09-30T13:35:07Z"), ts("2026-09-30T13:35:07Z")).expect("point");
        assert_eq!(point.display_utc(), "2026-09-30 13:35:07 UTC");
        let days =
            TimeWindow::new(ts("2026-09-30T23:00:00Z"), ts("2026-10-01T01:00:00Z")).expect("days");
        assert_eq!(
            days.display_utc(),
            "2026-09-30 23:00 – 2026-10-01 01:00 UTC"
        );
        let open = IncidentWindow {
            start: Some(ts("2026-10-14T13:03:00Z")),
            ..IncidentWindow::default()
        };
        assert_eq!(open.display(), "from 2026-10-14 13:03 UTC (end unknown)");
    }

    #[test]
    fn times_are_written_in_the_display_zone() {
        let london = DisplayZone::named("Europe/London").expect("zone");
        let at = ts("2026-10-14T13:21:00Z");
        assert_eq!(london.time(at), "14:21 BST");
        assert_eq!(london.date_time(at), "2026-10-14 14:21 BST");
        assert_eq!(london.describe(at), "Europe/London · BST (UTC+01:00)");
        let w = TimeWindow::new(ts("2026-10-14T12:30:00Z"), at).expect("window");
        assert_eq!(london.window(&w), "2026-10-14 13:30–14:21 BST");
        // Across the end of summer time, each end carries its own label.
        let fall =
            TimeWindow::new(ts("2026-10-25T00:30:00Z"), ts("2026-10-25T01:30:00Z")).expect("fall");
        assert_eq!(
            london.window(&fall),
            "2026-10-25 01:30 BST – 2026-10-25 01:30 GMT"
        );
        // A zone whose abbreviation is a number is labelled by its offset.
        let sao_paulo = DisplayZone::named("America/Sao_Paulo").expect("zone");
        assert_eq!(sao_paulo.time(at), "10:21 UTC−03:00");
        assert_eq!(DisplayZone::named("utc").expect("utc"), DisplayZone::utc());
        assert!(DisplayZone::named("Mars/Olympus_Mons").is_err());
    }

    #[test]
    fn form_times_are_read_in_the_display_zone() {
        let london = DisplayZone::named("Europe/London").expect("zone");
        let at = london.parse_input("2026-10-14T14:21").expect("parse");
        assert_eq!(at, ts("2026-10-14T13:21:00Z"));
        assert_eq!(london.input_value(at), "2026-10-14T14:21");
        assert_eq!(
            DisplayZone::utc()
                .parse_input("2026-10-14T14:21:30")
                .expect("seconds"),
            ts("2026-10-14T14:21:30Z")
        );
        // 01:30 on 29 March 2026 never happens in London; it moves forward.
        assert_eq!(
            london.parse_input("2026-03-29T01:30").expect("gap"),
            ts("2026-03-29T01:30:00Z")
        );
        assert!(london.parse_input("yesterday").is_err());
    }
}
