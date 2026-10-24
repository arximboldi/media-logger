//! A month grid of days, shaded by how loud each one was.
//!
//! The overview and, on the page, the control that narrows the report to
//! a day. Shown in addition to the timeline and never instead of it.

use super::*;

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
pub(super) fn calendar(report: &Report, quiet: &QuietHours, scale: GainScale) -> Result<String> {
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
    // A day listened to that held nothing. The same question the cell asks,
    // so a key cannot appear for a mark that is not on the page.
    let silents = days
        .iter()
        .filter(|d| d.recorded() != Recorded::None && step(d.sustained).is_none())
        .count();
    // Keys only for what is actually on the page: a legend entry for a mark
    // that never appears is one more thing to decode for nothing.
    // Two of these come and go under filtering -- a day whose events are
    // all hidden is a day with nothing played -- so they are written once
    // and hidden until a cell carries the mark. No filter can conjure an
    // unrecorded day, so that key is settled here.
    let key = |on: bool, kind: &str, text: &str| {
        format!(
            "<span class=\"key\" data-key=\"{kind}\"{}>\
             <span class=\"swatch cal-{kind}-key\"></span>{text}</span>",
            if on { "" } else { " hidden" }
        )
    };
    let gaps = format!(
        "{}{}",
        key(silents > 0, "silent", "nothing played"),
        if voids > 0 {
            key(true, "void", "no record")
        } else {
            String::new()
        },
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
            // Nothing recorded: an empty cell, which is what a machine that
            // was never on leaves behind. The day's number still prints, so
            // it stays apart from the cells outside the window, which have
            // nothing in them at all.
            Recorded::None => "",
            // Recorded at all, and nothing played: a silence somebody was
            // listening for, which is a finding and not an absence. How
            // much of the day was covered is in the tooltip; it does not
            // earn a mark of its own, because a machine goes on and off
            // and almost every day is partial.
            _ if tone.is_none() => " cal-silent",
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
        // Whether anything was recorded, which is all the script needs to
        // tell a day filtered down to nothing from one never listened to.
        let rec = if day.recorded() == Recorded::None {
            "none"
        } else {
            "some"
        };
        // The day's own level, which is what this page was shaded by. The
        // script cannot rebuild it from the rows: a day's level is held
        // across whatever was playing, and no single span need hold it.
        let level = day.sustained.map(|v| format!("{v:.4}")).unwrap_or_default();
        o.push_str(&format!(
            "<td class=\"cal-day{mark}\" data-date=\"{}\" data-rec=\"{rec}\" \
             data-level=\"{level}\" title=\"{}\" role=\"button\" \
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
