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

/// The gain a slider shows, from the gain PipeWire reports.
///
/// `channelVolumes` is linear; every UI that displays it is cubic -- the
/// desktop's slider, `wpctl`, and a Bluetooth speaker's own front panel.
/// They agree only at unity, so a figure of 0.008 is a fifth of the way up
/// the slider, not a hundredth. Reports are read against those UIs, so they
/// speak that scale; the log keeps the linear float it was given, and says
/// which scale it is in.
fn cubic(linear: f64) -> f64 {
    linear.max(0.0).cbrt()
}

/// How wide a boundary mark is drawn, as a stylesheet fragment.
///
/// One dot of the display by default, not one CSS pixel: on a 2x screen a
/// 1px line is two dots wide, which is a rule drawn between two tracks
/// rather than the hairline parting them -- and the marks either side are
/// only a few dots themselves. CSS has no term for the device ratio, so
/// the ladder is explicit; each step is the width that comes to one dot at
/// that ratio, and print, which matches none of them, keeps the plain
/// pixel the base rule gives it.
///
/// `MEDIA_LOGGER_TICK_WIDTH=device` asks for that instead. Read side by
/// side on a 2x screen, the plain pixel won: one dot is so fine that at
/// the opacity the mark is drawn at there is nearly nothing left of it,
/// and what the eye wants here is a division it can see rather than the
/// thinnest line the display can make. A knob for looking rather than
/// configuration, like `MEDIA_LOGGER_TICK_SPACING`, and nothing sets it.
fn tick_ladder() -> &'static str {
    match std::env::var("MEDIA_LOGGER_TICK_WIDTH").as_deref() {
        Ok("device") => {
            "\
/* One dot of the display, not one CSS pixel. On a 2x screen a 1px line is
   two dots wide, which is a rule drawn between two tracks rather than the
   hairline parting them -- and the marks either side are only a few dots
   themselves. The ladder is explicit because CSS has no term for the
   device ratio; each step is one dot at that ratio, and print, matching
   none of them, keeps the plain pixel above. */
@media (min-resolution: 1.5dppx) { .run > i { width: 0.667px; } }
@media (min-resolution: 2dppx) { .run > i { width: 0.5px; } }
@media (min-resolution: 3dppx) { .run > i { width: 0.334px; } }
@media (min-resolution: 4dppx) { .run > i { width: 0.25px; } }"
        }
        _ => "",
    }
}

/// How solid a boundary mark is drawn, on the page and on paper.
///
/// The gain fill runs along the bottom of a block, and an opaque mark
/// chops it into dashes; on an evening of short tracks there is more dash
/// left than curve to follow. Below one the fill reads as a single line
/// through the divisions while they still part the tint above.
///
/// The two media want different figures, which reading them side by side
/// is how anyone would find out: paper takes the marks darker than a
/// screen does, so the same opacity that parts a block on screen closes
/// over the fill in print. Screen 0.6, paper 0.4.
///
/// `MEDIA_LOGGER_TICK_OPACITY` overrides both at once -- a blunt
/// instrument, for trying a figure rather than for setting the pair, which
/// is settled. Another knob for looking, and nothing sets it.
fn tick_opacity() -> (f64, f64) {
    match std::env::var("MEDIA_LOGGER_TICK_OPACITY")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
    {
        Some(v) => (v, v),
        None => (0.60, 0.40),
    }
}

/// A date as a reader parses it, rather than as a machine sorts one.
///
/// Day first: between two rows of the same month the day is the field that
/// differs, and it should be the one the eye lands on. ISO stays wherever
/// the order is taken from the text itself -- the `data-` attributes the
/// filters group by, the two exports, the JSON -- because there the leading
/// year is the sort.
fn human_date(d: jiff::civil::Date) -> String {
    format!("{:02}-{:02}-{:04}", d.day(), d.month(), d.year())
}

/// The same, for a date already written ISO.
///
/// Anything that is not one passes through: the integrity table's days come
/// from the names of files in the tree, and a name that is not a date is a
/// fact about the tree worth showing exactly as it is.
fn human_iso(s: &str) -> String {
    match *s.split('-').collect::<Vec<_>>() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => format!("{d}-{m}-{y}"),
        _ => s.to_owned(),
    }
}

/// A volume as a reader sees it on a slider: a percentage of the way up.
///
/// One decimal, which is what the live checks in the design notes are
/// written to and finer than any slider's own steps, so two levels a
/// listener set apart do not print the same.
fn pct(cubic: f64) -> String {
    format!("{:.1}%", cubic * 100.0)
}

/// Every gain in a report, converted once, before anything renders it.
///
/// At the boundary rather than at each place a figure is printed: there are
/// a dozen of those -- two tables, two exports, a tooltip, a profile, a
/// calendar -- and one of them silently left in the other scale is the
/// eighteen-decibel mistake this conversion exists to stop.
///
/// In place rather than on a copy, because a report carries every record it
/// was built from and this would copy the lot to change three numbers in
/// each span. It runs once, at the top of `render`.
///
/// Taking min, median and max across is sound because the cube root is
/// monotonic: the cube root of the median is the median of the cube roots.
fn to_cubic(report: &mut Report) {
    for (_, _, v) in &mut report.levels {
        *v = cubic(*v);
    }
    for span in &mut report.spans {
        if let Some(l) = &mut span.levels {
            l.min = cubic(l.min);
            l.median = cubic(l.median);
            l.max = cubic(l.max);
        }
        span.sustained = span.sustained.map(cubic);
    }
}

/// How the calendar maps gain to colour.
///
/// Fixed bounds are a guess rather than a fact: until the Bluetooth machine
/// is measured, nobody knows what ordinary and loud actually read as there.
/// So the default takes them from the days on screen, which needs no such
/// knowledge -- the quietest day shown anchors one end and the loudest the
/// other, and the ramp spends its whole range on the differences that exist
/// rather than on a guessed interval the days may all sit inside.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum GainScale {
    /// Derived from the days being coloured, after filtering.
    #[default]
    Auto,
    Fixed(f64, f64),
}

impl GainScale {
    /// The narrowest span `Auto` will stretch across the whole ramp.
    ///
    /// Without a floor, a month whose days all sat within a hundredth of
    /// each other would be drawn as the full light-to-dark sweep, reading
    /// as a dramatic difference that is not there. Below this the range is
    /// widened around its middle instead, and near-identical days come out
    /// the near-identical shades they are.
    pub const FLOOR: f64 = 0.1;

    /// The bounds to colour with, given the values actually being coloured.
    pub fn resolve(self, values: impl IntoIterator<Item = f64>) -> (f64, f64) {
        let (mut lo, mut hi) = match self {
            GainScale::Fixed(lo, hi) => return (lo, hi),
            GainScale::Auto => (f64::INFINITY, f64::NEG_INFINITY),
        };
        for v in values {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        if !lo.is_finite() || !hi.is_finite() {
            // Nothing played. The bounds colour nothing, but they are
            // printed in the legend, so they still have to read sanely.
            return (0.0, Self::FLOOR);
        }
        if hi - lo >= Self::FLOOR {
            (lo, hi)
        } else {
            let lo = ((lo + hi) / 2.0 - Self::FLOOR / 2.0).max(0.0);
            (lo, lo + Self::FLOOR)
        }
    }
}

impl std::str::FromStr for GainScale {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        if s.trim().eq_ignore_ascii_case("auto") {
            return Ok(GainScale::Auto);
        }
        let (lo, hi) = s.split_once(':').context(
            "gain scale is auto, or LOW:HIGH on the slider's scale, for example 0.2:0.6",
        )?;
        let (lo, hi): (f64, f64) = (lo.trim().parse()?, hi.trim().parse()?);
        // NaN compares false either way, so test the ordering we require
        // rather than negating the one we do not.
        if !matches!(lo.partial_cmp(&hi), Some(std::cmp::Ordering::Less)) {
            bail!("gain scale needs HIGH above LOW: {s}");
        }
        Ok(GainScale::Fixed(lo, hi))
    }
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

/// Whole seconds between two instants.
///
/// `Span::get_seconds` returns only the seconds *component* of a calendar
/// span, so a 40 minute stretch reads as zero. Totalling in seconds is what
/// is actually meant.
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

pub fn secs_between(a: &Zoned, b: &Zoned) -> i64 {
    (b - a).total(jiff::Unit::Second).unwrap_or(0.0).round() as i64
}

fn duration(span: &Span) -> String {
    human_secs(secs_between(&span.start, &span.end).max(0))
}

fn human_secs(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    // Past a couple of days hours stop being a quantity anyone can picture:
    // a window of "912h00m" is five weeks, and reads as a typo.
    if h >= 48 {
        return format!("{}d{:02}h", h / 24, h % 24);
    }
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
        z.strftime("%d-%m %H:%M:%S").to_string()
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
            human_iso(&first)
        } else {
            format!("{} … {}  ({n} days)", human_iso(&first), human_iso(&last))
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

/// What the report is able to speak for.
///
/// A log proves absence only while the daemon is running. Every format says
/// this somewhere, because a reader who takes a blank month for a quiet one
/// has been misled by the document, not by the data.
struct Covered {
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
fn record_note(report: &Report, quiet: &QuietHours) -> Result<Option<String>> {
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

fn covered(days: &[crate::query::DaySummary]) -> Covered {
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

    o.push_str(&footer_text(integrity, report, quiet)?);
    Ok(o)
}

/// Greedy wrap. The terminal report is the one read in a narrow window.
fn wrapped(s: &str, width: usize) -> Vec<String> {
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

fn footer_text(integrity: &Integrity, report: &Report, quiet: &QuietHours) -> Result<String> {
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
                        "host": s.host, "gain_cubic": s.levels.map(|l| serde_json::json!({
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
        "coverage": report.coverage.iter().map(|c| serde_json::json!({
            "host": c.host,
            "recorded_s": c.secs(&report.window.from, &report.window.to),
            "up": c.up.iter().map(|(a, b)| serde_json::json!({
                "from": rfc3339(a), "to": rfc3339(b),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
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
        key: "volume_min",
        title: "Volume min",
        width: 10.0,
    },
    Column {
        key: "volume_median",
        title: "Volume median",
        width: 12.0,
    },
    Column {
        key: "volume_max",
        title: "Volume max",
        width: 10.0,
    },
    Column {
        key: "volume_samples",
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
    if let Some(line) = record_note(report, quiet)? {
        o.push_str(&format!("\n# {line}\n"));
    }
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
    if let Some(line) = record_note(report, quiet)? {
        put("Record", line)?;
    }
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
    return ['host', 'player', 'device', 'during'].every(function (k) {
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
    steps: parseInt(months.dataset.steps, 10),
    auto: months.dataset.auto === '1',
    floor: parseFloat(months.dataset.floor)
  };
  var range = document.querySelector('.ramp-range');
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
    // An automatic scale is taken from what survived, not from what the
    // report was built with: filtering down to one quiet host should spend
    // the ramp on that host's days rather than leave them all at one end.
    if (scale.auto) {
      var lo = Infinity, hi = -Infinity;
      for (var k in best) { lo = Math.min(lo, best[k]); hi = Math.max(hi, best[k]); }
      if (isFinite(lo)) {
        if (hi - lo < scale.floor) {
          lo = Math.max(0, (lo + hi) / 2 - scale.floor / 2);
          hi = lo + scale.floor;
        }
        scale.lo = lo;
        scale.hi = hi;
        if (range) range.textContent =
          (lo * 100).toFixed(1) + '% to ' + (hi * 100).toFixed(1) + '%';
      }
    }
    days.forEach(function (cell) {
      var d = cell.dataset.date;
      var v = best[d];
      var dot = cell.querySelector('.cal-dot');
      if (v === undefined) {
        // Cleared, not painted: a cell marked as unrecorded carries its
        // hatch in CSS, and an inline background would cover it.
        cell.style.background = '';
        cell.style.color = '';
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
    // A block whose songs were all filtered out would otherwise outline
    // empty track. One that keeps some of them keeps its full extent, and
    // the hole inside it reads as what it is.
    document.querySelectorAll('.run').forEach(function (run) {
      run.hidden = !run.querySelector('.bar:not([hidden])');
    });
    // A timeline row with nothing left in it is noise, not evidence.
    document.querySelectorAll('.row').forEach(function (row) {
      var any = row.querySelector('.bar:not([hidden])');
      if (row.querySelector('.bar')) row.hidden = !any;
    });
    // A day's heading in the table stands over its own rows; with all of
    // them filtered out it stands over the next day's.
    document.querySelectorAll('tr.daybreak').forEach(function (head) {
      var any = false;
      for (var el = head.nextElementSibling; el; el = el.nextElementSibling) {
        if (el.classList.contains('daybreak')) break;
        if (!el.hidden) { any = true; break; }
      }
      head.hidden = !any;
    });
    // A day is drawn as one block and its first row carries the date, so a
    // filter that empties either end has to hand both to the rows left.
    var opened = {}, closes = {};
    document.querySelectorAll('.row[data-day]').forEach(function (row) {
      var when = row.querySelector('.when');
      var day = row.dataset.day;
      row.classList.remove('dayfirst', 'daylast');
      if (row.hidden) { if (when) when.classList.add('ditto'); return; }
      if (opened[day]) { if (when) when.classList.add('ditto'); }
      else {
        row.classList.add('dayfirst');
        if (when) when.classList.remove('ditto');
        opened[day] = true;
      }
      closes[day] = row;
    });
    Object.keys(closes).forEach(function (day) {
      closes[day].classList.add('daylast');
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
      // The colon belongs to the dimension's name, so it is written apart
      // from the value and stays out of the weight the value takes.
      var count = group.querySelector('.count');
      count.textContent = ': ';
      var val = document.createElement('span');
      val.className = 'val';
      val.textContent = label;
      count.appendChild(val);
      // A dimension standing at all or at none is not choosing between its
      // values; one that has picked some of them is, and that is the state
      // worth finding at a glance across a row of four.
      // Anything but "all" is a choice the reader made, none included:
      // showing nothing is as much a filter as showing two of three.
      var custom = on.length < all.length;
      group.classList.toggle('narrowed', custom);
      // And in the list itself, so a dropdown says what is picked without
      // the reader having to read the boxes -- except where everything is
      // picked, since a list with every line in bold says no more than a
      // list with none.
      all.forEach(function (x) {
        if (x.parentNode) x.parentNode.classList.toggle('on', x.checked && custom);
      });
    });
    recolour();
    var filtered = dates.length || boxes.some(function (b) { return !b.checked; });
    clear.hidden = !filtered;
    shown.textContent = filtered ? visible + ' of ' + rows.length + ' shown' : '';
  }

  boxes.forEach(function (b) { b.addEventListener('change', apply); });

  // Shift-click is a selection gesture here, not a text one; without this
  // the browser extends its text selection and drags the labels into it.
  function noTextDrag(e) {
    if (e.shiftKey) e.preventDefault();
  }

  // Clicking a name means "show only this", and again means "show
  // everything" — the question being asked most of the time. The checkbox
  // and shift-click stay available for building a set by hand.
  document.querySelectorAll('.pick').forEach(function (group) {
    var all = Array.prototype.slice.call(group.querySelectorAll('input'));
    group.querySelectorAll('label').forEach(function (lab) {
      lab.addEventListener('mousedown', noTextDrag);
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
    d.addEventListener('mousedown', noTextDrag);
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
    var gain = g[i] ? ' \u00b7 volume ' + g[i] : '';
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
  --rule: #e3e2dd; --quiet: #ebe9e3; --quiet-ink: #6d6a62; --hatch: #c6c4bc;
  --flag: #b03030; --link: #4a3aa7;
{light_ramp}
{light_series}
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    color-scheme: dark;
    --surface: #1a1a19; --panel: #232322;
    --ink: #ffffff; --ink-2: #c3c2b7; --ink-3: #96958c;
    --rule: #3a3a38; --quiet: #2c2c2a; --quiet-ink: #9a988f; --hatch: #605f59;
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
  --rule: #3a3a38; --quiet: #2c2c2a; --quiet-ink: #9a988f; --hatch: #605f59;
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
/* Row gap smaller than column gap: the two lines of the calendar's legend
   are one thought in two parts, and a full gap between them read as two
   separate blocks. */
.legend {{ display: flex; flex-wrap: wrap; gap: .3rem 1rem; margin: 0 0 1rem;
  color: var(--ink-2); }}
/* What the ramp means is one line and what the marks on a cell mean is the
   next, at any width rather than only where the page is too narrow to hold
   them together. Two rows rather than a break inside one: weasyprint does
   not honour a flex gap here, and a break element took a whole line box,
   so the two halves sat a line apart on paper. Stacked, the space between
   them is a margin, which both renderers agree about. */
.legend.legend-lead {{ margin-bottom: .35rem; }}
.key {{ display: inline-flex; align-items: center; gap: .4rem; white-space: nowrap; }}
/* flex: none, or the renderer shrinks a swatch to a hairline when the
   legend is wider than the page -- which is exactly when it wraps. */
/* inline-block, not left to the flex container to blockify: weasyprint
   kept an inline box for the empty span as well as the flex item, and a
   swatch with a border came out with a stem hanging off it. Stating the
   display makes the width and height apply whatever lays it out. */
.swatch {{ display: inline-block; vertical-align: middle;
  width: 12px; height: 12px; border-radius: 3px; flex: none; }}
.axis {{ display: flex; color: var(--ink-3); font-size: 11px;
  font-variant-numeric: tabular-nums; margin: 0 0 .3rem 4.25rem; }}
.axis span {{ flex: 1 1 0; }}
/* One day is one block. Its rows -- a machine each -- sit flush and
   overlap by the width of a border, so they share a single rule instead of
   each drawing its own box; the block then closes and the next day opens
   clear of it. Two machines are one day, and the drawing says so. */
.row {{ display: flex; align-items: center; gap: .75rem; margin: 0 0 -1px; }}
.row.dayfirst {{ margin-top: .35rem; }}
.row.daylast {{ margin-bottom: .35rem; }}
.day {{ width: 3.5rem; flex: none; color: var(--ink-2); font-size: 12px;
  font-variant-numeric: tabular-nums; line-height: 1.25; text-align: right; }}
.day b {{ font-weight: 600; }}
.day em {{ font-style: normal; color: var(--ink-3); font-weight: 400; }}
.day i {{ display: block; font-style: normal; color: var(--ink-3);
  font-size: 10.5px; }}
/* Hidden rather than absent, so the rows of one day still line up with
   each other and the host names sit on one baseline. */
.day .ditto {{ visibility: hidden; }}
.tl-month {{ font-size: 12px; font-weight: 600; color: var(--ink-2);
  margin: 1.25rem 0 .35rem; }}
.tl-month:first-of-type {{ margin-top: .25rem; }}
/* Rounded on the block's own corners, square where two rows meet. */
.track {{ position: relative; flex: 1 1 auto; height: 34px;
  background: var(--panel); border: 1px solid var(--rule); overflow: hidden; }}
.row.dayfirst .track {{ border-top-left-radius: 4px; border-top-right-radius: 4px; }}
.row.daylast .track {{ border-bottom-left-radius: 4px; border-bottom-right-radius: 4px; }}
.band {{ position: absolute; top: 0; bottom: 0; background: var(--quiet); }}
.hour {{ position: absolute; top: 0; bottom: 0; width: 1px; background: var(--rule);
  opacity: .55; }}
/* Tracks with no readable gap between them are drawn inside one block. At
   day scale a song is about two pixels wide and its halo four, so a run of
   songs spent more of the page on halo than on data and the halos closed
   over each other. The block carries the halo once; the real gaps and the
   hairlines inside it are what keep the songs apart. */
/* Three above and three below, not four and two. The track is 34px with a
   border either side, so what a block is centred in is the 32px between
   them -- a block set 4px down sat 2px low, which no one saw while every
   row was its own rounded box and everyone saw once the rows met. */
.run {{ position: absolute; top: 3px; height: 26px; border-radius: 3px;
  box-shadow: 0 0 0 2px var(--panel); overflow: hidden; }}
.bar {{ position: absolute; top: 0; height: 100%; min-width: 2px;
  overflow: hidden; background: var(--cs); }}
/* The boundary between two tracks, as its own pixel directly on the block.
   Not a border, which would be taken out of a mark already only a few
   pixels wide; and not a shadow, because weasyprint draws no shadows at
   all, so the hairline existed on screen and nowhere else.

   Each segment also runs a pixel past its neighbour's edge. Tracks that
   merely abut leave the boundary on a fractional pixel, where the renderer
   rounds it away at one width and into a visible line at the next -- which
   is how a record came out with a separator of a different width at every
   boundary, some of them missing. Overlapping leaves nothing there to
   round, so the only thing drawn at a boundary is this one pixel.

   Its colour is whatever the cut reveals, chosen per boundary: the track,
   or the quiet band where the block crosses one. A white nick through a
   grey band reads as a mark laid over the block rather than a gap opened
   in it. Drawn from the series, which is what it was first, it fell near
   the gain fill and could be read as a spike in the level.

   Not quite opaque, so the gain fill still reads as one curve. An opaque
   mark chops it into dashes, and on an evening of short tracks there is
   more dash left than curve to follow. */
.run > i {{ position: absolute; top: 0; bottom: 0; width: 1px;
  opacity: {tick_opacity}; pointer-events: none; }}
{tick_ladder}
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
/* The narrowest column has the longest heading, so on paper it was the one
   that wrapped -- two lines of heading over one line of value. Abbreviated
   and held to a line: the window it counts against is named in the
   subtitle, so the heading has only to say which column this is. */
th.nowrap {{ white-space: nowrap; }}
td.num {{ font-variant-numeric: tabular-nums; white-space: nowrap; }}
/* Period and gain each stack their values, which keeps seven columns on a
   printed page where nine overflowed. */
td .sub2 {{ display: block; color: var(--ink-3); }}
/* A day's heading carries the date the rows beneath it no longer need. The
   rule above it is the separator; the one the row would have drawn under
   itself is dropped, so the heading sits with the rows it opens rather
   than between two days. */
tr.daybreak td {{ padding-top: 1.1rem; border-bottom: none;
  color: var(--ink-2); font-weight: 600; }}
tr.daybreak + tr td {{ border-top: 1px solid var(--rule); }}
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
/* The value carries the weight, not the dimension's name: reading the row
   is reading what has been picked, and "player" is there either way. */
.pick.narrowed .val {{ color: var(--ink); font-weight: 600; }}
.pick .count:empty {{ display: none; }}
.pick > div {{ position: absolute; z-index: 5; margin-top: .25rem;
  background: var(--panel); border: 1px solid var(--rule); border-radius: 6px;
  padding: .4rem; min-width: 11rem; max-height: 15rem; overflow-y: auto;
  box-shadow: 0 6px 18px rgb(0 0 0 / .12); }}
.pick .swatch {{ width: 10px; height: 10px; flex: none; }}
.pick label {{ display: flex; gap: .45rem; align-items: center; font-size: 12.5px;
  padding: .18rem .25rem; white-space: nowrap; cursor: pointer;
  -webkit-user-select: none; user-select: none; }}
/* Picked values carry the same weight here as in the summary above, so
   opening a dimension shows the same thing the closed row was saying. */
.pick label.on {{ font-weight: 600; color: var(--ink); }}
/* Not a pill like the ones beside it. Those four are the state of the
   report and stay put; this one is an action and comes and goes with the
   filtering. A button shaped like its neighbours reads as a fifth
   dimension to choose from, which is the one thing it is not. */
#clear {{ font: inherit; font-size: 12px; padding: .25rem .2rem; cursor: pointer;
  border: 0; background: none; color: var(--link);
  text-decoration: underline; text-underline-offset: 2px; }}
#shown {{ font-size: 12px; }}
tr[hidden], .bar[hidden], .run[hidden], .row[hidden],
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
.cal-day {{ position: relative; width: 2.1rem; height: 2.1rem; color: var(--ink-3);
  border: 1px solid var(--rule); border-radius: 4px; font-size: 11px;
  font-variant-numeric: tabular-nums; text-align: center;
  vertical-align: middle; cursor: pointer;
  /* Shift-click is a selection gesture here, not a text one. */
  -webkit-user-select: none; user-select: none; }}
.cal-day[aria-pressed="true"] {{ outline: 2px solid var(--ink); outline-offset: 1px; }}
/* The dot hangs off an inner block, not off the cell: the PDF renderer does
   not make a table cell a containing block, and positioned against the page
   the dot landed beside the number instead of in the corner. */
.cal-in {{ position: relative; display: block; height: 2rem; line-height: 2rem; }}
.cal-dot {{ position: absolute; bottom: 2px; right: 2px; width: 4px; height: 4px;
  border-radius: 50%; background: currentColor; opacity: .55; }}
.cal-dot-key {{ background: var(--ink-3); border-radius: 50%; width: 6px; height: 6px; }}
/* A dashed edge for an incomplete record, hatched when there is none at all.
   Achromatic on purpose: the fill already means loudness, and this is not a
   quantity but a statement about whether there is one. */
/* Hatched where the record is partial: the shading rests on only part of
   the day, and the pattern says so over it. Dashed and unfilled where
   there is no record at all -- there is nothing to shade, and a broken
   edge suits a cell with nothing behind it.

   On the inner block, not on the cell: weasyprint does not clip a
   background image to the border radius, and on the cell the hatch ran
   past the corners and out into the spacing between cells. Inside, a
   square box within the rounded one contains it.

   One stop per position: the PDF renderer rejects the two-position colour
   stop shorthand outright and drops the whole declaration. */
.cal-part .cal-in, .cal-part-key {{ background-image:
  repeating-linear-gradient(45deg,
  var(--hatch) 0, var(--hatch) 1px, transparent 1px, transparent 4px); }}
/* A dashed edge and a flat achromatic fill, together. The edge is the
   statement -- a broken outline for a cell with nothing behind it -- and
   weasyprint draws no dashed border on a cell at all, so on paper the
   fill is what is left to say it. Achromatic either way: this is not a
   quantity, and it must not read as a place on the ramp. */
.cal-void {{ border-style: dashed; border-color: var(--ink-3);
  background-color: var(--quiet); }}
/* Darker than the hatch: on a filled cell a pale dashed edge disappears,
   and a part-recorded day is exactly the one a reader must not misread. */
.cal-void-key {{ border: 1px dashed var(--hatch); }}
.cal-part-key {{ border: 1px solid var(--rule); }}
@media print {{
  .cal-part .cal-in, .cal-part-key, .cal-void, .cal-void-key
    {{ print-color-adjust: exact; }}
  .cal-void-key {{ background-color: var(--quiet); }}
}}
.ramp {{ width: 72px; height: 10px; border-radius: 3px;
  background: linear-gradient(to right, var(--ramp-0), var(--ramp-4)); }}
@media print {{ .cal-day {{ print-color-adjust: exact; }} }}
@media print {{
  body {{ background: #fff; }}
  main {{ max-width: none; padding: 0; }}
  .run {{ box-shadow: 0 0 0 2px #fff; }}
  .bar {{ print-color-adjust: exact; }}
  /* Paper takes the marks darker than a screen does, so the figure that
     parts a block on screen closes over the gain fill here. */
  .run > i {{ opacity: {tick_opacity_print}; print-color-adjust: exact; }}
  .band, .swatch {{ print-color-adjust: exact; }}
  /* Three months to a page-width rather than two. A screen can be scrolled
     and a page cannot, so paper is where the months are worth tightening;
     the cell keeps its proportions and only loses size.

     The width spends the printable column exactly: 643px measured off a
     rendered page -- edge of the leftmost cell to edge of the widest row
     -- less two gaps, over three. Earlier figures were a little under and
     left the third month short of the margin by about a gap's width, which
     read as a row not lining up with the timeline beneath it.

     Stated, not computed. `calc((100% - 2rem) / 3)` is the expression this
     wants and weasyprint does not resolve it here: the width fell back to
     auto, the tables sized to their contents, and two went back to filling
     the row. Every length below is therefore literal, and the cells keep
     their own widths so that nothing can size itself from its contents. */
  .months {{ gap: 0 1rem; }}
  .cal {{ width: 12.7rem; border-spacing: 2px; }}
  .cal td, .cal th {{ width: 1.67rem; }}
  .cal th {{ font-size: 9px; }}
  .cal-day {{ width: 1.67rem; height: 1.67rem; font-size: 9.5px; }}
  .cal-in {{ height: 1.57rem; line-height: 1.57rem; }}
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
        tick_ladder = tick_ladder(),
        tick_opacity = tick_opacity().0,
        tick_opacity_print = tick_opacity().1,
    ));

    // -- opening state -----------------------------------------------------
    if opening_state_matters(report) {
        o.push_str("<h2>State at the start of the window</h2>\n");
        for (host, at) in &report.at_start {
            o.push_str(&format!("<p><b>{}</b><br>", esc(host)));
            let playing = at.playing();
            let recorded = report
                .coverage
                .iter()
                .any(|c| c.host == *host && c.covers(&report.window.from));
            if !recorded {
                // "Nothing playing" is a claim about the room. With the
                // daemon down it is only a claim about the log.
                o.push_str("No record: the logger was not running.");
            } else if playing.is_empty() {
                o.push_str("Nothing playing.");
            } else {
                o.push_str("Playing: ");
                o.push_str(
                    &playing
                        .iter()
                        .map(|p| {
                            esc(&format!(
                                "{} — {}",
                                crate::query::player_name(&p.player),
                                p.track.title.clone().unwrap_or_else(|| "(untitled)".into())
                            ))
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if let Some(s) = &at.sink {
                o.push_str(&format!(
                    "<br>Output: {} ({}) at volume {} <span class=\"muted\">cubic</span>",
                    esc(s.description.as_deref().unwrap_or(&s.name)),
                    esc(s.device_api.as_deref().unwrap_or("?")),
                    s.effective()
                        .iter()
                        .map(|v| pct(cubic(*v)))
                        .collect::<Vec<_>>()
                        .join(" / "),
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
        o.push_str(&filter_bar(report, &players, quiet));
        o.push_str(&calendar(report, quiet, scale)?);
        o.push_str(&timeline(report, quiet, &players)?);
        o.push_str(&spans_table(report, quiet));
    }

    o.push_str(&footer_html(integrity, report, quiet)?);
    if !report.levels.is_empty() {
        o.push_str(HOVER);
    }
    if !report.window.instant {
        o.push_str(FILTERS);
    }
    o.push_str("</main></body></html>\n");
    Ok(o)
}

/// One pixel of a day track, as a share of the day.
///
/// The track is what a 60rem page has left over once the day gutter is
/// taken, so about 880px wide and narrower on a phone. The distances below
/// are drawing distances rather than spans of time, so they are stated in
/// those pixels and converted once, here.
const PX: f64 = 100.0 / 880.0;

/// Narrowest a bar is drawn, matching `min-width` in the stylesheet. A
/// three-minute song is a quarter of this, and would otherwise vanish.
const FLOOR: f64 = 2.0 * PX;

/// Clear track two marks need between them to read as two marks. A bar
/// carries two pixels of halo either side, so anything less and the halos
/// close over each other -- and over the bars, which are no wider.
const APART: f64 = 4.0 * PX;

/// Closest two boundary marks are drawn, in track pixels. None by default.
///
/// A boundary nearer than this to the last one drawn is left out, and the
/// tracks either side of it read as one. It is off because, once the mark
/// became one dot of the display in the rule's own colour, there was
/// nothing left to thin: three pixels was a remedy for a line that was two
/// dots wide and the colour of the data, and both of those are fixed.
///
/// Kept because the remedy may be wanted again on an evening denser than
/// any yet seen, and because reading the same evening at several spacings
/// is how the question gets settled. `MEDIA_LOGGER_TICK_SPACING` sets it,
/// in pixels; it is a knob for looking rather than configuration, and
/// nothing sets it.
fn spacing() -> f64 {
    std::env::var("MEDIA_LOGGER_TICK_SPACING")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0)
        * PX
}

/// One span as the page will draw it, in shares of the day.
struct Mark<'a> {
    span: &'a Span,
    left: f64,
    width: f64,
}

/// Marks with no readable gap between them, drawn inside one block.
struct Block {
    /// Which of the day's marks, as a run of consecutive ones. They arrive
    /// in start order, so a block is always a slice.
    marks: std::ops::Range<usize>,
    left: f64,
    /// Furthest edge any mark in it reaches, which is not always the last
    /// mark's: two players on one host play at once.
    right: f64,
}

/// Groups a day's marks into the blocks the page draws, by where they land.
///
/// Takes geometry alone -- each mark's left edge and width, as shares of the
/// day -- because where a mark lands is the whole of what makes a block, and
/// asking the question should not require inventing a track to ask it about.
fn blocks(marks: &[(f64, f64)]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for (i, (left, width)) in marks.iter().enumerate() {
        match out.last_mut() {
            // A block closes as soon as a mark opens clear of the furthest
            // edge reached. The previous mark's own edge will not do: two
            // players on one host play at once, and the one that started
            // earlier can outlast the later.
            Some(block) if *left < block.right + APART => {
                block.marks.end = i + 1;
                block.right = block.right.max(left + width);
            }
            _ => out.push(Block {
                marks: i..i + 1,
                left: *left,
                right: left + width,
            }),
        }
    }
    out
}

fn timeline(report: &Report, quiet: &QuietHours, players: &[String]) -> Result<String> {
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
            let slot = players.iter().position(|x| x == p).unwrap_or(0) % 3 + 1;
            o.push_str(&format!(
                "<span class=\"key\"><span class=\"swatch\" \
                 style=\"background:var(--series-{slot})\"></span>{}</span>",
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
            format!(" (0–{})", pct(ceiling))
        };
        o.push_str(&format!(
            "<span class=\"key\"><span class=\"swatch gainkey\"></span>volume{scale}</span>"
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

    let spacing = spacing();
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

        // A day's rows are drawn as one block, so each has to know whether
        // it opens or closes that block before any of them is written.
        let shown: Vec<&String> = report
            .hosts
            .iter()
            .filter(|h| !many || busy(Some(h.as_str())))
            .collect();
        for (i, host) in shown.iter().enumerate() {
            let edge = match (i == 0, i + 1 == shown.len()) {
                (true, true) => " dayfirst daylast",
                (true, false) => " dayfirst",
                (false, true) => " daylast",
                _ => "",
            };
            // Day and weekday only: the month is the heading above, and a
            // full date did not fit the gutter.
            //
            // And only on the day's first row. With a row per host, the
            // date ran down the gutter once per machine, so three machines
            // read as three days. The repeats are in the markup and hidden
            // rather than left out: a filter can empty the row carrying the
            // date, and the next one then has to take it up.
            let label = format!(
                "<span class=\"when{}\"><b>{:02}</b> <em>{}</em></span>{}",
                if i == 0 { "" } else { " ditto" },
                day.day(),
                day.strftime("%a"),
                if many {
                    format!("<i>{}</i>", esc(host))
                } else {
                    String::new()
                }
            );
            o.push_str(&format!(
                "<div class=\"row{edge}\" data-day=\"{day}\"><div class=\"day\">{label}</div>\
                 <div class=\"track\">"
            ));

            // Quiet bands sit behind the data as a recessive annotation, not a
            // series: they are context, not a measured quantity.
            //
            // Kept, because a boundary mark is a cut through the block and
            // has to be the colour of whatever the cut reveals -- which over
            // these hours is the band, not the track.
            let mut bands: Vec<(f64, f64)> = Vec::new();
            for (qs, qe) in quiet.intervals(day, &tz).unwrap_or_default() {
                let (a, b) = (pct(&qs, day), pct(&qe, day));
                o.push_str(&format!(
                    "<div class=\"band\" style=\"left:{a:.3}%;width:{:.3}%\"></div>",
                    (b - a).max(0.0)
                ));
                bands.push((a, b));
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
            // What counts as one mark is decided by the drawing, not the
            // clock: a bar is floored at FLOOR wide and clears APART of
            // track either side, so tracks a few minutes apart already
            // overlap on the page. Grouping them is what the page was going
            // to draw anyway, minus the collision.
            let mut marks: Vec<Mark> = Vec::new();
            for span in &report.spans {
                if span.end <= day_start || span.start >= day_end || (many && &span.host != *host) {
                    continue;
                }
                let left = pct(&span.start.clone().max(day_start.clone()), day);
                let right = pct(&span.end.clone().min(day_end.clone()), day);
                marks.push(Mark {
                    span,
                    left,
                    width: (right - left).max(FLOOR),
                });
            }

            let geometry: Vec<(f64, f64)> = marks.iter().map(|m| (m.left, m.width)).collect();
            for block in blocks(&geometry) {
                let (left, full) = (block.left, block.right - block.left);
                let held = &marks[block.marks];
                o.push_str(&format!(
                    "<div class=\"run\" style=\"left:{left:.3}%;width:{full:.3}%\">"
                ));
                let mut cursor = left;
                // Boundaries are drawn after the tracks, so they sit above
                // them whatever order the tracks were painted in.
                let mut cuts = String::new();
                // Measured from the block's own edge, so the first mark does
                // not land against the rounded corner.
                let mut marked = left;
                for (i, mark) in held.iter().enumerate() {
                    let a = mark.left.max(cursor);
                    // Tracks are laid edge to edge, each running to where
                    // the next began. The gaps between them are what put
                    // them in one block: seconds, a fifth of a pixel, and
                    // drawing them anyway put a line of a different width
                    // at every boundary, none of them meaning anything.
                    // The block's own extent is untouched, so what it says
                    // about when playback started and stopped stays true.
                    let b = match held.get(i + 1) {
                        // Unless the two overlap -- two players on one host
                        // play at once, and neither is the other's end.
                        Some(next) if next.left >= mark.left + mark.width => next.left,
                        _ => mark.left + mark.width,
                    };
                    let span = mark.span;
                    let (a, b) = (a, b.max(a + FLOOR));
                    cursor = b;
                    let slot = players.iter().position(|p| *p == span.player).unwrap_or(0) % 3 + 1;
                    // Every boundary inside a block is marked, and marked
                    // the same way. A record of three-minute songs is the
                    // case this is for: the marks come out small, but a
                    // reader counting them is reading the evening right,
                    // where leaving them joined says one long thing played.
                    if i > 0 && a - marked >= spacing && block.right - a >= spacing {
                        let under = if bands.iter().any(|(s, e)| a >= *s && a < *e) {
                            "--quiet"
                        } else {
                            "--panel"
                        };
                        cuts.push_str(&format!(
                            "<i style=\"left:{:.3}%;background:var({under})\"></i>",
                            (a - left) / full * 100.0
                        ));
                        marked = a;
                    }
                    // Direct-label only where the bar can actually hold the
                    // text; the table below carries every row regardless.
                    let label = if b - a > 14.0 {
                        format!("<span>{}</span>", esc(&span.label()))
                    } else {
                        String::new()
                    };
                    let from = span.start.clone().max(day_start.clone());
                    let to = span.end.clone().min(day_end.clone());
                    let (profile, gains) = gain_profile(report, &span.host, &from, &to, ceiling);
                    o.push_str(&format!(
                        "<div class=\"bar\" data-host=\"{host}\" data-player=\"{player}\" \
                         data-device=\"{device}\" data-date=\"{date}\" \
                         data-during=\"{during}\" \
                         style=\"left:{l:.3}%;width:calc({wide:.3}% + 1px);\
                         --c:var(--series-{slot});--cs:var(--series-{slot}-soft)\" \
                         data-g=\"{gains}\" data-s0=\"{s0}\" data-s1=\"{s1}\" \
                         title=\"{title}\">{profile}{label}</div>",
                        l = (a - left) / full * 100.0,
                        wide = (b - a) / full * 100.0,
                        host = esc(&span.host),
                        player = esc(&span.player),
                        device = esc(span
                            .sink_label
                            .as_deref()
                            .or(span.sink.as_deref())
                            .unwrap_or("")),
                        date = span.start.date(),
                        during = when_of(span, quiet),
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
                o.push_str(&cuts);
                o.push_str("</div>");
            }
            o.push_str("</div></div>\n");
        }
    }
    o.push_str("</div>\n");
    if empty > 0 {
        // A day with no record is not a day with no playback, and one count
        // covering both would say the opposite of what happened.
        let blank = crate::query::days(report, quiet)?
            .iter()
            .filter(|d| d.recorded() != Recorded::Full && !busy_on(report, d.date, &tz))
            .count()
            .min(empty);
        let silent = empty - blank;
        let mut said = Vec::new();
        if silent > 0 {
            said.push(format!(
                "{silent} day{} with no playback",
                if silent == 1 { "" } else { "s" }
            ));
        }
        if blank > 0 {
            said.push(format!(
                "{blank} day{} with no record",
                if blank == 1 { "" } else { "s" }
            ));
        }
        o.push_str(&format!(
            "<p class=\"muted\">{} not shown; every day in the window appears in \
             the calendar above.</p>\n",
            said.join(" and ")
        ));
    }
    Ok(o)
}

/// Did anything play on this date, on any host?
fn busy_on(report: &Report, date: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> bool {
    let Ok(opens) = date.to_zoned(tz.clone()) else {
        return false;
    };
    let Ok(closes) = opens.checked_add(jiff::Span::new().days(1)) else {
        return false;
    };
    report
        .spans
        .iter()
        .any(|s| s.end > opens && s.start < closes)
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
        .map(|v| if v.is_nan() { String::new() } else { pct(*v) })
        .collect::<Vec<_>>()
        .join(",");
    (marks, gains)
}

/// A month grid of days, shaded by how loud each was.
///
/// Shown for any window of more than one day, *in addition to* the timeline
/// and never instead of it: a report that quietly stops showing data once
/// there is a lot of it is the same failure as one that omits a host.
///
/// It was a week's worth before, on the grounds that two shaded cells say
/// nothing the two timeline rows beneath them do not. True of the shading
/// and beside the point: the calendar is also the control that narrows the
/// report, and a reader with two days has the same reason to click one of
/// them as a reader with two months.
fn calendar(report: &Report, quiet: &QuietHours, scale: GainScale) -> Result<String> {
    let days = crate::query::days(report, quiet)?;
    if days.len() <= 1 {
        return Ok(String::new());
    }

    let (lo, hi) = scale.resolve(days.iter().filter_map(|d| d.sustained));
    let step = |v: Option<f64>| -> Option<usize> {
        let v = v?;
        let t = ((v - lo) / (hi - lo).max(f64::EPSILON)).clamp(0.0, 1.0);
        Some(((t * (RAMP_LIGHT.len() - 1) as f64).round() as usize).min(RAMP_LIGHT.len() - 1))
    };

    // Say where the numbers come from: under Auto they move when the
    // filters do, and a reader who does not know that reads two views of
    // the same days as a contradiction.
    let from = match scale {
        GainScale::Auto => ", across the days shown",
        GainScale::Fixed(..) => "",
    };

    let voids = days
        .iter()
        .filter(|d| d.recorded() == Recorded::None)
        .count();
    // Partial days only carry the mark where they have no shading, so the
    // count that decides the key has to ask the same question the cell
    // does -- not how many days were partly recorded.
    let parts = days
        .iter()
        .filter(|d| d.recorded() == Recorded::Partial && step(d.sustained).is_none())
        .count();
    // Keys only for what is actually on the page: a legend entry for a mark
    // that never appears is one more thing to decode for nothing.
    let gaps = format!(
        "{}{}",
        if voids > 0 {
            "<span class=\"key\"><span class=\"swatch cal-void-key\"></span>no record</span>"
        } else {
            ""
        },
        if parts > 0 {
            "<span class=\"key\"><span class=\"swatch cal-part-key\"></span>\
             part of the day</span>"
        } else {
            ""
        }
    );

    let mut o = String::from("<h2>Calendar</h2>\n");
    // The legend reads in percent; the data attributes stay the numbers the
    // script recolours from, so the two must not share a formatting.
    let (lo_pct, hi_pct) = (pct(lo), pct(hi));
    o.push_str(&format!(
        "<div class=\"legend legend-lead\"><span class=\"key\">loudest level held for 5 minutes</span>\
         <span class=\"key\"><span class=\"ramp\"></span>\
         <span><span class=\"ramp-range\">{lo_pct} to {hi_pct}</span>{from}</span></span>\
         </div>\
         <div class=\"legend\">\
         <span class=\"key\"><span class=\"swatch cal-dot-key\"></span>some of it in quiet hours</span>{gaps}</div>\n"
    ));

    // One table per month, so a window of a year reads as twelve calendars
    // rather than one unbroken run of weeks with no boundaries in it.
    // Tables, not CSS grid: the PDF renderer lays tables out correctly and
    // silently spills grid cells across columns.
    o.push_str(&format!(
        "<div class=\"months\" data-lo=\"{lo}\" data-hi=\"{hi}\" data-steps=\"{}\" \
         data-auto=\"{}\" data-floor=\"{}\">",
        RAMP_LIGHT.len(),
        u8::from(scale == GainScale::Auto),
        GainScale::FLOOR
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
            None => (String::new(), String::new()),
        };
        // Blank because nothing played, or blank because nobody was
        // listening for it? The cell has to say which.
        //
        // Only where it is blank. A day recorded in part still shades from
        // what was recorded, and a pattern laid over that colour obscured
        // the reading without adding one -- a day with logs is a day with
        // logs. Where there is no colour the mark stays, because a plain
        // empty cell would say nothing played, and part of that day was
        // never listened to. The tooltip gives the figure either way.
        let mark = match day.recorded() {
            Recorded::None => " cal-void",
            Recorded::Partial if tone.is_none() => " cal-part",
            _ => "",
        };
        let dot = if day.quiet_secs > 0 {
            "<span class=\"cal-dot\"></span>"
        } else {
            ""
        };
        let told = match day.recorded() {
            Recorded::None => " · no record: the logger was not running".to_owned(),
            Recorded::Partial => {
                format!(" · recorded {} of the day", human_secs(day.recorded_secs))
            }
            Recorded::Full => String::new(),
        };
        let title = match day.sustained {
            Some(v) => format!(
                "{} · 5-min sustained {}, peak {} · {} playing, {} in quiet hours",
                human_date(day.date),
                pct(v),
                pct(day.peak.unwrap_or(v)),
                human_secs(day.playing_secs),
                human_secs(day.quiet_secs)
            ),
            None if day.recorded() == Recorded::None => human_date(day.date),
            None => format!("{} · nothing played", human_date(day.date)),
        };
        let title = format!("{title}{told}");
        o.push_str(&format!(
            "<td class=\"cal-day{mark}\" data-date=\"{}\" title=\"{}\" role=\"button\" \
             tabindex=\"0\" style=\"background:{bg};color:{ink}\">\
             <span class=\"cal-in\">{}{dot}</span></td>",
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
fn filter_bar(report: &Report, players: &[String], quiet: &QuietHours) -> String {
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
    let mut whens: Vec<String> = report
        .spans
        .iter()
        .map(|s| when_of(s, quiet).to_owned())
        .collect();
    whens.sort();
    whens.dedup();

    let groups = [
        ("host", values(|s| s.host.clone())),
        ("player", values(|s| s.player.clone())),
        ("during", whens),
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
            // Players carry the colour their bars will have. The list is
            // built the same way the timeline builds its own, so the slots
            // line up rather than happening to agree.
            let swatch = if *kind == "player" {
                let slot = players.iter().position(|p| p == v).unwrap_or(0) % 3 + 1;
                format!("<span class=\"swatch\" style=\"background:var(--series-{slot})\"></span>")
            } else {
                String::new()
            };
            o.push_str(&format!(
                "<label><input type=\"checkbox\" data-kind=\"{kind}\" value=\"{0}\" checked>{swatch} {0}</label>",
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
    o.push_str("<th>Period</th><th>Duration</th><th>Player</th><th>Media</th><th>Output</th><th>Volume</th><th class=\"nowrap\">Quiet hrs</th></tr></thead><tbody>\n");
    let columns = 7 + usize::from(hostcol);
    let mut heading: Option<jiff::civil::Date> = None;
    for s in &report.spans {
        let q = quiet_seconds(s, quiet);
        // One heading per day, so the rows beneath it need carry no date at
        // all and their times line up as times. Only where there is more
        // than one day to separate: a single heading over the whole table
        // says nothing the report's own subtitle has not.
        if wide && heading != Some(s.start.date()) {
            heading = Some(s.start.date());
            o.push_str(&format!(
                "<tr class=\"daybreak\" data-day=\"{0}\"><td colspan=\"{columns}\">\
                 {1} <span class=\"muted\">{2}</span></td></tr>\n",
                s.start.date(),
                human_date(s.start.date()),
                s.start.strftime("%A"),
            ));
        }
        // The end repeats its date only when the span crosses midnight.
        let end = if s.start.date() == s.end.date() {
            hhmmss(&s.end)
        } else {
            clock(&s.end, true)
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
        let gain =
            s.levels
                .map(|l| {
                    format!(
                    "<span>{}<u>max</u></span><span>{}<u>med</u></span><span>{}<u>min</u></span>",
                    pct(l.max), pct(l.median), pct(l.min)
                )
                })
                .unwrap_or_else(|| "—".into());
        o.push_str(&format!(
            "<tr data-host=\"{}\" data-player=\"{}\" data-device=\"{}\" data-date=\"{}\" \
             data-during=\"{}\" data-sustained=\"{}\" data-quiet=\"{}\">",
            esc(&s.host),
            esc(&s.player),
            esc(s.sink_label.as_deref().or(s.sink.as_deref()).unwrap_or("")),
            s.start.date(),
            when_of(s, quiet),
            s.sustained.map(|v| format!("{v:.4}")).unwrap_or_default(),
            q,
        ));
        if hostcol {
            o.push_str(&format!("<td class=\"muted\">{}</td>", esc(&s.host)));
        }
        o.push_str(&format!(
            "<td class=\"num\">{} –<span class=\"sub2\">{end}</span></td>\
             <td class=\"num\">{}</td><td>{}</td><td>{}</td><td class=\"muted\">{}</td>\
             <td class=\"num stack\">{gain}</td><td class=\"num {}\">{}</td></tr>\n",
            hhmmss(&s.start),
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

fn footer_html(integrity: &Integrity, report: &Report, quiet: &QuietHours) -> Result<String> {
    let mut o = String::from("<footer>\n");
    // Beside the integrity notes rather than above the data: both are
    // statements about the record itself rather than about the evening.
    if let Some(line) = record_note(report, quiet)? {
        o.push_str(&format!("<p><b>Record.</b> {}</p>\n", esc(&line)));
    }
    o.push_str("<p><b>Integrity.</b> ");
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
    let stamp = |z: &Zoned| z.strftime("%d-%m-%Y %H:%M:%S %:z").to_string();
    o.push_str(&format!(
        "<p class=\"muted\">Generated {} · window {} to {} · quiet hours {}. Volumes are the cubic figure a slider displays; the log keeps the linear sink gain itself.</p>\n",
        stamp(&Zoned::now()),
        stamp(&report.window.from),
        stamp(&report.window.to),
        if quiet.is_empty() { "none".to_owned() } else { esc(&quiet.describe().join(", ")) },
    ));
    o.push_str("</footer>\n");
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which marks share a block is decided by where they land, so this
    /// asks it in those terms. Widths are in shares of a day: `APART` is
    /// four pixels of an 880-pixel track, about six and a half minutes.
    #[test]
    fn marks_share_a_block_when_the_page_would_collide_them() {
        let mins = |m: f64| m / 1440.0 * 100.0;

        // Far apart: two blocks, whatever their widths.
        let got = blocks(&[(mins(0.0), mins(30.0)), (mins(120.0), mins(30.0))]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].marks, 0..1);
        assert_eq!(got[1].marks, 1..2);

        // Back to back, as one track of a record follows the next.
        let got = blocks(&[(mins(0.0), mins(4.0)), (mins(4.0), mins(4.0))]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].marks, 0..2);
        assert_eq!(got[0].right, mins(8.0), "the block reaches the last edge");

        // A pause too short to draw a gap in merges; a longer one does not.
        assert_eq!(
            blocks(&[(mins(0.0), mins(4.0)), (mins(8.0), mins(4.0))]).len(),
            1
        );
        assert_eq!(
            blocks(&[(mins(0.0), mins(4.0)), (mins(20.0), mins(4.0))]).len(),
            2
        );

        // A long mark holding over a short one that started later is what
        // keeps the furthest edge, not the last mark's own: two players on
        // one host play at once. Taking the last edge would close the block
        // here and draw the third mark apart from a span still under way.
        let got = blocks(&[
            (mins(0.0), mins(90.0)),
            (mins(10.0), mins(3.0)),
            (mins(60.0), mins(5.0)),
        ]);
        assert_eq!(got.len(), 1, "one block, not two");
        assert_eq!(got[0].marks, 0..3);

        assert!(blocks(&[]).is_empty());
    }

    /// The scale a report speaks is the one its reader's slider speaks.
    ///
    /// The figures are from the live check in the design notes: a speaker
    /// sampled at 0.008 linear read 20% on its own front panel and 25/127
    /// over AVRCP. A report saying "0.008" invites the reader to drag a
    /// slider to 1%, which is about eighteen decibels down from where the
    /// music actually was.
    #[test]
    fn a_gain_is_shown_on_the_scale_every_slider_uses() {
        assert!((cubic(0.008) - 0.200).abs() < 1e-9, "{}", cubic(0.008));
        // A slider at half way is an eighth of the linear gain, which is
        // the pair most often mistaken for each other.
        assert!((cubic(0.125) - 0.5).abs() < 1e-9, "{}", cubic(0.125));
        // The two scales agree only at the ends.
        assert_eq!(cubic(0.0), 0.0);
        assert_eq!(cubic(1.0), 1.0);
        // Monotonic, which is what lets min/median/max convert one by one
        // rather than needing the samples back.
        assert!(cubic(0.3) < cubic(0.4) && cubic(0.4) < cubic(0.5));
        // A sink reporting a shade below zero must not produce NaN and
        // colour a calendar day with it.
        assert_eq!(cubic(-0.0001), 0.0);
    }

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

    /// The automatic scale exists so that days which differ are drawn
    /// differently; it must not also turn days which barely differ into a
    /// full sweep from white to black.
    #[test]
    fn an_auto_scale_spans_the_days_it_colours() {
        let (lo, hi) = GainScale::Auto.resolve([0.12, 0.44, 0.68, 0.31]);
        assert_eq!((lo, hi), (0.12, 0.68));

        // A flat month is widened around its middle rather than stretched.
        let (lo, hi) = GainScale::Auto.resolve([0.40, 0.41, 0.405]);
        assert!(hi - lo >= GainScale::FLOOR, "{lo} .. {hi}");
        assert!(
            lo < 0.40 && hi > 0.41,
            "the days sit inside it: {lo} .. {hi}"
        );

        // Never below zero, however quiet: a negative gain is not a thing.
        let (lo, _) = GainScale::Auto.resolve([0.01]);
        assert_eq!(lo, 0.0);

        // Nothing to colour still has to print a legend.
        let (lo, hi) = GainScale::Auto.resolve([]);
        assert!(lo < hi);

        // Fixed bounds are used as given, whatever the days say.
        assert_eq!(GainScale::Fixed(0.3, 0.7).resolve([0.01, 0.99]), (0.3, 0.7));
        assert_eq!("auto".parse::<GainScale>().unwrap(), GainScale::Auto);
        assert_eq!(
            "0.3:0.7".parse::<GainScale>().unwrap(),
            GainScale::Fixed(0.3, 0.7)
        );
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
    fn volume_columns_are_present_and_ordered() {
        let keys: Vec<&str> = COLUMNS.iter().map(|c| c.key).collect();
        let at = |k: &str| keys.iter().position(|x| *x == k).expect(k);
        assert!(at("volume_min") < at("volume_median"));
        assert!(at("volume_median") < at("volume_max"));
        assert!(at("volume_max") < at("volume_samples"));
        // The columns were gain_* and held linear gain. A consumer handed a
        // file whose headers had not changed but whose numbers had would
        // read a slider at a fifth of the way up as one at a hundredth, so
        // the rename is the signal that the scale moved.
        assert!(
            !keys.iter().any(|k| k.starts_with("gain_")),
            "a column naming the old scale is left: {keys:?}"
        );
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
            coverage: Vec::new(),
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
