//! Output formats.
//!
//! Every format carries the integrity footer. A report that needs a separate
//! `verify` run before anyone trusts it will be circulated without one.

use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use jiff::civil::Date;
use jiff::{Span as JiffSpan, Zoned};

use crate::query::{Integrity, Recorded, Report, Span};
use crate::quiet::QuietHours;

// What this module is made of. Three modules are the vocabulary every
// format writes in; the rest are one per thing written.
//
// Submodules reach back here with `use super::*`, which is what keeps a
// helper shared by three of them in one place rather than three.

/// Durations, clock times, dates, and escaping.
mod fmt;
/// Colours, the calendar's ramp, the cubic gain scale.
mod palette;
/// Spans grouped into the rows the reading formats show.
mod stretch;

/// The month grid inside the page.
mod calendar;
/// The whole report as one document.
mod json;
/// The html page, and the pdf printed from it.
mod page;
/// The spans table, as csv and as xlsx.
mod table;
/// The terminal report, and the notes every format ends with.
mod text;
/// The day strip inside the page.
mod timeline;

use calendar::calendar;
use fmt::*;
use json::json;
use page::*;
use palette::*;
use stretch::*;
use table::*;
use text::*;
use timeline::*;

/// The one part of the palette the command line names: `--gain-scale`.
pub use palette::GainScale;

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
    Jsonl,
    Csv,
    Html,
    Pdf,
    Xlsx,
}

pub fn render(
    report: &mut Report,
    integrity: &Integrity,
    quiet: &QuietHours,
    format: Format,
    out: Option<&Path>,
    scale: GainScale,
) -> Result<()> {
    to_cubic(report);
    let report = &*report;
    let body = match format {
        Format::Text => text(report, integrity, quiet)?,
        Format::Json => json(report, integrity, quiet)?,
        Format::Jsonl => report
            .records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
        Format::Csv => csv(report, integrity, quiet)?,
        Format::Html | Format::Pdf => html(report, integrity, quiet, scale)?,
        Format::Xlsx => {
            let path = out.context("--format xlsx needs -o <file>")?;
            return xlsx(report, integrity, quiet, path);
        }
    };

    if format == Format::Pdf {
        let path = out.context("--format pdf needs -o <file>")?;
        return pdf(&body, path);
    }

    match out {
        Some(path) => {
            std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => to_stdout(&body)?,
    }
    Ok(())
}

/// Writes to stdout, treating a closed pipe as a clean finish.
///
/// Rust ignores SIGPIPE, so `println!` panics once a reader like `head`
/// exits. Piping is ordinary use of a CLI and must not look like a crash.
pub fn to_stdout(body: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(body.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
    {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// Whether the opening state is worth showing.
///
/// For an instant it *is* the answer, including when the answer is that
/// nothing was playing. For a window it only says something when playback
/// was already under way as the window opened — a session carried over from
/// the night before. Otherwise it is a heading above the words "nothing
/// playing" on every report, which is the kind of filler that teaches a
/// reader to skip the top of the page.
fn opening_state_matters(report: &Report) -> bool {
    report.window.instant
        || report
            .at_start
            .iter()
            .any(|(_, at)| !at.playing().is_empty())
}

// ---------------------------------------------------------------- helpers

/// Which side of Ruhezeiten a stretch falls on.
///
/// A session that is partly inside counts as inside: it touched the
/// protected hours, which is what the question is about.
pub fn when_of(span: &Span, quiet: &QuietHours) -> &'static str {
    if quiet_seconds(span, quiet) > 0 {
        "quiet hours"
    } else {
        "other times"
    }
}

/// Seconds of a span that fall inside quiet hours.
fn quiet_seconds(span: &Span, quiet: &QuietHours) -> i64 {
    if quiet.is_empty() {
        return 0;
    }
    let tz = span.start.time_zone().clone();
    let mut total = 0;
    let mut day = span.start.date();
    while day <= span.end.date() {
        for (qs, qe) in quiet.intervals(day, &tz).unwrap_or_default() {
            let lo = qs.max(span.start.clone());
            let hi = qe.min(span.end.clone());
            if hi > lo {
                total += secs_between(&lo, &hi).max(0);
            }
        }
        match day.tomorrow() {
            Ok(next) => day = next,
            Err(_) => break,
        }
    }
    total
}

fn days_in(report: &Report) -> Vec<Date> {
    let mut days = Vec::new();
    let mut d = report.window.from.date();
    // A day window ends at midnight of the *next* day. Taking that date
    // literally renders a trailing empty row for a day the window does not
    // actually cover.
    let last = report
        .window
        .to
        .checked_sub(jiff::Span::new().seconds(1))
        .map(|z| z.date())
        .unwrap_or(report.window.to.date())
        .max(report.window.from.date());
    while d <= last && days.len() < 400 {
        days.push(d);
        match d.tomorrow() {
            Ok(next) => d = next,
            Err(_) => break,
        }
    }
    days
}

fn players_in(report: &Report) -> Vec<String> {
    let mut names: Vec<String> = report.spans.iter().map(|s| s.player.clone()).collect();
    names.sort();
    names.dedup();
    names
}

/// Collapses consecutive days that report the same anchor status.
///
/// A window of four months listed a hundred and forty-five rows saying the
/// same thing, which buries the one row that does not. Runs become a single
/// line naming the span; anything that failed verification is always listed
/// on its own, because that is the line a reader has to see.
fn anchor_runs(integrity: &Integrity) -> Vec<(String, String, String)> {
    let mut rows: Vec<(&String, &String, &String)> = integrity
        .anchors
        .iter()
        .map(|(h, d, s)| (h, d, s))
        .collect();
    rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));

    let mut out: Vec<(String, String, String)> = Vec::new();
    let mut run: Option<(String, String, String, String, usize)> = None;

    let flush = |run: Option<(String, String, String, String, usize)>,
                 out: &mut Vec<(String, String, String)>| {
        let Some((host, first, last, status, n)) = run else {
            return;
        };
        let label = if n == 1 {
            human_iso(&first)
        } else {
            format!("{} … {}  ({n} days)", human_iso(&first), human_iso(&last))
        };
        // How long the wait has been, not only that there is one. A run
        // groups days that say the same thing, so the figure belongs to
        // its oldest -- which is the date the label already opens with.
        let status = match status.contains("not yet stamped") {
            false => status,
            true => first
                .parse::<Date>()
                .ok()
                .and_then(|d| crate::anchor::waited(d, &Zoned::now()))
                .map(|w| {
                    let whose = if n == 1 { "" } else { "the oldest " };
                    format!("{status} — {whose}for {w}")
                })
                .unwrap_or(status),
        };
        out.push((host, label, status));
    };

    for (host, date, status) in rows {
        let alone = status.contains("INVALID");
        match run.take() {
            Some((h, first, _, st, n)) if !alone && h == *host && st == *status => {
                run = Some((h, first, date.clone(), st, n + 1));
            }
            other => {
                flush(other, &mut out);
                run = Some((host.clone(), date.clone(), date.clone(), status.clone(), 1));
            }
        }
        if alone {
            flush(run.take(), &mut out);
        }
    }
    flush(run, &mut out);
    out
}
