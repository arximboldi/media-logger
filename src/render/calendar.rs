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
         data-auto=\"{}\" data-floor=\"{}\" data-held=\"{}\">",
        RAMP_LIGHT.len(),
        u8::from(scale == GainScale::Auto),
        GainScale::FLOOR,
        // How long a level has to hold to be one, which the script needs
        // to know before it repeats the claim about a filtered day.
        crate::query::SUSTAINED
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
        let played = day.playing_secs > 0 || day.peak.is_some();
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
            //
            // Nothing played, not merely nothing held: a day of tracks
            // too short to hold a level for five minutes has no colour
            // and is not a silence.
            _ if !played => " cal-silent",
            _ => "",
        };
        let dot = if day.quiet_secs > 0 {
            "<span class=\"cal-dot\"></span>"
        } else {
            ""
        };
        let told = told(day);
        // Has a twin in the filter script, which says the same sentence
        // again about whichever rows a filter leaves. A change to the
        // wording here is a change there.
        let when = human_date(day.date);
        let levels = match (day.sustained, day.peak) {
            (Some(held), peak) => format!(
                " · 5-min sustained {}, peak {}",
                pct(held),
                pct(peak.unwrap_or(held))
            ),
            (None, Some(peak)) => format!(" · peak {}", pct(peak)),
            (None, None) => String::new(),
        };
        let title = if played {
            format!(
                "{when}{levels} · {} playing, {} in quiet hours",
                human_secs(day.playing_secs),
                human_secs(day.quiet_secs)
            )
        } else if day.recorded() == Recorded::None {
            when.clone()
        } else {
            format!("{when} · nothing played")
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
             data-level=\"{level}\" data-when=\"{when}\" data-told=\"{}\" title=\"{}\" \
             role=\"button\" tabindex=\"0\" style=\"background:{bg};color:{ink}\">\
             <span class=\"cal-in\">{}{dot}</span></td>",
            day.date,
            esc(&told),
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

/// What a day's cell adds about how much of the day was recorded.
///
/// By host, where there is more than one. A day counts as recorded only as
/// far as its least recorded machine, because what played on the one that
/// was off left no trace -- but the sentence used to be written from that
/// one figure too, and a day `ce3` had logged nine hours of playing on was
/// described, beside those nine hours, as "no record: the logger was not
/// running". It was not running on `ce2`.
fn told(day: &crate::query::DaySummary) -> String {
    let hosts: Vec<(&str, Recorded, i64)> = day.hosts().collect();
    let same = |which: Recorded| hosts.iter().all(|(_, how, _)| *how == which);
    // One machine, or all of them alike: nothing needs naming.
    if hosts.len() < 2 || same(Recorded::None) || same(Recorded::Full) {
        return match day.recorded() {
            Recorded::None => " · no record: the logger was not running".to_owned(),
            Recorded::Partial => {
                format!(" · recorded {} of the day", human_secs(day.recorded_secs))
            }
            Recorded::Full => String::new(),
        };
    }
    hosts
        .iter()
        .filter_map(|(host, how, secs)| match how {
            Recorded::None => Some(format!(
                " · no record on {host}: the logger was not running there"
            )),
            Recorded::Partial => Some(format!(
                " · recorded {} of the day on {host}",
                human_secs(*secs)
            )),
            Recorded::Full => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(by_host: &[(&str, i64)]) -> crate::query::DaySummary {
        crate::query::DaySummary {
            date: jiff::civil::date(2026, 11, 15),
            sustained: None,
            peak: None,
            playing_secs: 0,
            quiet_secs: 0,
            recorded_secs: by_host.iter().map(|(_, s)| *s).min().unwrap_or(0),
            window_secs: 86_400,
            by_host: by_host.iter().map(|(h, s)| ((*h).to_owned(), *s)).collect(),
        }
    }

    /// A day is described by what each machine recorded of it, not by the
    /// least of them alone.
    ///
    /// One machine switched off made the whole day "no record: the logger
    /// was not running", in the same breath as the nine hours of playing
    /// the other machine had logged.
    #[test]
    fn a_day_one_host_missed_is_not_a_day_nobody_recorded() {
        let mixed = day(&[("ce2", 0), ("ce3", 32_640)]);
        // Still not a day that can be vouched for as a whole.
        assert_eq!(mixed.recorded(), Recorded::None);
        assert_eq!(
            told(&mixed),
            " · no record on ce2: the logger was not running there \
             · recorded 9h04m of the day on ce3"
        );
        // A host that recorded all of it needs no mention.
        assert_eq!(
            told(&day(&[("ce2", 0), ("ce3", 86_400)])),
            " · no record on ce2: the logger was not running there"
        );

        // Where they agree, or there is one, the sentence names nobody.
        assert_eq!(
            told(&day(&[("ce2", 0), ("ce3", 0)])),
            " · no record: the logger was not running"
        );
        assert_eq!(told(&day(&[("ce2", 86_400), ("ce3", 86_400)])), "");
        assert_eq!(
            told(&day(&[("ce3", 32_640)])),
            " · recorded 9h04m of the day"
        );
    }
}
