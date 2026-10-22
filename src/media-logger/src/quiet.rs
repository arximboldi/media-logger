//! Ruhezeiten — the periods a report shades and counts against.
//!
//! The default is Nachtruhe, 22:00–06:00, which is the one with broad
//! statutory backing in Germany (TA Lärm and the Länder immission statutes
//! use that window).
//!
//! Mittagsruhe and all-day Sunday rules are deliberately *not* default: they
//! come from a building's Hausordnung and vary, and shading a period as quiet
//! that the house rules do not actually impose would overstate the case in a
//! document meant to withstand being picked at. Add them explicitly when the
//! Hausordnung says so.

use anyhow::{bail, Context, Result};
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{Span, Zoned};

/// Nachtruhe.
pub const DEFAULT: &str = "22:00-06:00";

#[derive(Debug, Clone)]
struct Rule {
    /// `None` means every day. Days are 1 = Monday .. 7 = Sunday.
    days: Option<Vec<i8>>,
    start: Time,
    end: Time,
    source: String,
}

#[derive(Debug, Clone, Default)]
pub struct QuietHours {
    rules: Vec<Rule>,
}

impl QuietHours {
    /// Parses `["22:00-06:00", "sun 00:00-24:00", "mon,tue 13:00-15:00"]`.
    pub fn parse(specs: &[String]) -> Result<Self> {
        let mut rules = Vec::new();
        for spec in specs {
            rules.push(parse_rule(spec)?);
        }
        Ok(QuietHours { rules })
    }

    pub fn describe(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.source.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Concrete quiet intervals overlapping `date`, in `tz`.
    ///
    /// A rule whose end is at or before its start wraps midnight, so the
    /// previous day is consulted too: 22:00–06:00 on Monday puts Tuesday's
    /// small hours inside Monday's rule, which is where a complaint about
    /// "Monday night" actually falls.
    pub fn intervals(&self, date: Date, tz: &TimeZone) -> Result<Vec<(Zoned, Zoned)>> {
        let mut out = Vec::new();
        for offset in [-1, 0] {
            let day = date.checked_add(Span::new().days(offset))?;
            let weekday = day.weekday().to_monday_one_offset();
            for rule in &self.rules {
                if rule.days.as_ref().is_some_and(|d| !d.contains(&weekday)) {
                    continue;
                }
                let start = day.to_zoned(tz.clone())?.with().time(rule.start).build()?;
                let mut end = day.to_zoned(tz.clone())?.with().time(rule.end).build()?;
                if rule.end <= rule.start {
                    end = end.checked_add(Span::new().days(1))?;
                }
                // Keep only what actually touches the requested day.
                let day_start = date.to_zoned(tz.clone())?;
                let day_end = day_start.checked_add(Span::new().days(1))?;
                if end > day_start && start < day_end {
                    out.push((start.max(day_start), end.min(day_end)));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn parse_rule(spec: &str) -> Result<Rule> {
    let trimmed = spec.trim();
    let (days, range) = match trimmed.split_once(char::is_whitespace) {
        Some((head, tail)) => (Some(parse_days(head)?), tail.trim()),
        None => (None, trimmed),
    };

    let (a, b) = range
        .split_once('-')
        .with_context(|| format!("quiet hours need a HH:MM-HH:MM range: {spec}"))?;
    Ok(Rule {
        days,
        start: parse_time(a.trim())?,
        end: parse_time(b.trim())?,
        source: trimmed.to_owned(),
    })
}

fn parse_days(spec: &str) -> Result<Vec<i8>> {
    let mut days = Vec::new();
    for part in spec.split(',') {
        let part = part.trim().to_ascii_lowercase();
        // `mon-fri`
        if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (day_index(a)?, day_index(b)?);
            let mut d = a;
            loop {
                days.push(d);
                if d == b {
                    break;
                }
                d = if d == 7 { 1 } else { d + 1 };
            }
            continue;
        }
        days.push(day_index(&part)?);
    }
    Ok(days)
}

fn day_index(name: &str) -> Result<i8> {
    let name = name.trim().to_ascii_lowercase();
    DAYS.iter()
        .position(|d| name.starts_with(d))
        .map(|i| i as i8 + 1)
        .with_context(|| format!("not a weekday: {name}"))
}

/// `24:00` is accepted as end-of-day, which is how house rules are written.
fn parse_time(s: &str) -> Result<Time> {
    if s == "24:00" {
        return Ok(Time::constant(23, 59, 59, 999_999_999));
    }
    let (h, m) = s
        .split_once(':')
        .with_context(|| format!("not a HH:MM time: {s}"))?;
    let (h, m): (i8, i8) = (h.parse()?, m.parse()?);
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        bail!("time out of range: {s}");
    }
    Ok(Time::new(h, m, 0, 0)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hours(specs: &[&str]) -> QuietHours {
        QuietHours::parse(&specs.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    /// Whether an instant falls inside any quiet period. Production asks
    /// `intervals` how much of a *span* overlaps; this is the pointwise form
    /// the wrap-around cases are easiest to state in.
    fn covers(q: &QuietHours, at: &Zoned) -> bool {
        let tz = at.time_zone().clone();
        q.intervals(at.date(), &tz)
            .map(|spans| spans.iter().any(|(s, e)| at >= s && at < e))
            .unwrap_or(false)
    }

    fn at(day: i8, hour: i8) -> Zoned {
        jiff::civil::date(2026, 10, day)
            .at(hour, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap()
    }

    #[test]
    fn nachtruhe_wraps_midnight() {
        let q = hours(&[DEFAULT]);
        assert!(covers(&q, &at(22, 23)), "23:00 is quiet");
        assert!(
            covers(&q, &at(22, 2)),
            "02:00 is quiet, under the previous day's rule"
        );
        assert!(!covers(&q, &at(22, 12)), "midday is not");
        assert!(!covers(&q, &at(22, 21)), "21:00 is not");
    }

    #[test]
    fn day_scoped_rules_apply_only_on_their_days() {
        // 2026-10-25 is a Sunday.
        let q = hours(&["sun 00:00-24:00"]);
        assert!(covers(&q, &at(25, 12)), "Sunday midday is quiet");
        assert!(!covers(&q, &at(26, 12)), "Monday midday is not");
    }

    #[test]
    fn ranges_of_days_expand() {
        let q = hours(&["mon-fri 13:00-15:00"]);
        assert!(covers(&q, &at(22, 14)), "Thursday afternoon");
        assert!(!covers(&q, &at(24, 14)), "Saturday is outside mon-fri");
    }

    #[test]
    fn rejects_nonsense() {
        assert!(QuietHours::parse(&["22:00".to_owned()]).is_err());
        assert!(QuietHours::parse(&["25:00-26:00".to_owned()]).is_err());
        assert!(QuietHours::parse(&["funday 01:00-02:00".to_owned()]).is_err());
    }
}
