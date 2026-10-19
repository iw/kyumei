//! Showing times in the operator's time zone.
//!
//! Kyumei records and reasons in UTC, and the text it generates (explanations,
//! request windows, notes) writes times in UTC. Presentation rewrites those
//! times into the selected zone, so every page reads in one zone.

use std::sync::LazyLock;

use jiff::{Timestamp, civil, tz::TimeZone};
use kyumei_domain::{DisplayZone, TimeWindow};
use regex::{Captures, Regex};

/// The UTC forms Kyumei writes, longest first: a window across days, a window
/// within a day, a date and time (to the second or the minute), a time window
/// without a date, and a bare time.
static UTC_TIMES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?P<d1>\d{4}-\d{2}-\d{2}) (?P<t1>\d{2}:\d{2}) – (?P<d2>\d{4}-\d{2}-\d{2}) (?P<t2>\d{2}:\d{2}) UTC",
        r"|(?P<d3>\d{4}-\d{2}-\d{2}) (?P<t3>\d{2}:\d{2})–(?P<t4>\d{2}:\d{2}) UTC",
        r"|(?P<d5>\d{4}-\d{2}-\d{2})[ T](?P<t5>\d{2}:\d{2}(?::\d{2})?) UTC",
        r"|\b(?P<t6>\d{2}:\d{2})–(?P<t7>\d{2}:\d{2}) UTC",
        r"|\b(?P<t8>\d{2}:\d{2}(?::\d{2})?) UTC",
    ))
    .unwrap_or_else(|e| panic!("{e}"))
});

/// Rewrite the UTC times Kyumei wrote in `text` into `zone`. Times without a
/// date are taken to be on `reference`'s date. Text in UTC comes back unchanged.
pub fn localize(text: &str, zone: &DisplayZone, reference: Timestamp) -> String {
    if zone.is_utc() || !text.contains("UTC") {
        return text.to_owned();
    }
    let day = reference.to_zoned(TimeZone::UTC).date();
    UTC_TIMES
        .replace_all(text, |c: &Captures<'_>| {
            rewrite(c, zone, day).unwrap_or_else(|| c[0].to_owned())
        })
        .into_owned()
}

fn rewrite(c: &Captures<'_>, zone: &DisplayZone, day: civil::Date) -> Option<String> {
    let at = |date: Option<&str>, time: &str| -> Option<Timestamp> {
        let date = match date {
            Some(d) => d.parse::<civil::Date>().ok()?,
            None => day,
        };
        let time = time.parse::<civil::Time>().ok()?;
        date.to_datetime(time)
            .to_zoned(TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp())
    };
    let name = |n: &str| c.name(n).map(|m| m.as_str());
    if let (Some(d1), Some(t1), Some(d2), Some(t2)) =
        (name("d1"), name("t1"), name("d2"), name("t2"))
    {
        let w = TimeWindow::new(at(Some(d1), t1)?, at(Some(d2), t2)?).ok()?;
        return Some(zone.window(&w));
    }
    if let (Some(d3), Some(t3), Some(t4)) = (name("d3"), name("t3"), name("t4")) {
        let w = TimeWindow::new(at(Some(d3), t3)?, at(Some(d3), t4)?).ok()?;
        return Some(zone.window(&w));
    }
    if let (Some(d5), Some(t5)) = (name("d5"), name("t5")) {
        let t = at(Some(d5), t5)?;
        return Some(if t5.len() > 5 {
            zone.date_time_seconds(t)
        } else {
            zone.date_time(t)
        });
    }
    if let (Some(t6), Some(t7)) = (name("t6"), name("t7")) {
        let (start, end) = (at(None, t6)?, at(None, t7)?);
        let label = zone.label(end);
        return Some(format!("{}–{} {label}", zone.clock(start), zone.clock(end)));
    }
    let t8 = name("t8")?;
    let t = at(None, t8)?;
    Some(if t8.len() > 5 {
        format!("{} {}", zone.clock_seconds(t), zone.label(t))
    } else {
        zone.time(t)
    })
}

/// The zones an operator may choose from: UTC, then `preferred`, or every IANA
/// zone when none are preferred.
#[derive(Clone, Debug, Default)]
pub struct TimeZones {
    preferred: Vec<DisplayZone>,
}

impl TimeZones {
    pub fn new(preferred: Vec<DisplayZone>) -> Self {
        let mut unique: Vec<DisplayZone> = Vec::new();
        for z in preferred {
            if !z.is_utc() && !unique.iter().any(|u| u.name() == z.name()) {
                unique.push(z);
            }
        }
        Self { preferred: unique }
    }

    /// The zones configured as preferred, without UTC.
    pub fn preferred(&self) -> &[DisplayZone] {
        &self.preferred
    }

    /// The zones the selector offers, UTC first, grouped by region when every
    /// zone is offered.
    pub fn offered(&self) -> Vec<(String, Vec<DisplayZone>)> {
        let mut groups = vec![(String::new(), vec![DisplayZone::utc()])];
        if !self.preferred.is_empty() {
            groups[0].1.extend(self.preferred.iter().cloned());
            return groups;
        }
        let mut names: Vec<String> = jiff::tz::db()
            .available()
            .map(|n| n.as_str().to_owned())
            .filter(|n| is_region_zone(n))
            .collect();
        names.sort();
        names.dedup();
        for name in names {
            let Ok(zone) = DisplayZone::named(&name) else {
                continue;
            };
            let region = name.split('/').next().unwrap_or_default().to_owned();
            match groups.last_mut() {
                Some((r, zones)) if *r == region => zones.push(zone),
                _ => groups.push((region, vec![zone])),
            }
        }
        groups
    }

    /// The zone to show times in: `chosen` when it is offered, else UTC.
    pub fn resolve(&self, chosen: Option<&str>) -> DisplayZone {
        let Some(name) = chosen.map(str::trim).filter(|n| !n.is_empty()) else {
            return DisplayZone::utc();
        };
        if self.preferred.is_empty() {
            return DisplayZone::named(name)
                .ok()
                .filter(|z| z.is_utc() || is_region_zone(z.name()))
                .unwrap_or_else(DisplayZone::utc);
        }
        self.preferred
            .iter()
            .find(|z| z.name() == name)
            .cloned()
            .unwrap_or_else(DisplayZone::utc)
    }
}

/// A zone named for a region and place, such as `Europe/London`: not the
/// legacy aliases (`US/Eastern`, `Etc/GMT+5`).
fn is_region_zone(name: &str) -> bool {
    const REGIONS: [&str; 10] = [
        "Africa",
        "America",
        "Antarctica",
        "Arctic",
        "Asia",
        "Atlantic",
        "Australia",
        "Europe",
        "Indian",
        "Pacific",
    ];
    name.split_once('/')
        .is_some_and(|(region, _)| REGIONS.contains(&region))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn kyumei_text_is_rewritten_into_the_zone() {
        let london = DisplayZone::named("Europe/London").expect("zone");
        let reference = ts("2026-10-14T13:00:00Z");
        let text = "2026-10-14 12:20–14:26 UTC (the incident window plus 10 min before and 5 min after); \
                    since 13:03 UTC; from 2026-10-14 13:03 UTC (end unknown); \
                    2026-09-30 23:00 – 2026-10-01 01:00 UTC; captured 2026-10-14 13:21:05 UTC; \
                    13:35–13:50 UTC; in UTC terms";
        assert_eq!(
            localize(text, &london, reference),
            "2026-10-14 13:20–15:26 BST (the incident window plus 10 min before and 5 min after); \
             since 14:03 BST; from 2026-10-14 14:03 BST (end unknown); \
             2026-10-01 00:00–02:00 BST; captured 2026-10-14 14:21:05 BST; \
             14:35–14:50 BST; in UTC terms"
        );
        assert_eq!(localize(text, &DisplayZone::utc(), reference), text);
    }

    #[test]
    fn the_selector_offers_utc_and_the_preferred_zones() {
        let zones = TimeZones::new(vec![
            DisplayZone::named("Europe/London").expect("zone"),
            DisplayZone::utc(),
            DisplayZone::named("Europe/London").expect("zone"),
            DisplayZone::named("America/New_York").expect("zone"),
        ]);
        let offered = zones.offered();
        assert_eq!(offered.len(), 1);
        let names: Vec<&str> = offered[0].1.iter().map(DisplayZone::name).collect();
        assert_eq!(names, ["UTC", "Europe/London", "America/New_York"]);
        assert_eq!(
            zones.resolve(Some("America/New_York")).name(),
            "America/New_York"
        );
        assert_eq!(
            zones.resolve(Some("Asia/Tokyo")).name(),
            "UTC",
            "not preferred"
        );
        assert_eq!(zones.resolve(None).name(), "UTC");

        let any = TimeZones::default();
        let groups = any.offered();
        assert_eq!(groups[0].1[0].name(), "UTC");
        assert!(
            groups
                .iter()
                .any(|(r, z)| r == "Asia" && z.iter().any(|z| z.name() == "Asia/Tokyo"))
        );
        assert!(!groups.iter().any(|(r, _)| r == "US" || r == "Etc"));
        assert_eq!(any.resolve(Some("Asia/Tokyo")).name(), "Asia/Tokyo");
        assert_eq!(
            any.resolve(Some("US/Eastern")).name(),
            "UTC",
            "legacy alias"
        );
        assert_eq!(any.resolve(Some("not a zone")).name(), "UTC");
    }
}
