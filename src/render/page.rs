//! The html page: its shell and stylesheet, the filter bar, the spans
//! table, the footer -- and the pdf, which is this page printed.
//!
//! The timeline and the calendar are large enough to read on their own
//! and live beside this file; everything else the page is made of is
//! here, in the order the reader meets it.

use super::*;

// -------------------------------------------------------------------- pdf

pub(super) fn pdf(html_body: &str, path: &Path) -> Result<()> {
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

/// The script behind the filter bar, the calendar and the timeline.
///
/// The tags are here because they are markup; what they wrap is
/// javascript, and lives in a file an editor can read as javascript.
pub(super) fn filters() -> String {
    format!(
        "\n<script>\n{}</script>\n",
        include_str!("assets/filters.js")
    )
}

/// Reads the gain at the cursor's position along a bar.
///
/// The marks are a couple of pixels wide at day scale, so they are far too
/// small to be hover targets themselves; the bar is the target and the
/// position within it selects the value. Inert in the PDF, which is why the
/// figures also appear in the table.
pub(super) fn hover() -> String {
    format!(
        "<div id=\"tip\" role=\"status\" aria-live=\"polite\"></div>\n\
         <script>\n{}</script>\n",
        include_str!("assets/hover.js")
    )
}

// ------------------------------------------------------------------- html

/// The page's stylesheet, with its generated colours filled in.
///
/// Static apart from those: the ramps and the series are computed from
/// the palette, and the tick ladder from the spacing. Substituted rather
/// than formatted, because inside a `format!` every one of the
/// stylesheet's own braces has to be written twice -- two hundred and
/// twenty-nine of them, to carry seven values -- and the file stops
/// being css that an editor can read.
pub(super) fn stylesheet() -> String {
    let mut css = include_str!("assets/report.css").to_owned();
    for (slot, value) in [
        (
            "{light_ramp}",
            RAMP_LIGHT
                .iter()
                .zip(RAMP_INK)
                .enumerate()
                .map(|(i, (c, ink))| format!("  --ramp-{i}: {c}; --ramp-ink-{i}: {ink};"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "{dark_ramp}",
            RAMP_DARK
                .iter()
                .zip(["#ffffff", "#ffffff", "#ffffff", "#0b0b0b", "#0b0b0b"])
                .enumerate()
                .map(|(i, (c, ink))| format!("    --ramp-{i}: {c}; --ramp-ink-{i}: {ink};"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "{light_series}",
            SERIES_LIGHT
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        "  --series-{n}: {c};\n  --series-{n}-soft: {};",
                        mix(c, PANEL_LIGHT, 0.20),
                        n = i + 1
                    )
                })
                .chain(std::iter::once(format!(
                    "  --series-other: {OTHER_LIGHT};\n  --series-other-soft: {};",
                    mix(OTHER_LIGHT, PANEL_LIGHT, 0.20)
                )))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "{dark_series}",
            SERIES_DARK
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        "    --series-{n}: {c};\n    --series-{n}-soft: {};",
                        mix(c, PANEL_DARK, 0.28),
                        n = i + 1
                    )
                })
                .chain(std::iter::once(format!(
                    "    --series-other: {OTHER_DARK};\n    --series-other-soft: {};",
                    mix(OTHER_DARK, PANEL_DARK, 0.28)
                )))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        ("{tick_ladder}", tick_ladder().to_owned()),
        ("{tick_opacity}", tick_opacity().0.to_string()),
        ("{tick_opacity_print}", tick_opacity().1.to_string()),
    ] {
        css = css.replace(slot, &value);
    }
    css
}

pub(super) fn html(
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
{style}</style></head><body><main>
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
        style = stylesheet(),
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
        o.push_str(&hover());
    }
    if !report.window.instant {
        o.push_str(&filters());
    }
    o.push_str("</main></body></html>\n");
    Ok(o)
}

/// Multi-select filters over the dimensions a report actually has.
///
/// Inert in the PDF, which is right: these are for *finding* a window, while
/// the command line produces the document someone else reads.
pub(super) fn filter_bar(report: &Report, players: &[String], quiet: &QuietHours) -> String {
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
    // Volume is not a dimension with values to list but a threshold, and it
    // is worth offering wherever anything was measured at all.
    let levelled = report.spans.iter().any(|s| s.levels.is_some());
    if useful.is_empty() && !levelled {
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
                format!(
                    "<span class=\"swatch\" style=\"background:{}\"></span>",
                    series_var(players, v, false)
                )
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
    if levelled {
        // A slider to find a level by eye and a box to state one exactly:
        // the first is how a reader asks "louder than about this", the
        // second how they ask for the figure they already have in mind.
        o.push_str(
            "<details class=\"pick vol\" id=\"volpick\" title=\"Show only playback that \
             reached at least this volume -- the loudest the span got, on the slider's \
             own scale.\">\
             <summary>volume<span class=\"count\"></span></summary>\
             <div class=\"volbox\">\
             <input type=\"range\" id=\"volrange\" min=\"0\" max=\"100\" step=\"1\" \
             value=\"0\" aria-label=\"minimum volume\">\
             <span class=\"volnum\"><input type=\"number\" id=\"volnum\" min=\"0\" \
             max=\"100\" step=\"1\" value=\"0\" aria-label=\"minimum volume\">%</span>\
             </div></details>",
        );
    }
    o.push_str(
        "<button id=\"clear\" hidden>clear filters</button>\
         <span id=\"shown\" class=\"muted\"></span></div>\n",
    );
    o
}

pub(super) fn spans_table(report: &Report, quiet: &QuietHours) -> String {
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
    for st in stretches(report) {
        let s = st.head();
        let q = st.quiet_seconds(quiet);
        // One heading per day, so the rows beneath it need carry no date at
        // all and their times line up as times. Only where there is more
        // than one day to separate: a single heading over the whole table
        // says nothing the report's own subtitle has not.
        if wide && heading != Some(st.start().date()) {
            heading = Some(st.start().date());
            o.push_str(&format!(
                "<tr class=\"daybreak\" data-day=\"{0}\"><td colspan=\"{columns}\">\
                 {1} <span class=\"muted\">{2}</span></td></tr>\n",
                s.start.date(),
                human_date(s.start.date()),
                s.start.strftime("%A"),
            ));
        }
        // The end repeats its date only when the span crosses midnight.
        let end = if st.start().date() == st.end().date() {
            hhmmss(st.end())
        } else {
            clock(st.end(), true)
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
        let parts = st
            .parts()
            .map(|p| format!("<span class=\"src\">{}</span>", esc(&p)))
            .unwrap_or_default();
        let media = format!("{}{source}{parts}", esc(&s.label()));
        let gain =
            st.levels()
                .map(|l| {
                    format!(
                    "<span>{}<u>max</u></span><span>{}<u>med</u></span><span>{}<u>min</u></span>",
                    pct(l.max), pct(l.median), pct(l.min)
                )
                })
                .unwrap_or_else(|| "—".into());
        o.push_str(&format!(
            "<tr data-host=\"{}\" data-player=\"{}\" data-device=\"{}\" data-date=\"{}\" \
             data-during=\"{}\" data-sustained=\"{}\" data-quiet=\"{}\" \
             data-played=\"{}\" data-vol=\"{}\">",
            esc(&s.host),
            esc(&s.player),
            esc(s.sink_label.as_deref().or(s.sink.as_deref()).unwrap_or("")),
            st.start().date(),
            st.during(quiet),
            st.sustained()
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            q,
            st.played(),
            // The loudest this span reached. A threshold asks whether it
            // ever got that loud, which the peak answers and the sustained
            // figure cannot -- that is absent for anything under five
            // minutes, which is most of a record.
            st.levels()
                .map(|l| format!("{:.4}", l.max))
                .unwrap_or_default(),
        ));
        if hostcol {
            o.push_str(&format!("<td class=\"muted\">{}</td>", esc(&s.host)));
        }
        o.push_str(&format!(
            "<td class=\"num\">{} –<span class=\"sub2\">{end}</span></td>\
             <td class=\"num\">{}</td><td>{}</td><td>{}</td><td class=\"muted\">{}</td>\
             <td class=\"num stack\">{gain}</td><td class=\"num {}\">{}</td></tr>\n",
            hhmmss(st.start()),
            st.duration(),
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

    // Printed whether or not quiet hours are configured, and before the
    // quiet figure, which is a part of it.
    let played: i64 = stretches(report).iter().map(|st| st.played()).sum();
    o.push_str(&format!(
        "<p>Total playback: <b id=\"ptotal\">{}</b></p>\n",
        human_secs(played)
    ));

    let total: i64 = report.spans.iter().map(|s| quiet_seconds(s, quiet)).sum();
    if !quiet.is_empty() {
        o.push_str(&format!(
            "<p>Playback inside quiet hours: <b id=\"qtotal\" class=\"{}\">{}</b></p>\n",
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

pub(super) fn footer_html(
    integrity: &Integrity,
    report: &Report,
    quiet: &QuietHours,
) -> Result<String> {
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
