//! Output formats.
//!
//! Every format carries the integrity footer. A report that needs a separate
//! `verify` run before anyone trusts it will be circulated without one.

use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use jiff::civil::Date;
use jiff::{Span as JiffSpan, Zoned};

use crate::query::{Integrity, Report, Span};
use crate::quiet::QuietHours;

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

/// Categorical slots, assigned per player in fixed order and never cycled.
/// Validated for both surfaces; see docs/design.md.
const SERIES_LIGHT: [&str; 3] = ["#2a78d6", "#eb6834", "#1baf7a"];
const SERIES_DARK: [&str; 3] = ["#3987e5", "#d95926", "#199e70"];
/// Sequential ramp for magnitude: one hue, light to dark.
///
/// Not green-yellow-red. That is a rainbow for what is a magnitude, and
/// red/green is the worst pair for colour vision deficiency — around 8% of
/// men, which may well include the person the document has to persuade. One
/// hue also survives being printed in greyscale, which a traffic light does
/// not.
const RAMP_LIGHT: [&str; 5] = ["#cde2fb", "#9ec5f4", "#5598e7", "#2a78d6", "#184f95"];
const RAMP_DARK: [&str; 5] = ["#1c3252", "#21456f", "#256abf", "#3987e5", "#86b6ef"];
/// Ink that stays legible as the ramp darkens.
const RAMP_INK: [&str; 5] = ["#0b0b0b", "#0b0b0b", "#ffffff", "#ffffff", "#ffffff"];

const PANEL_LIGHT: &str = "#ffffff";
const PANEL_DARK: &str = "#232322";

/// `hex` blended `pct` of the way toward `onto`.
///
/// Computed here rather than with `color-mix()` so the value reaches the PDF:
/// the renderer resolves literal hex and little else.
fn mix(hex: &str, onto: &str, pct: f64) -> String {
    let channel = |s: &str, i: usize| u8::from_str_radix(&s[1 + i * 2..3 + i * 2], 16).unwrap_or(0);
    let blend = |i: usize| {
        (channel(hex, i) as f64 * pct + channel(onto, i) as f64 * (1.0 - pct)).round() as u8
    };
    format!("#{:02x}{:02x}{:02x}", blend(0), blend(1), blend(2))
}

/// How the calendar maps gain to colour.
///
/// A default rather than a fact: until the Bluetooth machine is measured,
/// nobody knows what ordinary and loud actually read as there, so this is a
/// guess and is configurable precisely because it is one.
#[derive(Copy, Clone, Debug)]
pub struct GainScale(pub f64, pub f64);

impl Default for GainScale {
    fn default() -> Self {
        GainScale(0.05, 0.35)
    }
}

impl std::str::FromStr for GainScale {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let (lo, hi) = s
            .split_once(':')
            .context("gain scale is LOW:HIGH, for example 0.05:0.35")?;
        let (lo, hi): (f64, f64) = (lo.trim().parse()?, hi.trim().parse()?);
        // NaN compares false either way, so test the ordering we require
        // rather than negating the one we do not.
        if !matches!(lo.partial_cmp(&hi), Some(std::cmp::Ordering::Less)) {
            bail!("gain scale needs HIGH above LOW: {s}");
        }
        Ok(GainScale(lo, hi))
    }
}

pub fn render(
    report: &Report,
    integrity: &Integrity,
    quiet: &QuietHours,
    format: Format,
    out: Option<&Path>,
    scale: GainScale,
) -> Result<()> {
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

/// Whole seconds between two instants.
///
/// `Span::get_seconds` returns only the seconds *component* of a calendar
/// span, so a 40 minute stretch reads as zero. Totalling in seconds is what
/// is actually meant.
pub fn secs_between(a: &Zoned, b: &Zoned) -> i64 {
    (b - a).total(jiff::Unit::Second).unwrap_or(0.0).round() as i64
}

fn duration(span: &Span) -> String {
    human_secs(secs_between(&span.start, &span.end).max(0))
}

fn human_secs(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

fn hhmmss(z: &Zoned) -> String {
    z.strftime("%H:%M:%S").to_string()
}

/// Times carry their date once a window spans more than one day.
///
/// A bare "21:10:00" in a three-day table says nothing about which day, which
/// is exactly the ambiguity a noise complaint turns on.
fn clock(z: &Zoned, multiday: bool) -> String {
    if multiday {
        z.strftime("%m-%d %H:%M:%S").to_string()
    } else {
        z.strftime("%H:%M:%S").to_string()
    }
}

fn multiday(report: &Report) -> bool {
    days_in(report).len() > 1
}

/// RFC 3339, matching the log's own `ts` field.
///
/// Not jiff's `Display`, which appends the IANA annotation
/// (`...+02:00[Europe/Berlin]`) -- correct but something spreadsheets and
/// dataframe libraries refuse to parse, which defeats the point of csv and
/// xlsx existing at all.
fn rfc3339(z: &Zoned) -> String {
    z.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
            first
        } else {
            format!("{first} … {last}  ({n} days)")
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

// ------------------------------------------------------------------- text

fn text(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
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
            if playing.is_empty() {
                o.push_str("    nothing playing\n");
            } else {
                for p in playing {
                    o.push_str(&format!(
                        "    {} playing: {}\n",
                        p.player,
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
        o.push_str(&footer_text(integrity, report));
        return Ok(o);
    }

    let wide = multiday(report);
    o.push_str("\nPlayback\n");
    if report.spans.is_empty() {
        o.push_str("  nothing played in this window\n");
    }
    for span in &report.spans {
        let q = quiet_seconds(span, quiet);
        o.push_str(&format!(
            "  {}–{}  {:>7}  {:10} {}\n",
            clock(&span.start, wide),
            clock(&span.end, wide),
            duration(span),
            span.player,
            span.label()
        ));
        if let Some(l) = span.levels {
            o.push_str(&format!(
                "  {:width$}gain {:.3} min / {:.3} med / {:.3} max  (n={}){}\n",
                "",
                l.min,
                l.median,
                l.max,
                l.samples,
                span.sink_label
                    .as_deref()
                    .or(span.sink.as_deref())
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default(),
                width = if wide { 36 } else { 30 },
            ));
        }
        if q > 0 {
            o.push_str(&format!(
                "  {:width$}within quiet hours for {}\n",
                "",
                human_secs(q),
                width = if wide { 36 } else { 30 },
            ));
        }
    }

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

    o.push_str(&footer_text(integrity, report));
    Ok(o)
}

fn footer_text(integrity: &Integrity, report: &Report) -> String {
    let mut o = String::from("\nIntegrity\n");
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
    o
}

// ------------------------------------------------------------------- json

fn json(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
    let spans: Vec<_> = report
        .spans
        .iter()
        .map(|s| {
            serde_json::json!({
                "host": s.host,
                "start": rfc3339(&s.start),
                "end": rfc3339(&s.end),
                "duration_s": secs_between(&s.start, &s.end).max(0),
                "player": s.player,
                "title": s.title,
                "artist": s.artist,
                "url": s.url,
                "sink": s.sink,
                        "host": s.host, "gain_linear": s.levels.map(|l| serde_json::json!({
                    "min": l.min, "median": l.median, "max": l.max, "samples": l.samples,
                })),
                "quiet_hours_s": quiet_seconds(s, quiet),
            })
        })
        .collect();

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "hosts": report.hosts,
        "window": {
            "label": report.window.label,
            "from": rfc3339(&report.window.from),
            "to": rfc3339(&report.window.to),
            "instant": report.window.instant,
        },
        "quiet_hours": quiet.describe(),
        "at_start": report.at_start.iter().map(|(host, at)| serde_json::json!({
            "host": host,
            "as_of": at.as_of,
            "players": at.players,
            "sink": at.sink,
        })).collect::<Vec<_>>(),
        "spans": spans,
        "integrity": {
            "chain_ok": integrity.ok(),
            "lines": integrity.lines(),
            "problems": integrity.problems(),
            "anchors": integrity.anchors.iter()
                .map(|(h, d, s)| serde_json::json!({"host": h, "date": d, "status": s}))
                .collect::<Vec<_>>(),
        },
    }))?)
}

// ------------------------------------------------------------------ table
//
// csv and xlsx are the same table in two encodings, so the columns and the
// values are defined once here and each renderer only decides how to write
// them. They were written twice before and drifted: the xlsx kept a "Peak
// volume" header after the data underneath had become min/median/max.
//
// The document formats (text, html, pdf) are not this table -- they group
// and stack columns for a printed page -- so they stay separate on purpose.

pub struct Column {
    /// Machine name, used as the csv header.
    key: &'static str,
    /// Human name, used as the spreadsheet header.
    title: &'static str,
    width: f64,
}

/// A value, not a rendering: each format decides how to write it.
enum Cell {
    Text(String),
    Num(f64),
    Count(u64),
    /// Seconds. Raw in csv for analysis, human in xlsx for reading.
    Duration(i64),
    /// Seconds inside quiet hours; zero means none, and is flagged when not.
    Quiet(i64),
    Empty,
}

const COLUMNS: &[Column] = &[
    Column {
        key: "host",
        title: "Host",
        width: 12.0,
    },
    Column {
        key: "start",
        title: "Start",
        width: 24.0,
    },
    Column {
        key: "end",
        title: "End",
        width: 24.0,
    },
    Column {
        key: "duration_s",
        title: "Duration",
        width: 10.0,
    },
    Column {
        key: "player",
        title: "Player",
        width: 10.0,
    },
    Column {
        key: "title",
        title: "Title",
        width: 40.0,
    },
    Column {
        key: "artist",
        title: "Artist",
        width: 22.0,
    },
    Column {
        key: "url",
        title: "URL",
        width: 46.0,
    },
    Column {
        key: "sink",
        title: "Output",
        width: 40.0,
    },
    Column {
        key: "gain_min",
        title: "Gain min",
        width: 10.0,
    },
    Column {
        key: "gain_median",
        title: "Gain median",
        width: 12.0,
    },
    Column {
        key: "gain_max",
        title: "Gain max",
        width: 10.0,
    },
    Column {
        key: "gain_samples",
        title: "Samples",
        width: 9.0,
    },
    Column {
        key: "quiet_hours_s",
        title: "In quiet hours",
        width: 14.0,
    },
];

fn rows(report: &Report, quiet: &QuietHours) -> Vec<Vec<Cell>> {
    report
        .spans
        .iter()
        .map(|s| {
            let gain = |pick: fn(&crate::query::Levels) -> f64| {
                s.levels.map(|l| Cell::Num(pick(&l))).unwrap_or(Cell::Empty)
            };
            vec![
                Cell::Text(s.host.clone()),
                Cell::Text(rfc3339(&s.start)),
                Cell::Text(rfc3339(&s.end)),
                Cell::Duration(secs_between(&s.start, &s.end).max(0)),
                Cell::Text(s.player.clone()),
                Cell::Text(s.title.clone().unwrap_or_default()),
                Cell::Text(s.artist.join("; ")),
                Cell::Text(s.url.clone().unwrap_or_default()),
                Cell::Text(s.sink.clone().unwrap_or_default()),
                gain(|l| l.min),
                gain(|l| l.median),
                gain(|l| l.max),
                s.levels
                    .map(|l| Cell::Count(l.samples as u64))
                    .unwrap_or(Cell::Empty),
                Cell::Quiet(quiet_seconds(s, quiet)),
            ]
        })
        .collect()
}

// -------------------------------------------------------------------- csv

fn csv(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
    let quote = |v: &str| {
        if v.contains([',', '"', '\n']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_owned()
        }
    };

    let mut o = COLUMNS.iter().map(|c| c.key).collect::<Vec<_>>().join(",");
    o.push('\n');

    for row in rows(report, quiet) {
        let cells: Vec<String> = row
            .iter()
            .map(|cell| match cell {
                Cell::Text(v) => quote(v),
                Cell::Num(v) => format!("{v:.4}"),
                Cell::Count(v) => v.to_string(),
                // Raw seconds: a spreadsheet can format them, a script
                // cannot un-format "2h20m".
                Cell::Duration(v) | Cell::Quiet(v) => v.to_string(),
                Cell::Empty => String::new(),
            })
            .collect();
        o.push_str(&cells.join(","));
        o.push('\n');
    }

    // The footer travels with the data, as a comment the spreadsheet shows.
    o.push_str(&format!(
        "\n# chain {} ({} lines)\n",
        if integrity.ok() { "intact" } else { "BROKEN" },
        integrity.lines()
    ));
    for (host, span, state) in anchor_runs(integrity) {
        o.push_str(&format!("# {host} {span}: {state}\n"));
    }
    Ok(o)
}

// ------------------------------------------------------------------- xlsx

fn xlsx(report: &Report, integrity: &Integrity, quiet: &QuietHours, path: &Path) -> Result<()> {
    use rust_xlsxwriter::{Format as XFormat, Workbook};

    let mut book = Workbook::new();
    let head = XFormat::new().set_bold();
    let flag = XFormat::new().set_bold().set_font_color("#B03030");

    let sheet = book.add_worksheet().set_name("Playback")?;
    for (i, column) in COLUMNS.iter().enumerate() {
        sheet.write_with_format(0, i as u16, column.title, &head)?;
        sheet.set_column_width(i as u16, column.width)?;
    }

    for (r, row) in rows(report, quiet).iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let (r, c) = (r as u32 + 1, c as u16);
            match cell {
                Cell::Text(v) => sheet.write(r, c, v.as_str())?,
                // Numbers stay numbers, so a spreadsheet can chart or
                // average them without anyone re-typing the column.
                Cell::Num(v) => sheet.write(r, c, *v)?,
                Cell::Count(v) => sheet.write(r, c, *v as f64)?,
                Cell::Duration(v) => sheet.write(r, c, human_secs(*v))?,
                Cell::Quiet(0) => sheet.write(r, c, "")?,
                Cell::Quiet(v) => sheet.write_with_format(r, c, human_secs(*v), &flag)?,
                Cell::Empty => sheet.write(r, c, "")?,
            };
        }
    }

    // A separate sheet, because an integrity note mixed into the data rows
    // would be sorted away the first time somebody sorted the table.
    let meta = book.add_worksheet().set_name("Integrity")?;
    let mut row = 0;
    let mut put = |a: &str, b: String| -> Result<()> {
        meta.write(row, 0, a)?;
        meta.write(row, 1, b)?;
        row += 1;
        Ok(())
    };
    put("Hosts", report.hosts.join(", "))?;
    put("Window", report.window.label.clone())?;
    put("Quiet hours", quiet.describe().join(", "))?;
    put(
        "Chain",
        format!(
            "{} ({} lines)",
            if integrity.ok() { "intact" } else { "BROKEN" },
            integrity.lines()
        ),
    )?;
    for p in &integrity.problems() {
        put("Problem", p.clone())?;
    }
    for (host, span, state) in anchor_runs(integrity) {
        put(&format!("{host} {span}"), state)?;
    }
    meta.set_column_width(0, 22)?;
    meta.set_column_width(1, 70)?;

    book.save(path)
        .with_context(|| format!("writing {}", path.display()))?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

// -------------------------------------------------------------------- pdf

fn pdf(html_body: &str, path: &Path) -> Result<()> {
    let scratch = std::env::temp_dir().join(format!("media-logger-{}.html", std::process::id()));
    std::fs::write(&scratch, html_body)?;

    let out = std::process::Command::new("weasyprint")
        .arg(&scratch)
        .arg(path)
        .output();
    let _ = std::fs::remove_file(&scratch);

    match out {
        Ok(o) if o.status.success() => {
            eprintln!("wrote {}", path.display());
            Ok(())
        }
        Ok(o) => bail!(
            "weasyprint failed: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .next()
                .unwrap_or("")
        ),
        Err(e) => bail!(
            "could not run weasyprint ({e}). Render --format html and print it, \
             or install weasyprint."
        ),
    }
}

/// Reads the gain at the cursor's position along a bar.
///
/// The marks are a couple of pixels wide at day scale, so they are far too
/// small to be hover targets themselves; the bar is the target and the
/// position within it selects the value. Inert in the PDF, which is why the
/// figures also appear in the table.
const FILTERS: &str = r#"
<script>
(function () {
  var boxes = Array.prototype.slice.call(
    document.querySelectorAll('#filters input[type=checkbox]'));
  var days = Array.prototype.slice.call(document.querySelectorAll('.cal-day'));
  var bars = Array.prototype.slice.call(document.querySelectorAll('.bar'));
  var rows = Array.prototype.slice.call(document.querySelectorAll('tbody tr[data-host]'));
  var clear = document.getElementById('clear');
  var shown = document.getElementById('shown');
  if (!bars.length && !days.length) return;
  var dates = [];

  function short(v) {
    return v.length > 22 ? v.slice(0, 21) + '\u2026' : v;
  }
  function wanted(kind) {
    var all = boxes.filter(function (b) { return b.dataset.kind === kind; });
    if (!all.length) return null;
    var on = all.filter(function (b) { return b.checked; }).map(function (b) { return b.value; });
    return on.length === all.length ? null : on;
  }
  // What kind of thing is shown, as chosen in the dropdowns.
  function matchesDimensions(el) {
    return ['host', 'player', 'device'].every(function (k) {
      var want = wanted(k);
      return !want || want.indexOf(el.dataset[k]) >= 0;
    });
  }
  // Which day, as chosen in the calendar. Kept separate: picking a day is
  // navigation, and recolouring the calendar by it would blank every other
  // cell and destroy the overview being navigated with.
  function matchesDate(el) {
    return !dates.length || dates.indexOf(el.dataset.date) >= 0;
  }
  function keep(el) {
    return matchesDimensions(el) && matchesDate(el);
  }
  // The calendar must say the same thing as the rows below it, so it is
  // recoloured from whichever rows survive rather than left showing the
  // unfiltered day.
  var months = document.querySelector('.months');
  var scale = months && {
    lo: parseFloat(months.dataset.lo),
    hi: parseFloat(months.dataset.hi),
    steps: parseInt(months.dataset.steps, 10)
  };
  function recolour() {
    if (!scale) return;
    var best = {}, quiet = {};
    rows.forEach(function (r) {
      // Dimensions only: the day selection must not recolour the calendar.
      if (!matchesDimensions(r)) return;
      var d = r.dataset.date;
      var v = parseFloat(r.dataset.sustained);
      if (!isNaN(v)) best[d] = Math.max(best[d] === undefined ? -Infinity : best[d], v);
      if (parseInt(r.dataset.quiet, 10) > 0) quiet[d] = true;
    });
    days.forEach(function (cell) {
      var d = cell.dataset.date;
      var v = best[d];
      var dot = cell.querySelector('.cal-dot');
      if (v === undefined) {
        cell.style.background = 'transparent';
        cell.style.color = 'var(--ink-3)';
      } else {
        var t = (v - scale.lo) / Math.max(1e-9, scale.hi - scale.lo);
        var i = Math.min(scale.steps - 1, Math.max(0, Math.round(t * (scale.steps - 1))));
        cell.style.background = 'var(--ramp-' + i + ')';
        cell.style.color = 'var(--ramp-ink-' + i + ')';
      }
      if (dot) dot.hidden = !quiet[d];
    });
  }

  function apply() {
    var visible = 0;
    bars.forEach(function (b) { b.hidden = !keep(b); });
    rows.forEach(function (r) { r.hidden = !keep(r); if (!r.hidden) visible++; });
    // A timeline row with nothing left in it is noise, not evidence.
    document.querySelectorAll('.row').forEach(function (row) {
      var any = row.querySelector('.bar:not([hidden])');
      if (row.querySelector('.bar')) row.hidden = !any;
    });
    // A month heading standing over no rows is worse than noise: it says a
    // month is covered while showing nothing from it.
    document.querySelectorAll('.tl-month').forEach(function (head) {
      var axis = head.nextElementSibling;
      var used = false;
      for (var el = axis; el; el = el.nextElementSibling) {
        if (el.classList.contains('tl-month')) break;
        if (el.classList.contains('row') && !el.hidden) { used = true; break; }
      }
      head.hidden = !used;
      if (axis && axis.classList.contains('axis')) axis.hidden = !used;
    });
    days.forEach(function (d) {
      d.setAttribute('aria-pressed', dates.indexOf(d.dataset.date) >= 0 ? 'true' : 'false');
    });
    document.querySelectorAll('.pick').forEach(function (group) {
      var all = Array.prototype.slice.call(group.querySelectorAll('input'));
      var on = all.filter(function (x) { return x.checked; });
      var label;
      // Name the selection when naming it is short enough to read; fall
      // back to a count only when it is not.
      if (!on.length) label = 'none';
      else if (on.length === all.length) label = 'all';
      else if (on.length === 1) label = short(on[0].value);
      else label = on.length + ' of ' + all.length;
      group.querySelector('.count').textContent = ': ' + label;
    });
    recolour();
    var filtered = dates.length || boxes.some(function (b) { return !b.checked; });
    clear.hidden = !filtered;
    shown.textContent = filtered ? visible + ' of ' + rows.length + ' shown' : '';
  }

  boxes.forEach(function (b) { b.addEventListener('change', apply); });

  // Clicking a name means "show only this", and again means "show
  // everything" — the question being asked most of the time. The checkbox
  // and shift-click stay available for building a set by hand.
  document.querySelectorAll('.pick').forEach(function (group) {
    var all = Array.prototype.slice.call(group.querySelectorAll('input'));
    group.querySelectorAll('label').forEach(function (lab) {
      lab.addEventListener('click', function (e) {
        if (e.target.tagName === 'INPUT') return;   // the box toggles itself
        e.preventDefault();
        var box = lab.querySelector('input');
        if (e.shiftKey) {
          box.checked = !box.checked;
        } else {
          var soloed = box.checked && all.every(function (x) {
            return x === box || !x.checked;
          });
          all.forEach(function (x) { x.checked = soloed || x === box; });
        }
        apply();
      });
    });
  });
  // A details element stays open until its own summary is clicked again,
  // which is wrong for something that behaves like a menu.
  document.addEventListener('click', function (e) {
    document.querySelectorAll('.pick[open]').forEach(function (group) {
      if (!group.contains(e.target)) group.open = false;
    });
  });
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') return;
    document.querySelectorAll('.pick[open]').forEach(function (g) { g.open = false; });
  });
  function pickDay(cell, extend) {
    var when = cell.dataset.date;
    if (extend) {
      var i = dates.indexOf(when);
      if (i >= 0) dates.splice(i, 1); else dates.push(when);
    } else {
      // Absolute: this day alone, or back to all of them.
      dates = (dates.length === 1 && dates[0] === when) ? [] : [when];
    }
    apply();
  }
  days.forEach(function (d) {
    d.addEventListener('click', function (e) { pickDay(d, e.shiftKey); });
    // The cells are focusable, so they have to answer the keyboard too.
    d.addEventListener('keydown', function (e) {
      if (e.key !== 'Enter' && e.key !== ' ') return;
      e.preventDefault();
      pickDay(d, e.shiftKey);
    });
  });
  clear.addEventListener('click', function () {
    dates = [];
    boxes.forEach(function (b) { b.checked = true; });
    apply();
  });
  apply();
})();
</script>
"#;

const HOVER: &str = r#"<div id="tip" role="status" aria-live="polite"></div>
<script>
(function () {
  var tip = document.getElementById('tip');
  // With the script running, the native title would duplicate the tooltip
  // on a still cursor; without it, the attribute is the whole fallback.
  document.querySelectorAll('.bar[title]').forEach(function (bar) {
    bar.dataset.label = bar.getAttribute('title');
    bar.removeAttribute('title');
  });
  function read(bar, frac) {
    var g = (bar.dataset.g || '').split(',');
    var s0 = +bar.dataset.s0, s1 = +bar.dataset.s1;
    var i = Math.min(g.length - 1, Math.max(0, Math.floor(frac * g.length)));
    var when = new Date((s0 + (s1 - s0) * ((i + 0.5) / g.length)) * 1000);
    var hh = String(when.getHours()).padStart(2, '0');
    var mm = String(when.getMinutes()).padStart(2, '0');
    var gain = g[i] ? ' \u00b7 gain ' + g[i] : '';
    return (bar.dataset.label || '') + '\n' + hh + ':' + mm + gain;
  }
  document.addEventListener('mousemove', function (e) {
    var bar = e.target.closest ? e.target.closest('.bar') : null;
    if (!bar) { tip.style.opacity = 0; return; }
    var box = bar.getBoundingClientRect();
    var parts = read(bar, (e.clientX - box.left) / Math.max(1, box.width)).split('\n');
    tip.textContent = '';
    var b = document.createElement('b'); b.textContent = parts[0];
    var s = document.createElement('i'); s.textContent = parts[1];
    tip.appendChild(b); tip.appendChild(s);
    tip.style.opacity = 1;
    tip.style.left = Math.min(window.innerWidth - tip.offsetWidth - 12, e.clientX + 12) + 'px';
    tip.style.top = Math.max(4, e.clientY - tip.offsetHeight - 10) + 'px';
  });
  document.addEventListener('mouseleave', function () { tip.style.opacity = 0; });
})();
</script>
"#;

// ------------------------------------------------------------------- html

fn html(
    report: &Report,
    integrity: &Integrity,
    quiet: &QuietHours,
    scale: GainScale,
) -> Result<String> {
    let players = players_in(report);
    let colour = |name: &str, dark: bool| -> &'static str {
        let i = players.iter().position(|p| p == name).unwrap_or(0) % SERIES_LIGHT.len();
        if dark {
            SERIES_DARK[i]
        } else {
            SERIES_LIGHT[i]
        }
    };

    let mut o = String::new();
    o.push_str(&format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>media-logger — {label} — {host}</title>
<style>
:root {{
  color-scheme: light;
  --surface: #fcfcfb; --panel: #ffffff;
  --ink: #0b0b0b; --ink-2: #52514e; --ink-3: #78766f;
  --rule: #e3e2dd; --quiet: #ebe9e3; --quiet-ink: #6d6a62;
  --flag: #b03030; --link: #4a3aa7;
{light_ramp}
{light_series}
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    color-scheme: dark;
    --surface: #1a1a19; --panel: #232322;
    --ink: #ffffff; --ink-2: #c3c2b7; --ink-3: #96958c;
    --rule: #3a3a38; --quiet: #2c2c2a; --quiet-ink: #9a988f;
    --flag: #e66767; --link: #9085e9;
{dark_ramp}
{dark_ramp}
{dark_series}
  }}
}}
:root[data-theme="dark"] {{
  color-scheme: dark;
  --surface: #1a1a19; --panel: #232322;
  --ink: #ffffff; --ink-2: #c3c2b7; --ink-3: #96958c;
  --rule: #3a3a38; --quiet: #2c2c2a; --quiet-ink: #9a988f;
  --flag: #e66767; --link: #9085e9;
{dark_series}
}}
* {{ box-sizing: border-box; }}
body {{ margin: 0; background: var(--surface); color: var(--ink);
  font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }}
main {{ max-width: 60rem; margin: 0 auto; padding: 2rem 1.25rem 4rem; }}
h1 {{ font-size: 1.4rem; margin: 0 0 .25rem; letter-spacing: -.01em; }}
h2 {{ font-size: 1rem; margin: 2.25rem 0 .75rem; letter-spacing: -.01em; }}
.sub {{ color: var(--ink-2); margin: 0 0 1.5rem; }}
.legend {{ display: flex; flex-wrap: wrap; gap: 1rem; margin: 0 0 1rem; color: var(--ink-2); }}
.key {{ display: inline-flex; align-items: center; gap: .4rem; white-space: nowrap; }}
.swatch {{ width: 12px; height: 12px; border-radius: 3px; }}
.axis {{ display: flex; color: var(--ink-3); font-size: 11px;
  font-variant-numeric: tabular-nums; margin-left: 4.25rem; }}
.axis span {{ flex: 1 1 0; }}
.row {{ display: flex; align-items: center; gap: .75rem; margin: .35rem 0; }}
.day {{ width: 3.5rem; flex: none; color: var(--ink-2); font-size: 12px;
  font-variant-numeric: tabular-nums; line-height: 1.25; text-align: right; }}
.day b {{ font-weight: 600; }}
.day em {{ font-style: normal; color: var(--ink-3); font-weight: 400; }}
.day i {{ display: block; font-style: normal; color: var(--ink-3);
  font-size: 10.5px; }}
.tl-month {{ font-size: 12px; font-weight: 600; color: var(--ink-2);
  margin: 1.25rem 0 .35rem; }}
.tl-month:first-of-type {{ margin-top: .25rem; }}
.track {{ position: relative; flex: 1 1 auto; height: 34px;
  background: var(--panel); border: 1px solid var(--rule); border-radius: 4px;
  overflow: hidden; }}
.band {{ position: absolute; top: 0; bottom: 0; background: var(--quiet); }}
.hour {{ position: absolute; top: 0; bottom: 0; width: 1px; background: var(--rule);
  opacity: .55; }}
.bar {{ position: absolute; top: 4px; height: 26px; border-radius: 3px;
  box-shadow: 0 0 0 2px var(--panel); min-width: 2px; overflow: hidden;
  background: var(--cs); }}
/* The soft block is the span's extent; the solid fill inside is its gain.
   One scale, shared by every bar, so the two readings cannot be confused. */
.bar i {{ position: absolute; bottom: 0; display: block; background: var(--c); }}
.bar span {{ position: absolute; left: 5px; top: 2px; font-size: 10.5px;
  color: var(--ink); white-space: nowrap; pointer-events: none;
  text-shadow: 0 0 3px var(--panel), 0 0 3px var(--panel); }}
.gainkey {{ background: linear-gradient(to top, var(--ink-2) 55%, var(--rule) 55%); }}
#tip {{ position: fixed; z-index: 10; pointer-events: none; opacity: 0;
  transition: opacity .08s; background: var(--ink); color: var(--surface);
  font-size: 11.5px; padding: .35rem .5rem; border-radius: 4px;
  white-space: nowrap; font-variant-numeric: tabular-nums; line-height: 1.45; }}
#tip b {{ display: block; font-weight: 600; }}
#tip i {{ display: block; font-style: normal; opacity: .75; }}
@media (prefers-reduced-motion: reduce) {{ #tip {{ transition: none; }} }}
table {{ border-collapse: collapse; width: 100%; font-size: 12.5px; }}
th, td {{ text-align: left; padding: .45rem .6rem; border-bottom: 1px solid var(--rule);
  vertical-align: top; }}
th {{ color: var(--ink-2); font-weight: 600; }}
td.num {{ font-variant-numeric: tabular-nums; white-space: nowrap; }}
/* Period and gain each stack their values, which keeps seven columns on a
   printed page where nine overflowed. */
td .sub2 {{ display: block; color: var(--ink-3); }}
td.stack span {{ display: block; line-height: 1.35; }}
td.stack u {{ text-decoration: none; color: var(--ink-3); font-size: 10.5px;
  margin-left: .25rem; }}
/* The source under the title: a path identifies the exact file, and a URL
   has to stay visible because in print the text is all a reader gets. */
.src {{ display: block; margin-top: .15rem; font-size: 10.5px;
  color: var(--ink-3); overflow-wrap: anywhere; }}
a.src {{ color: var(--link); }}
tbody td:nth-child(5) {{ min-width: 6rem; }}
.flag {{ color: var(--flag); font-weight: 600; }}
.muted {{ color: var(--ink-3); }}
footer {{ margin-top: 2.5rem; padding-top: 1rem; border-top: 1px solid var(--rule);
  color: var(--ink-2); font-size: 12.5px; }}
footer b {{ color: var(--ink); }}
.scroll {{ overflow-x: auto; }}
.filters {{ display: flex; flex-wrap: wrap; gap: .5rem; align-items: center;
  margin: 0 0 1rem; }}
.pick {{ position: relative; }}
.pick summary {{ cursor: pointer; list-style: none; padding: .25rem .6rem;
  border: 1px solid var(--rule); border-radius: 999px; font-size: 12px;
  color: var(--ink-2); background: var(--panel); user-select: none; }}
.pick summary::-webkit-details-marker {{ display: none; }}
.pick[open] summary {{ border-color: var(--ink-3); }}
.pick .count {{ color: var(--ink-3); }}
.pick .count:empty {{ display: none; }}
.pick > div {{ position: absolute; z-index: 5; margin-top: .25rem;
  background: var(--panel); border: 1px solid var(--rule); border-radius: 6px;
  padding: .4rem; min-width: 11rem; max-height: 15rem; overflow-y: auto;
  box-shadow: 0 6px 18px rgb(0 0 0 / .12); }}
.pick label {{ display: flex; gap: .45rem; align-items: center; font-size: 12.5px;
  padding: .18rem .25rem; white-space: nowrap; cursor: pointer; }}
#clear {{ font: inherit; font-size: 12px; padding: .25rem .6rem; cursor: pointer;
  border: 1px solid var(--rule); border-radius: 999px; background: var(--panel);
  color: var(--ink-2); }}
#shown {{ font-size: 12px; }}
tr[hidden], .bar[hidden], .row[hidden],
.axis[hidden], .tl-month[hidden] {{ display: none !important; }}
@media print {{ .filters {{ display: none; }} }}
.cal {{ border-collapse: separate; border-spacing: 3px;
  table-layout: fixed; width: 17.5rem; }}
.cal caption {{ text-align: left; font-size: 12px; color: var(--ink-2);
  padding: .5rem 0 .15rem; font-weight: 600; }}
/* Each month keeps its own height. As flex items they stretch to the
   tallest by default, so a five-week month beside a six-week one grew its
   rows to match. */
.months {{ display: flex; flex-wrap: wrap; gap: 0 1.75rem; align-items: flex-start; }}
.cal th {{ font-size: 10.5px; color: var(--ink-3); text-align: center;
  font-weight: 500; padding: 0 0 .15rem; border: none; }}
.cal td {{ padding: 0; border: none; width: 2.1rem; }}
.cal th {{ width: 2.1rem; }}
.cal-day {{ position: relative; width: 2.1rem; height: 2.1rem;
  border: 1px solid var(--rule); border-radius: 4px; font-size: 11px;
  font-variant-numeric: tabular-nums; text-align: center;
  vertical-align: middle; cursor: pointer; }}
.cal-day[aria-pressed="true"] {{ outline: 2px solid var(--ink); outline-offset: 1px; }}
.cal-dot {{ position: absolute; bottom: 3px; right: 3px; width: 4px; height: 4px;
  border-radius: 50%; background: currentColor; opacity: .55; }}
.cal-dot-key {{ background: var(--ink-3); border-radius: 50%; width: 6px; height: 6px; }}
.ramp {{ width: 72px; height: 10px; border-radius: 3px;
  background: linear-gradient(to right, var(--ramp-0), var(--ramp-4)); }}
@media print {{ .cal-day {{ print-color-adjust: exact; }} }}
@media print {{
  body {{ background: #fff; }}
  main {{ max-width: none; padding: 0; }}
  .bar {{ box-shadow: 0 0 0 2px #fff; print-color-adjust: exact; }}
  .band, .swatch {{ print-color-adjust: exact; }}
}}
</style></head><body><main>
<h1>Media playback log</h1>
<p class="sub">{label} · host <b>{host}</b>{quiet_note}</p>
"#,
        label = esc(&report.window.label),
        host = esc(&report.hosts.join(", ")),
        quiet_note = if quiet.is_empty() {
            String::new()
        } else {
            format!(" · quiet hours {}", esc(&quiet.describe().join(", ")))
        },
        light_ramp = RAMP_LIGHT
            .iter()
            .zip(RAMP_INK)
            .enumerate()
            .map(|(i, (c, ink))| format!("  --ramp-{i}: {c}; --ramp-ink-{i}: {ink};"))
            .collect::<Vec<_>>()
            .join("\n"),
        dark_ramp = RAMP_DARK
            .iter()
            .zip(["#ffffff", "#ffffff", "#ffffff", "#0b0b0b", "#0b0b0b"])
            .enumerate()
            .map(|(i, (c, ink))| format!("    --ramp-{i}: {c}; --ramp-ink-{i}: {ink};"))
            .collect::<Vec<_>>()
            .join("\n"),
        light_series = SERIES_LIGHT
            .iter()
            .enumerate()
            .map(|(i, c)| format!(
                "  --series-{n}: {c};\n  --series-{n}-soft: {};",
                mix(c, PANEL_LIGHT, 0.20),
                n = i + 1
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        dark_series = SERIES_DARK
            .iter()
            .enumerate()
            .map(|(i, c)| format!(
                "    --series-{n}: {c};\n    --series-{n}-soft: {};",
                mix(c, PANEL_DARK, 0.28),
                n = i + 1
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    ));

    // -- opening state -----------------------------------------------------
    if opening_state_matters(report) {
        o.push_str("<h2>State at the start of the window</h2>\n");
        for (host, at) in &report.at_start {
            o.push_str(&format!("<p><b>{}</b><br>", esc(host)));
            let playing = at.playing();
            if playing.is_empty() {
                o.push_str("Nothing playing.");
            } else {
                o.push_str("Playing: ");
                o.push_str(
                    &playing
                        .iter()
                        .map(|p| {
                            esc(&format!(
                                "{} — {}",
                                p.player,
                                p.track.title.clone().unwrap_or_else(|| "(untitled)".into())
                            ))
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if let Some(s) = &at.sink {
                o.push_str(&format!(
                    "<br>Output: {} ({}) at volume {} <span class=\"muted\">{}</span>",
                    esc(s.description.as_deref().unwrap_or(&s.name)),
                    esc(s.device_api.as_deref().unwrap_or("?")),
                    s.effective()
                        .iter()
                        .map(|v| format!("{v:.3}"))
                        .collect::<Vec<_>>()
                        .join(" / "),
                    esc(&s.volume_scale),
                ));
            }
            if let Some(as_of) = &at.as_of {
                o.push_str(&format!(
                    "<br><span class=\"muted\">state as of {}</span>",
                    esc(as_of)
                ));
            }
            o.push_str("</p>\n");
        }
    }

    if !report.window.instant {
        // Calendar first when there is one: it is the overview, and the
        // timeline below it keeps every detail rather than being replaced.
        o.push_str(&filter_bar(report));
        o.push_str(&calendar(report, quiet, scale)?);
        o.push_str(&timeline(report, quiet, &players, &colour)?);
        o.push_str(&spans_table(report, quiet));
    }

    o.push_str(&footer_html(integrity, report, quiet));
    if !report.levels.is_empty() {
        o.push_str(HOVER);
    }
    if !report.window.instant {
        o.push_str(FILTERS);
    }
    o.push_str("</main></body></html>\n");
    Ok(o)
}

fn timeline(
    report: &Report,
    quiet: &QuietHours,
    players: &[String],
    colour: &dyn Fn(&str, bool) -> &'static str,
) -> Result<String> {
    let tz = report.window.from.time_zone().clone();
    // Scale to 1.0 unless something exceeded it, so the lane is comparable
    // across reports rather than rescaling to whatever happened that day.
    let ceiling = report
        .levels
        .iter()
        .map(|(_, _, v)| *v)
        .fold(1.0_f64, f64::max);
    let mut o = String::from("<h2>Timeline</h2>\n");

    // A single series needs no legend -- the title names it -- but the
    // shading is an annotation the reader cannot otherwise decode, so its
    // key is always present.
    o.push_str("<div class=\"legend\">");
    if players.len() >= 2 {
        for p in players {
            o.push_str(&format!(
                "<span class=\"key\"><span class=\"swatch\" style=\"background:{}\"></span>{}</span>",
                colour(p, false),
                esc(p)
            ));
        }
    }
    if !quiet.is_empty() {
        o.push_str(
            "<span class=\"key\"><span class=\"swatch\" style=\"background:var(--quiet);border:1px solid var(--rule)\"></span>quiet hours</span>",
        );
    }
    if !report.levels.is_empty() {
        // The swatch already shows a part-filled block, so "fill height ="
        // only restates it. The scale is named only when it is not the
        // usual 0–1, where leaving it out would misread the heights.
        let scale = if (ceiling - 1.0).abs() < 1e-9 {
            String::new()
        } else {
            format!(" (0–{ceiling:.2})")
        };
        o.push_str(&format!(
            "<span class=\"key\"><span class=\"swatch gainkey\"></span>gain{scale}</span>"
        ));
    }
    o.push_str("</div>\n");

    o.push_str("<div class=\"scroll\">");

    let pct = |z: &Zoned, day: Date| -> f64 {
        let Ok(start) = day.to_zoned(tz.clone()) else {
            return 0.0;
        };
        let Ok(end) = start.checked_add(JiffSpan::new().days(1)) else {
            return 0.0;
        };
        let span = secs_between(&start, &end).max(1) as f64;
        (secs_between(&start, z) as f64 / span * 100.0).clamp(0.0, 100.0)
    };

    let axis = {
        let mut a = String::from("<div class=\"axis\">");
        for h in (0..24).step_by(3) {
            a.push_str(&format!("<span>{h:02}:00</span>"));
        }
        a.push_str("</div>");
        a
    };

    let many = report.hosts.len() > 1;
    let mut empty = 0usize;
    // Grouped by month, so a row needs only its day and host rather than a
    // full date — which did not fit the gutter and wrapped.
    let mut month: Option<(i16, i8)> = None;
    for day in days_in(report) {
        let opens = day.to_zoned(tz.clone())?;
        let closes = opens.checked_add(JiffSpan::new().days(1))?;
        let busy = |h: Option<&str>| {
            report
                .spans
                .iter()
                .any(|s| s.end > opens && s.start < closes && h.is_none_or(|h| s.host == h))
        };
        // A row with nothing on it says nothing the calendar does not already
        // say, and twenty of them bury the days that matter. The count below
        // keeps the fact that those days were covered.
        if !busy(None) {
            empty += 1;
            continue;
        }
        let this = (day.year(), day.month());
        if month != Some(this) {
            o.push_str(&format!(
                "<p class=\"tl-month\">{}</p>{axis}",
                day.strftime("%B %Y")
            ));
            month = Some(this);
        }

        for host in &report.hosts {
            if many && !busy(Some(host)) {
                continue;
            }
            // Day and weekday only: the month is the heading above, and a
            // full date did not fit the gutter.
            let label = format!(
                "<b>{:02}</b> <em>{}</em>{}",
                day.day(),
                day.strftime("%a"),
                if many {
                    format!("<i>{}</i>", esc(host))
                } else {
                    String::new()
                }
            );
            o.push_str(&format!(
                "<div class=\"row\"><div class=\"day\">{label}</div><div class=\"track\">"
            ));

            // Quiet bands sit behind the data as a recessive annotation, not a
            // series: they are context, not a measured quantity.
            for (qs, qe) in quiet.intervals(day, &tz).unwrap_or_default() {
                let (a, b) = (pct(&qs, day), pct(&qe, day));
                o.push_str(&format!(
                    "<div class=\"band\" style=\"left:{a:.3}%;width:{:.3}%\"></div>",
                    (b - a).max(0.0)
                ));
            }
            // Only the labelled hours, and recessive: a line every hour reads
            // as a ladder and competes with the marks.
            for h in (3..24).step_by(3) {
                o.push_str(&format!(
                    "<div class=\"hour\" style=\"left:{:.3}%\"></div>",
                    h as f64 * 100.0 / 24.0
                ));
            }

            let day_start = day.to_zoned(tz.clone())?;
            let day_end = day_start.checked_add(JiffSpan::new().days(1))?;
            for span in &report.spans {
                if span.end <= day_start || span.start >= day_end || (many && &span.host != host) {
                    continue;
                }
                let a = pct(&span.start.clone().max(day_start.clone()), day);
                let b = pct(&span.end.clone().min(day_end.clone()), day);
                let width = (b - a).max(0.25);
                // Direct-label only where the bar can actually hold the text;
                // the table below carries every row regardless.
                let label = if width > 14.0 {
                    format!("<span>{}</span>", esc(&span.label()))
                } else {
                    String::new()
                };
                let from = span.start.clone().max(day_start.clone());
                let to = span.end.clone().min(day_end.clone());
                let (profile, gains) = gain_profile(report, &span.host, &from, &to, ceiling);
                let slot = players.iter().position(|p| *p == span.player).unwrap_or(0) % 3 + 1;
                o.push_str(&format!(
                    "<div class=\"bar\" data-host=\"{host}\" data-player=\"{player}\" \
                     data-device=\"{device}\" data-date=\"{date}\" \
                     style=\"left:{a:.3}%;width:{width:.3}%;\
                     --c:var(--series-{slot});--cs:var(--series-{slot}-soft)\" \
                     data-g=\"{gains}\" data-s0=\"{s0}\" data-s1=\"{s1}\" \
                     title=\"{title}\">{profile}{label}</div>",
                    host = esc(&span.host),
                    player = esc(&span.player),
                    device = esc(span
                        .sink_label
                        .as_deref()
                        .or(span.sink.as_deref())
                        .unwrap_or("")),
                    date = span.start.date(),
                    s0 = from.timestamp().as_second(),
                    s1 = to.timestamp().as_second(),
                    title = esc(&format!(
                        "{} · {} · {}–{}",
                        span.label(),
                        span.player,
                        hhmmss(&span.start),
                        hhmmss(&span.end)
                    )),
                ));
            }
            o.push_str("</div></div>\n");
        }
    }
    o.push_str("</div>\n");
    if empty > 0 {
        o.push_str(&format!(
            "<p class=\"muted\">{empty} day{} with no playback {} not shown; every day \
             in the window appears in the calendar above.</p>\n",
            if empty == 1 { "" } else { "s" },
            if empty == 1 { "is" } else { "are" },
        ));
    }
    Ok(o)
}

/// The gain profile drawn inside one bar, plus the same values for hover.
///
/// Five-minute buckets: finer than the eye can resolve at day scale, coarse
/// enough that a long span stays a readable shape rather than noise.
fn gain_profile(
    report: &Report,
    host: &str,
    from: &Zoned,
    to: &Zoned,
    ceiling: f64,
) -> (String, String) {
    let total = secs_between(from, to).max(1);
    let buckets = ((total as f64 / 300.0).ceil() as usize).clamp(1, 288);
    let mut peaks = vec![f64::NAN; buckets];

    for (whose, ts, v) in &report.levels {
        // A bar shows one host's playback, so only that host's levels.
        if whose != host || ts < from || ts > to {
            continue;
        }
        // Integer arithmetic: in floating point 4500/7800 * 26 comes out as
        // 14.999999999999998, dropping that sample a bucket early and
        // leaving the next one empty — a visible hole in the profile.
        let i = (secs_between(from, ts) as i128 * buckets as i128 / total as i128) as usize;
        let i = i.min(buckets - 1);
        peaks[i] = if peaks[i].is_nan() {
            *v
        } else {
            peaks[i].max(*v)
        };
    }

    let w = 100.0 / buckets as f64;
    let mut marks = String::new();
    for (i, v) in peaks.iter().enumerate() {
        if v.is_nan() {
            continue;
        }
        marks.push_str(&format!(
            "<i style=\"left:{:.3}%;width:{w:.3}%;height:{:.2}%\"></i>",
            i as f64 * w,
            (v / ceiling * 100.0).clamp(5.0, 100.0)
        ));
    }

    let gains = peaks
        .iter()
        .map(|v| {
            if v.is_nan() {
                String::new()
            } else {
                format!("{v:.3}")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    (marks, gains)
}

/// A month grid of days, shaded by how loud each was.
///
/// Shown for a window of more than a week, *in addition to* the timeline and
/// never instead of it: a report that quietly stops showing data once there
/// is a lot of it is the same failure as one that omits a host. The calendar
/// is a way to narrow, and the reader decides whether to.
fn calendar(report: &Report, quiet: &QuietHours, scale: GainScale) -> Result<String> {
    let days = crate::query::days(report, quiet)?;
    if days.len() <= 7 {
        return Ok(String::new());
    }

    let GainScale(lo, hi) = scale;
    let step = |v: Option<f64>| -> Option<usize> {
        let v = v?;
        let t = ((v - lo) / (hi - lo).max(f64::EPSILON)).clamp(0.0, 1.0);
        Some(((t * (RAMP_LIGHT.len() - 1) as f64).round() as usize).min(RAMP_LIGHT.len() - 1))
    };

    let mut o = String::from("<h2>Calendar</h2>\n");
    o.push_str(&format!(
        "<div class=\"legend\"><span class=\"key\">loudest level held for 5 minutes</span>\
         <span class=\"key\"><span class=\"ramp\"></span>{lo:.2} to {hi:.2} linear</span>\
         <span class=\"key\"><span class=\"swatch cal-dot-key\"></span>some of it in quiet hours</span></div>\n"
    ));

    // One table per month, so a window of a year reads as twelve calendars
    // rather than one unbroken run of weeks with no boundaries in it.
    // Tables, not CSS grid: the PDF renderer lays tables out correctly and
    // silently spills grid cells across columns.
    o.push_str(&format!(
        "<div class=\"months\" data-lo=\"{lo}\" data-hi=\"{hi}\" data-steps=\"{}\">",
        RAMP_LIGHT.len()
    ));
    let mut month: Option<(i16, i8)> = None;
    let mut column = 0usize;

    for day in &days {
        let this = (day.date.year(), day.date.month());
        if month != Some(this) {
            if month.is_some() {
                for _ in column..7 {
                    o.push_str("<td></td>");
                }
                o.push_str("</tr></tbody></table>");
            }
            o.push_str(&format!(
                "<table class=\"cal\"><caption>{}</caption><thead><tr>",
                day.date.strftime("%B %Y")
            ));
            for name in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
                o.push_str(&format!("<th>{name}</th>"));
            }
            o.push_str("</tr></thead><tbody><tr>");
            // Pad to the weekday the month opens on.
            column = day.date.weekday().to_monday_zero_offset() as usize;
            for _ in 0..column {
                o.push_str("<td></td>");
            }
            month = Some(this);
        }
        if column == 7 {
            o.push_str("</tr><tr>");
            column = 0;
        }

        let tone = step(day.sustained);
        let (bg, ink) = match tone {
            Some(i) => (format!("var(--ramp-{i})"), format!("var(--ramp-ink-{i})")),
            None => ("transparent".to_owned(), "var(--ink-3)".to_owned()),
        };
        let dot = if day.quiet_secs > 0 {
            "<span class=\"cal-dot\"></span>"
        } else {
            ""
        };
        let title = match day.sustained {
            Some(v) => format!(
                "{} · 5-min sustained {v:.3}, peak {:.3} · {} playing, {} in quiet hours",
                day.date,
                day.peak.unwrap_or(v),
                human_secs(day.playing_secs),
                human_secs(day.quiet_secs)
            ),
            None => format!("{} · nothing played", day.date),
        };
        o.push_str(&format!(
            "<td class=\"cal-day\" data-date=\"{}\" title=\"{}\" role=\"button\" \
             tabindex=\"0\" style=\"background:{bg};color:{ink}\">{}{dot}</td>",
            day.date,
            esc(&title),
            day.date.day()
        ));
        column += 1;
    }

    for _ in column..7 {
        o.push_str("<td></td>");
    }
    o.push_str("</tr></tbody></table></div>\n");
    Ok(o)
}

/// Multi-select filters over the dimensions a report actually has.
///
/// Inert in the PDF, which is right: these are for *finding* a window, while
/// the command line produces the document someone else reads.
fn filter_bar(report: &Report) -> String {
    let values = |pick: fn(&Span) -> String| -> Vec<String> {
        let mut v: Vec<String> = report
            .spans
            .iter()
            .map(pick)
            .filter(|s| !s.is_empty())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let groups = [
        ("host", values(|s| s.host.clone())),
        ("player", values(|s| s.player.clone())),
        (
            "device",
            values(|s| {
                s.sink_label
                    .clone()
                    .or_else(|| s.sink.clone())
                    .unwrap_or_default()
            }),
        ),
    ];

    // A dimension with one value filters nothing and is only clutter.
    let useful: Vec<_> = groups.iter().filter(|(_, v)| v.len() > 1).collect();
    if useful.is_empty() {
        return String::new();
    }

    let mut o = String::from("<div class=\"filters\" id=\"filters\">");
    for (kind, vals) in useful {
        o.push_str(&format!(
            "<details class=\"pick\" title=\"Click a name to show only it, or everything \
             again. Shift-click, or use the box, to pick several.\">\
             <summary>{kind}<span class=\"count\"></span></summary><div>"
        ));
        for v in vals {
            o.push_str(&format!(
                "<label><input type=\"checkbox\" data-kind=\"{kind}\" value=\"{0}\" checked> {0}</label>",
                esc(v)
            ));
        }
        o.push_str("</div></details>");
    }
    o.push_str(
        "<button id=\"clear\" hidden>clear filters</button>\
         <span id=\"shown\" class=\"muted\"></span></div>\n",
    );
    o
}

fn spans_table(report: &Report, quiet: &QuietHours) -> String {
    let wide = multiday(report);
    let mut o = String::from("<h2>Playback</h2>\n");
    if report.spans.is_empty() {
        return o + "<p><b>Nothing played in this window.</b></p>\n";
    }
    let hostcol = report.hosts.len() > 1;
    o.push_str("<div class=\"scroll\"><table><thead><tr>");
    if hostcol {
        o.push_str("<th>Host</th>");
    }
    o.push_str("<th>Period</th><th>Duration</th><th>Player</th><th>Media</th><th>Output</th><th>Gain</th><th>Quiet hours</th></tr></thead><tbody>\n");
    for s in &report.spans {
        let q = quiet_seconds(s, quiet);
        // The end repeats its date only when the span crosses midnight.
        let end = if s.start.date() == s.end.date() {
            hhmmss(&s.end)
        } else {
            clock(&s.end, wide)
        };
        // Linkify only http(s): a file:// link to someone else's machine
        // does nothing, while the path itself still identifies the file.
        let source = match s.url.as_deref() {
            Some(u) if u.starts_with("http://") || u.starts_with("https://") => {
                format!("<a class=\"src\" href=\"{0}\">{0}</a>", esc(u))
            }
            Some(u) if !u.is_empty() => format!("<span class=\"src\">{}</span>", esc(u)),
            _ => String::new(),
        };
        let media = format!("{}{source}", esc(&s.label()));
        let gain = s
            .levels
            .map(|l| {
                format!(
                    "<span>{:.3}<u>max</u></span><span>{:.3}<u>med</u></span><span>{:.3}<u>min</u></span>",
                    l.max, l.median, l.min
                )
            })
            .unwrap_or_else(|| "—".into());
        o.push_str(&format!(
            "<tr data-host=\"{}\" data-player=\"{}\" data-device=\"{}\" data-date=\"{}\" \
             data-sustained=\"{}\" data-quiet=\"{}\">",
            esc(&s.host),
            esc(&s.player),
            esc(s.sink_label.as_deref().or(s.sink.as_deref()).unwrap_or("")),
            s.start.date(),
            s.sustained.map(|v| format!("{v:.4}")).unwrap_or_default(),
            q,
        ));
        if hostcol {
            o.push_str(&format!("<td class=\"muted\">{}</td>", esc(&s.host)));
        }
        o.push_str(&format!(
            "<td class=\"num\">{}<span class=\"sub2\">– {end}</span></td>\
             <td class=\"num\">{}</td><td>{}</td><td>{}</td><td class=\"muted\">{}</td>\
             <td class=\"num stack\">{gain}</td><td class=\"num {}\">{}</td></tr>\n",
            clock(&s.start, wide),
            duration(s),
            esc(&s.player),
            media,
            esc(s.sink_label.as_deref().or(s.sink.as_deref()).unwrap_or("")),
            if q > 0 { "flag" } else { "" },
            if q > 0 {
                human_secs(q)
            } else {
                "—".to_owned()
            },
        ));
    }
    o.push_str("</tbody></table></div>\n");

    let total: i64 = report.spans.iter().map(|s| quiet_seconds(s, quiet)).sum();
    if !quiet.is_empty() {
        o.push_str(&format!(
            "<p>Playback inside quiet hours: <b class=\"{}\">{}</b></p>\n",
            if total > 0 { "flag" } else { "" },
            if total == 0 {
                "none".to_owned()
            } else {
                human_secs(total)
            }
        ));
    }
    o
}

fn footer_html(integrity: &Integrity, report: &Report, quiet: &QuietHours) -> String {
    let mut o = String::from("<footer>\n<p><b>Integrity.</b> ");
    o.push_str(&format!(
        "Hash chain <b>{}</b> over {} lines. ",
        if integrity.ok() { "intact" } else { "BROKEN" },
        integrity.lines()
    ));
    o.push_str("Each line hashes the literal bytes of the one before it, continuing across day boundaries. Closed days are timestamped by two independent authorities.</p>\n");
    for p in &integrity.problems() {
        o.push_str(&format!("<p class=\"flag\">{}</p>\n", esc(p)));
    }
    o.push_str("<table><tbody>\n");
    for (host, span, state) in anchor_runs(integrity) {
        o.push_str(&format!(
            "<tr><td class=\"num\">{}</td><td class=\"muted\">{}</td><td>{}</td></tr>\n",
            esc(&span),
            esc(&host),
            esc(&state)
        ));
    }
    o.push_str("</tbody></table>\n");
    let stamp = |z: &Zoned| z.strftime("%Y-%m-%d %H:%M:%S %:z").to_string();
    o.push_str(&format!(
        "<p class=\"muted\">Generated {} · window {} to {} · quiet hours {}. Volumes are linear sink gain, not the cubic figure a slider displays.</p>\n",
        stamp(&Zoned::now()),
        stamp(&report.window.from),
        stamp(&report.window.to),
        if quiet.is_empty() { "none".to_owned() } else { esc(&quiet.describe().join(", ")) },
    ));
    o.push_str("</footer>\n");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// csv and xlsx are the same table in two encodings. They drifted once --
    /// the xlsx kept a "Peak volume" header after the data beneath it had
    /// become min/median/max -- so the shape is pinned here.
    #[test]
    fn both_exports_describe_the_same_columns() {
        assert_eq!(
            COLUMNS.len(),
            14,
            "a column was added or removed; update both exports and this count"
        );
        for column in COLUMNS {
            assert!(!column.key.is_empty() && !column.title.is_empty());
            assert!(
                !column.key.contains(' '),
                "csv headers are machine names: {}",
                column.key
            );
        }
    }

    /// Bucketing a sample must not depend on floating point: 4500/7800 * 26
    /// evaluates to 14.999999999999998, which dropped that sample a bucket
    /// early and left the next one empty — a visible hole in the profile.
    #[test]
    fn bucket_indices_are_exact() {
        let (total, buckets) = (7800i128, 26i128);
        for step in 0..=26i128 {
            let offset = step * 300;
            let exact = (offset * buckets / total).min(buckets - 1) as usize;
            let floating = (((offset as f64 / total as f64) * buckets as f64) as usize).min(25);
            if step < 26 {
                assert_eq!(
                    exact, step as usize,
                    "offset {offset}s belongs in bucket {step}"
                );
            }
            // The float path is what was wrong; this records where.
            if exact != floating {
                assert_eq!(offset, 4500, "only the known case may differ");
            }
        }
    }

    #[test]
    fn gain_columns_are_present_and_ordered() {
        let keys: Vec<&str> = COLUMNS.iter().map(|c| c.key).collect();
        let at = |k: &str| keys.iter().position(|x| *x == k).expect(k);
        assert!(at("gain_min") < at("gain_median"));
        assert!(at("gain_median") < at("gain_max"));
        assert!(at("gain_max") < at("gain_samples"));
    }

    #[test]
    fn every_row_matches_the_column_count() {
        let report = Report {
            hosts: vec!["h".to_owned()],
            levels: Vec::new(),
            window: crate::query::parse_when("2026-10-20", &Zoned::now()).unwrap(),
            at_start: Vec::new(),
            records: Vec::new(),
            spans: Vec::new(),
            unreadable: 0,
        };
        // An empty report still has to agree about its shape.
        for row in rows(&report, &QuietHours::default()) {
            assert_eq!(row.len(), COLUMNS.len());
        }
        assert_eq!(
            csv(
                &report,
                &Integrity {
                    chains: Vec::new(),
                    anchors: Vec::new()
                },
                &QuietHours::default()
            )
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .split(',')
            .count(),
            COLUMNS.len()
        );
    }
}
