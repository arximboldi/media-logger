//! The terminal report, and the two notes every format ends with.
//!
//! The coverage note lives here rather than with the page because this is
//! where it reads most plainly, and csv, xlsx and html all ask for the
//! same sentence.

use super::*;

/// What the report is able to speak for.
///
/// A log proves absence only while the daemon is running. Every format says
/// this somewhere, because a reader who takes a blank month for a quiet one
/// has been misled by the document, not by the data.
pub(super) struct Covered {
    full: usize,
    partial: usize,
    none: usize,
    recorded: i64,
    window: i64,
}

impl Covered {
    fn complete(&self) -> bool {
        self.partial == 0 && self.none == 0
    }

    /// One sentence, or nothing to say.
    ///
    /// Per host, because the days are counted by their weakest link: with
    /// two machines in a report, "recorded for ten minutes" would be the
    /// time *both* were up, which is nobody's ten minutes.
    fn line(&self, report: &Report) -> Option<String> {
        if self.complete() {
            return None;
        }
        let each: Vec<String> = report
            .coverage
            .iter()
            .map(|c| {
                let secs = c.secs(&report.window.from, &report.window.to);
                if report.coverage.len() == 1 {
                    human_secs(secs)
                } else {
                    format!("{} {}", c.host, human_secs(secs))
                }
            })
            .collect();
        let mut what = Vec::new();
        if self.none > 0 {
            what.push(format!(
                "{} day{} no record at all",
                self.none,
                if self.none == 1 { " has" } else { "s have" }
            ));
        }
        if self.partial > 0 {
            what.push(format!(
                "{} day{} only partly recorded",
                self.partial,
                if self.partial == 1 { " is" } else { "s are" }
            ));
        }
        Some(format!(
            "The logger recorded {} of this window's {}: {}. A gap in the record \
             is not evidence that nothing played.",
            each.join(", "),
            human_secs(self.window),
            what.join(", ")
        ))
    }
}

/// The note that has to come before the data, and whether it is a warning.
///
/// For a window it is about days; for an instant it is about that instant,
/// which is the case this log exists for -- "nothing was playing at 23:40"
/// is worth nothing unless the logger was running at 23:40.
pub(super) fn record_note(report: &Report, quiet: &QuietHours) -> Result<Option<String>> {
    if report.window.instant {
        let at = hhmmss(&report.window.from);
        let down: Vec<&str> = report
            .coverage
            .iter()
            .filter(|c| !c.covers(&report.window.from))
            .map(|c| c.host.as_str())
            .collect();
        return Ok(Some(if down.is_empty() {
            format!("The logger was running on every host at {at}.")
        } else {
            format!(
                "The logger was not running on {} at {at}, so this report \
                 cannot say what was playing there.",
                down.join(", ")
            )
        }));
    }
    Ok(covered(&crate::query::days(report, quiet)?).line(report))
}

pub(super) fn covered(days: &[crate::query::DaySummary]) -> Covered {
    use crate::query::Recorded;
    let mut c = Covered {
        full: 0,
        partial: 0,
        none: 0,
        recorded: 0,
        window: 0,
    };
    for day in days {
        match day.recorded() {
            Recorded::Full => c.full += 1,
            Recorded::Partial => c.partial += 1,
            Recorded::None => c.none += 1,
        }
        c.recorded += day.recorded_secs;
        c.window += day.window_secs;
    }
    c
}

pub(super) fn text(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
    let mut o = String::new();
    o.push_str(&format!(
        "media-logger — {} (host {})\n",
        report.window.label,
        report.hosts.join(", ")
    ));

    if opening_state_matters(report) {
        o.push_str(&format!("\nAt {}\n", hhmmss(&report.window.from)));
        for (host, at) in &report.at_start {
            // Named even with one host, so a report never reads as though it
            // covered a machine it did not.
            o.push_str(&format!("  [{host}]\n"));
            let playing = at.playing();
            // "Nothing playing" is a claim about the room. With the daemon
            // down it is only a claim about the log, and the state shown
            // would be whatever was true whenever it last ran.
            let recorded = report
                .coverage
                .iter()
                .any(|c| c.host == *host && c.covers(&report.window.from));
            if !recorded {
                o.push_str("    no record: the logger was not running\n");
                continue;
            }
            if playing.is_empty() {
                o.push_str("    nothing playing\n");
            } else {
                for p in playing {
                    o.push_str(&format!(
                        "    {} playing: {}\n",
                        crate::query::player_name(&p.player),
                        p.track.title.clone().unwrap_or_else(|| "(untitled)".into())
                    ));
                }
            }
            match &at.sink {
                Some(s) => o.push_str(&format!(
                    "    sink: {} ({}), volume {} {}\n",
                    s.description.clone().unwrap_or_else(|| s.name.clone()),
                    s.device_api.clone().unwrap_or_else(|| "?".into()),
                    s.effective()
                        .iter()
                        .map(|v| format!("{v:.3}"))
                        .collect::<Vec<_>>()
                        .join("/"),
                    s.volume_scale,
                )),
                None => o.push_str("    sink: unknown\n"),
            }
            if let Some(as_of) = &at.as_of {
                o.push_str(&format!("    (state as of {as_of})\n"));
            }
        }
    }

    if report.window.instant {
        o.push_str(&footer_text(integrity, report, quiet)?);
        return Ok(o);
    }

    let wide = multiday(report);
    o.push_str("\nPlayback\n");
    if report.spans.is_empty() {
        o.push_str("  nothing played in this window\n");
    }
    for st in stretches(report) {
        let span = st.head();
        let q = st.quiet_seconds(quiet);
        let width = if wide { 36 } else { 30 };
        o.push_str(&format!(
            "  {}–{}  {:>7}  {:10} {}\n",
            clock(st.start(), wide),
            clock(st.end(), wide),
            st.duration(),
            span.player,
            span.label()
        ));
        if let Some(parts) = st.parts() {
            o.push_str(&format!("  {:width$}{parts}\n", ""));
        }
        if let Some(l) = st.levels() {
            o.push_str(&format!(
                "  {:width$}volume {} min / {} med / {} max  (n={}){}\n",
                "",
                pct(l.min),
                pct(l.median),
                pct(l.max),
                l.samples,
                span.sink_label
                    .as_deref()
                    .or(span.sink.as_deref())
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default(),
            ));
        }
        if q > 0 {
            o.push_str(&format!(
                "  {:width$}within quiet hours for {}\n",
                "",
                human_secs(q),
            ));
        }
    }

    let played: i64 = stretches(report).iter().map(|st| st.played()).sum();
    o.push_str(&format!("\n  total playback: {}\n", human_secs(played)));

    if !quiet.is_empty() {
        let total: i64 = report.spans.iter().map(|s| quiet_seconds(s, quiet)).sum();
        o.push_str(&format!("\nQuiet hours: {}\n", quiet.describe().join(", ")));
        o.push_str(&format!(
            "  playback inside quiet hours: {}\n",
            if total == 0 {
                "none".to_owned()
            } else {
                human_secs(total)
            }
        ));
    }

    o.push_str(&footer_text(integrity, report, quiet)?);
    Ok(o)
}

/// Greedy wrap. The terminal report is the one read in a narrow window.
pub(super) fn wrapped(s: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for word in s.split_whitespace() {
        let line = out.last_mut().expect("never empty");
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            out.push(word.to_owned());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    out
}

pub(super) fn footer_text(
    integrity: &Integrity,
    report: &Report,
    quiet: &QuietHours,
) -> Result<String> {
    let mut o = String::new();
    // Beside the integrity notes rather than above the data: both are
    // statements about the record itself rather than about the evening.
    if let Some(line) = record_note(report, quiet)? {
        o.push_str("\nRecord\n");
        for chunk in wrapped(&line, 74) {
            o.push_str(&format!("  {chunk}\n"));
        }
    }
    o.push_str("\nIntegrity\n");
    if report.unreadable > 0 {
        o.push_str(&format!(
            "  WARNING: {} line(s) had an unreadable timestamp and were skipped\n",
            report.unreadable
        ));
    }
    o.push_str(&format!(
        "  chain {} ({} lines)\n",
        if integrity.ok() { "intact" } else { "BROKEN" },
        integrity.lines()
    ));
    for p in &integrity.problems() {
        o.push_str(&format!("  PROBLEM: {p}\n"));
    }
    for (host, span, state) in anchor_runs(integrity) {
        o.push_str(&format!("  {host}  {span}: {state}\n"));
    }
    Ok(o)
}
