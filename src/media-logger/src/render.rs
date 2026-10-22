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

pub fn render(
    report: &Report,
    integrity: &Integrity,
    quiet: &QuietHours,
    format: Format,
    out: Option<&Path>,
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
        Format::Html | Format::Pdf => html(report, integrity, quiet)?,
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
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(body.as_bytes())?;
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
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

// ------------------------------------------------------------------- text

fn text(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
    let mut o = String::new();
    o.push_str(&format!(
        "media-logger — {} (host {})\n\n",
        report.window.label, report.host
    ));

    let at = &report.at_start;
    let playing = at.playing();
    o.push_str(&format!("At {}\n", hhmmss(&report.window.from)));
    if playing.is_empty() {
        o.push_str("  nothing playing\n");
    } else {
        for p in playing {
            o.push_str(&format!(
                "  {} playing: {}\n",
                p.player,
                p.track.title.clone().unwrap_or_else(|| "(untitled)".into())
            ));
        }
    }
    match &at.sink {
        Some(s) => o.push_str(&format!(
            "  sink: {} ({}), volume {} {}\n",
            s.description.clone().unwrap_or_else(|| s.name.clone()),
            s.device_api.clone().unwrap_or_else(|| "?".into()),
            s.channel_volumes
                .iter()
                .map(|v| format!("{v:.3}"))
                .collect::<Vec<_>>()
                .join("/"),
            s.volume_scale,
        )),
        None => o.push_str("  sink: unknown\n"),
    }
    if let Some(as_of) = &at.as_of {
        o.push_str(&format!("  (state as of {as_of})\n"));
    }

    if report.window.instant {
        o.push_str(&footer_text(integrity, report));
        return Ok(o);
    }

    o.push_str("\nPlayback\n");
    if report.spans.is_empty() {
        o.push_str("  nothing played in this window\n");
    }
    for span in &report.spans {
        let q = quiet_seconds(span, quiet);
        o.push_str(&format!(
            "  {}–{}  {:>7}  {:10} {}\n",
            hhmmss(&span.start),
            hhmmss(&span.end),
            duration(span),
            span.player,
            span.label()
        ));
        if let Some(v) = span.volume {
            o.push_str(&format!(
                "  {:30}peak volume {v:.3} linear{}\n",
                "",
                span.sink_label
                    .as_deref()
                    .or(span.sink.as_deref())
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default()
            ));
        }
        if q > 0 {
            o.push_str(&format!(
                "  {:30}within quiet hours for {}\n",
                "",
                human_secs(q)
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
        if integrity.chain_ok {
            "intact"
        } else {
            "BROKEN"
        },
        integrity.lines
    ));
    for p in &integrity.problems {
        o.push_str(&format!("  PROBLEM: {p}\n"));
    }
    for (day, state) in &integrity.anchors {
        o.push_str(&format!("  {day}: {state}\n"));
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
                "start": rfc3339(&s.start),
                "end": rfc3339(&s.end),
                "duration_s": secs_between(&s.start, &s.end).max(0),
                "player": s.player,
                "title": s.title,
                "artist": s.artist,
                "url": s.url,
                "sink": s.sink,
                "peak_volume_linear": s.volume,
                "quiet_hours_s": quiet_seconds(s, quiet),
            })
        })
        .collect();

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "host": report.host,
        "window": {
            "label": report.window.label,
            "from": rfc3339(&report.window.from),
            "to": rfc3339(&report.window.to),
            "instant": report.window.instant,
        },
        "quiet_hours": quiet.describe(),
        "at_start": {
            "as_of": report.at_start.as_of,
            "players": report.at_start.players,
            "sink": report.at_start.sink,
        },
        "spans": spans,
        "integrity": {
            "chain_ok": integrity.chain_ok,
            "lines": integrity.lines,
            "problems": integrity.problems,
            "anchors": integrity.anchors.iter()
                .map(|(d, s)| serde_json::json!({"date": d, "status": s}))
                .collect::<Vec<_>>(),
        },
    }))?)
}

// -------------------------------------------------------------------- csv

fn csv(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
    let mut o = String::from(
        "start,end,duration_s,player,title,artist,url,sink,peak_volume_linear,quiet_hours_s\n",
    );
    for s in &report.spans {
        let cell = |v: &str| {
            if v.contains([',', '"', '\n']) {
                format!("\"{}\"", v.replace('"', "\"\""))
            } else {
                v.to_owned()
            }
        };
        o.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            rfc3339(&s.start),
            rfc3339(&s.end),
            secs_between(&s.start, &s.end).max(0),
            cell(&s.player),
            cell(s.title.as_deref().unwrap_or("")),
            cell(&s.artist.join("; ")),
            cell(s.url.as_deref().unwrap_or("")),
            cell(s.sink.as_deref().unwrap_or("")),
            s.volume.map(|v| format!("{v:.4}")).unwrap_or_default(),
            quiet_seconds(s, quiet),
        ));
    }
    // The footer travels with the data, as a comment the spreadsheet shows.
    o.push_str(&format!(
        "\n# chain {} ({} lines)\n",
        if integrity.chain_ok {
            "intact"
        } else {
            "BROKEN"
        },
        integrity.lines
    ));
    for (day, state) in &integrity.anchors {
        o.push_str(&format!("# {day}: {state}\n"));
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
    for (i, h) in [
        "Start",
        "End",
        "Duration",
        "Player",
        "Title",
        "Artist",
        "URL",
        "Sink",
        "Peak volume (linear)",
        "In quiet hours",
    ]
    .iter()
    .enumerate()
    {
        sheet.write_with_format(0, i as u16, *h, &head)?;
    }
    for (r, s) in report.spans.iter().enumerate() {
        let row = r as u32 + 1;
        let q = quiet_seconds(s, quiet);
        sheet.write(row, 0, rfc3339(&s.start))?;
        sheet.write(row, 1, rfc3339(&s.end))?;
        sheet.write(row, 2, duration(s))?;
        sheet.write(row, 3, s.player.as_str())?;
        sheet.write(row, 4, s.title.as_deref().unwrap_or(""))?;
        sheet.write(row, 5, s.artist.join("; "))?;
        sheet.write(row, 6, s.url.as_deref().unwrap_or(""))?;
        sheet.write(row, 7, s.sink.as_deref().unwrap_or(""))?;
        match s.volume {
            Some(v) => sheet.write(row, 8, v)?,
            None => sheet.write(row, 8, "")?,
        };
        if q > 0 {
            sheet.write_with_format(row, 9, human_secs(q), &flag)?;
        } else {
            sheet.write(row, 9, "")?;
        }
    }
    sheet.set_column_width(0, 26)?;
    sheet.set_column_width(1, 26)?;
    sheet.set_column_width(4, 40)?;
    sheet.set_column_width(6, 50)?;
    sheet.set_column_width(7, 40)?;

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
    put("Host", report.host.clone())?;
    put("Window", report.window.label.clone())?;
    put("Quiet hours", quiet.describe().join(", "))?;
    put(
        "Chain",
        format!(
            "{} ({} lines)",
            if integrity.chain_ok {
                "intact"
            } else {
                "BROKEN"
            },
            integrity.lines
        ),
    )?;
    for p in &integrity.problems {
        put("Problem", p.clone())?;
    }
    for (day, state) in &integrity.anchors {
        put(day, state.clone())?;
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

// ------------------------------------------------------------------- html

fn html(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
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
  --flag: #b03030;
{light_series}
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    color-scheme: dark;
    --surface: #1a1a19; --panel: #232322;
    --ink: #ffffff; --ink-2: #c3c2b7; --ink-3: #96958c;
    --rule: #3a3a38; --quiet: #2c2c2a; --quiet-ink: #9a988f;
    --flag: #e66767;
{dark_series}
  }}
}}
:root[data-theme="dark"] {{
  color-scheme: dark;
  --surface: #1a1a19; --panel: #232322;
  --ink: #ffffff; --ink-2: #c3c2b7; --ink-3: #96958c;
  --rule: #3a3a38; --quiet: #2c2c2a; --quiet-ink: #9a988f;
  --flag: #e66767;
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
  font-variant-numeric: tabular-nums; margin-left: 5.5rem; }}
.axis span {{ flex: 1 1 0; }}
.row {{ display: flex; align-items: center; gap: .75rem; margin: .3rem 0; }}
.day {{ width: 5rem; flex: none; color: var(--ink-2); font-size: 12px;
  font-variant-numeric: tabular-nums; }}
.track {{ position: relative; flex: 1 1 auto; height: 26px;
  background: var(--panel); border: 1px solid var(--rule); border-radius: 4px;
  overflow: hidden; }}
.band {{ position: absolute; top: 0; bottom: 0; background: var(--quiet); }}
.hour {{ position: absolute; top: 0; bottom: 0; width: 1px; background: var(--rule);
  opacity: .55; }}
.bar {{ position: absolute; top: 5px; height: 16px; border-radius: 3px;
  box-shadow: 0 0 0 2px var(--panel); min-width: 2px; }}
.bar span {{ position: absolute; left: 6px; top: 50%; transform: translateY(-50%);
  font-size: 11px; color: #fff; white-space: nowrap; pointer-events: none; }}
table {{ border-collapse: collapse; width: 100%; font-size: 13px; }}
th, td {{ text-align: left; padding: .45rem .6rem; border-bottom: 1px solid var(--rule);
  vertical-align: top; }}
th {{ color: var(--ink-2); font-weight: 600; }}
td.num {{ font-variant-numeric: tabular-nums; white-space: nowrap; }}
.flag {{ color: var(--flag); font-weight: 600; }}
.muted {{ color: var(--ink-3); }}
footer {{ margin-top: 2.5rem; padding-top: 1rem; border-top: 1px solid var(--rule);
  color: var(--ink-2); font-size: 12.5px; }}
footer b {{ color: var(--ink); }}
.scroll {{ overflow-x: auto; }}
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
        host = esc(&report.host),
        quiet_note = if quiet.is_empty() {
            String::new()
        } else {
            format!(" · quiet hours {}", esc(&quiet.describe().join(", ")))
        },
        light_series = SERIES_LIGHT
            .iter()
            .enumerate()
            .map(|(i, c)| format!("  --series-{}: {c};", i + 1))
            .collect::<Vec<_>>()
            .join("\n"),
        dark_series = SERIES_DARK
            .iter()
            .enumerate()
            .map(|(i, c)| format!("    --series-{}: {c};", i + 1))
            .collect::<Vec<_>>()
            .join("\n"),
    ));

    // -- opening state -----------------------------------------------------
    o.push_str("<h2>State at the start of the window</h2>\n<p>");
    let playing = report.at_start.playing();
    if playing.is_empty() {
        o.push_str("<b>Nothing playing.</b>");
    } else {
        o.push_str("<b>Playing:</b> ");
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
    if let Some(s) = &report.at_start.sink {
        o.push_str(&format!(
            "<br>Output: {} ({}) at volume {} <span class=\"muted\">{}</span>",
            esc(s.description.as_deref().unwrap_or(&s.name)),
            esc(s.device_api.as_deref().unwrap_or("?")),
            s.channel_volumes
                .iter()
                .map(|v| format!("{v:.3}"))
                .collect::<Vec<_>>()
                .join(" / "),
            esc(&s.volume_scale),
        ));
    }
    if let Some(as_of) = &report.at_start.as_of {
        o.push_str(&format!(
            "<br><span class=\"muted\">state as of {}</span>",
            esc(as_of)
        ));
    }
    o.push_str("</p>\n");

    if !report.window.instant {
        o.push_str(&timeline(report, quiet, &players, &colour)?);
        o.push_str(&spans_table(report, quiet));
    }

    o.push_str(&footer_html(integrity, report, quiet));
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
    o.push_str("</div>\n");

    o.push_str("<div class=\"scroll\"><div class=\"axis\">");
    for h in (0..24).step_by(3) {
        o.push_str(&format!("<span>{h:02}:00</span>"));
    }
    o.push_str("</div>\n");

    let pct = |z: &Zoned, day: Date| -> f64 {
        let start = day.to_zoned(tz.clone()).ok();
        match start {
            Some(s) => (secs_between(&s, z) as f64 / 864.0).clamp(0.0, 100.0),
            None => 0.0,
        }
    };

    for day in days_in(report) {
        o.push_str(&format!(
            "<div class=\"row\"><div class=\"day\">{}</div><div class=\"track\">",
            day
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
            if span.end <= day_start || span.start >= day_end {
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
            o.push_str(&format!(
                "<div class=\"bar\" style=\"left:{a:.3}%;width:{width:.3}%;background:{}\" title=\"{} · {} · {}\">{label}</div>",
                colour(&span.player, false),
                esc(&span.player),
                esc(&span.label()),
                esc(&format!("{}–{}", hhmmss(&span.start), hhmmss(&span.end))),
            ));
        }
        o.push_str("</div></div>\n");
    }
    o.push_str("</div>\n");
    Ok(o)
}

fn spans_table(report: &Report, quiet: &QuietHours) -> String {
    let mut o = String::from("<h2>Playback</h2>\n");
    if report.spans.is_empty() {
        return o + "<p><b>Nothing played in this window.</b></p>\n";
    }
    o.push_str("<div class=\"scroll\"><table><thead><tr><th>Start</th><th>End</th><th>Duration</th><th>Player</th><th>Track</th><th>Output</th><th>Peak volume</th><th>Quiet hours</th></tr></thead><tbody>\n");
    for s in &report.spans {
        let q = quiet_seconds(s, quiet);
        o.push_str(&format!(
            "<tr><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td>{}</td><td>{}</td><td class=\"muted\">{}</td><td class=\"num\">{}</td><td class=\"num {}\">{}</td></tr>\n",
            hhmmss(&s.start),
            hhmmss(&s.end),
            duration(s),
            esc(&s.player),
            esc(&s.label()),
            esc(s.sink_label.as_deref().or(s.sink.as_deref()).unwrap_or("")),
            s.volume.map(|v| format!("{v:.3}")).unwrap_or_else(|| "—".into()),
            if q > 0 { "flag" } else { "" },
            if q > 0 { human_secs(q) } else { "—".to_owned() },
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
        if integrity.chain_ok {
            "intact"
        } else {
            "BROKEN"
        },
        integrity.lines
    ));
    o.push_str("Each line hashes the literal bytes of the one before it, continuing across day boundaries. Closed days are timestamped by two independent authorities.</p>\n");
    for p in &integrity.problems {
        o.push_str(&format!("<p class=\"flag\">{}</p>\n", esc(p)));
    }
    o.push_str("<table><tbody>\n");
    for (day, state) in &integrity.anchors {
        o.push_str(&format!(
            "<tr><td class=\"num\">{}</td><td>{}</td></tr>\n",
            esc(day),
            esc(state)
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
