//! The day strip: what played, where it lands, and the gain under it.
//!
//! Everything here is geometry. Where a mark falls on a day's track, which
//! marks are close enough together to share a block, and how much track a
//! block needs once every mark in it has been given its minimum width.

use super::*;

/// One pixel of a day track, as a share of the day.
///
/// The track is what a 60rem page has left over once the day gutter is
/// taken, so about 880px wide and narrower on a phone. The distances below
/// are drawing distances rather than spans of time, so they are stated in
/// those pixels and converted once, here.
pub(super) const PX: f64 = 100.0 / 880.0;

/// Narrowest a bar is drawn, matching `min-width` in the stylesheet. A
/// three-minute song is a quarter of this, and would otherwise vanish.
pub(super) const FLOOR: f64 = 2.0 * PX;

/// Clear track two marks need between them to read as two marks. A bar
/// carries two pixels of halo either side, so anything less and the halos
/// close over each other -- and over the bars, which are no wider.
pub(super) const APART: f64 = 4.0 * PX;

/// Closest two boundary marks are drawn, as a share of the day: a bar's
/// own minimum width, unless asked for more.
///
/// A boundary nearer than this to the last one drawn is left out, and the
/// tracks either side of it read as one. Marks are drawn where they fell,
/// so on a dense evening they fall closer together than anything can be
/// drawn -- forty tracks to the hour is one every pixel and a half -- and
/// a line at each would fill the block with lines. `FLOOR` is the
/// narrowest a bar is drawn, so it is the least two boundaries can be
/// apart and still have a bar between them.
///
/// `MEDIA_LOGGER_TICK_SPACING` widens it, in pixels; it is a knob for
/// looking rather than configuration, and nothing sets it.
pub(super) fn spacing() -> f64 {
    let asked = std::env::var("MEDIA_LOGGER_TICK_SPACING")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0)
        * PX;
    asked.max(FLOOR)
}

/// One span as the page will draw it, in shares of the day.
struct Mark<'a> {
    of: &'a Stretch<'a>,
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
/// Has a twin in the filter script. Which marks share a block depends on
/// which marks are on the page, so a filter changes the answer and the
/// script lays the blocks again over whatever survived. A change to this
/// rule is a change there.
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

/// Lays a block's marks out and says how much track the block covers.
///
/// Every mark opens where its playing began, and the block is as wide as
/// the playing was. That is the whole of it, and it has to be: the track
/// has the hours ruled underneath, and a mark's place on it is a time.
///
/// It was otherwise. A mark is never drawn narrower than `FLOOR`, and each
/// used to be pushed along to open where the one before it closed, so that
/// none hid another. Each push was two pixels, which is three minutes and
/// a quarter, and they added up: a night of a hundred and thirty-three
/// short videos between 00:42 and 02:35 was laid end to end as a hundred
/// and thirty-three floors and drawn as a block running to eight in the
/// morning -- five and a half hours of playing that the log does not hold,
/// through quiet hours, on the page that exists to show when things played.
///
/// So marks that fall closer together than `FLOOR` now overlap, the later
/// drawn over the earlier, and what shows of each is the stretch up to the
/// next. Where that is under a pixel it does not show at all; the table
/// below has every row, and a pixel cannot hold more than it holds. Each
/// still runs on to where the next began when they do not overlap, so
/// that tracks which merely abut leave nothing between them for the
/// renderer to round.
///
/// Nothing can fall outside the box: a mark's width is at least `FLOOR`
/// before the block is drawn around it, so the block already reaches as
/// far as every mark in it does.
fn lay(block: &Block, marks: &[(f64, f64)]) -> (Vec<(f64, f64)>, f64) {
    let laid = marks
        .iter()
        .enumerate()
        .map(|(i, &(left, width))| match marks.get(i + 1) {
            Some(&(next, _)) if next >= left + width => (left, next),
            _ => (left, left + width),
        })
        .collect();
    (laid, block.right - block.left)
}

pub(super) fn timeline(report: &Report, quiet: &QuietHours, players: &[String]) -> Result<String> {
    let tz = report.window.from.time_zone().clone();
    // One mark per row of the table, not one per span. The two halves of
    // the page are answering the same question and used to disagree: the
    // table grouped a player's bounce through Paused into one row while
    // the timeline drew a mark per bounce, then hid most of them.
    let grouped = stretches(report);
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
                "<span class=\"key\"><span class=\"swatch\" \
                 style=\"background:{}\"></span>{}</span>",
                series_var(players, p, false),
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

    // The drawing distances travel with the markup: the script relays the
    // blocks when a filter narrows the page, and a second copy of these
    // numbers in the script would be a second thing to keep in step.
    o.push_str(&format!(
        "<div class=\"scroll\" id=\"tl\" data-apart=\"{APART:.5}\" data-floor=\"{FLOOR:.5}\" \
         data-spacing=\"{:.5}\">",
        spacing()
    ));

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
            for of in &grouped {
                if *of.end() <= day_start
                    || *of.start() >= day_end
                    || (many && of.head().host != **host)
                {
                    continue;
                }
                let left = pct(&of.start().clone().max(day_start.clone()), day);
                let right = pct(&of.end().clone().min(day_end.clone()), day);
                marks.push(Mark {
                    of,
                    left,
                    width: (right - left).max(FLOOR),
                });
            }

            let geometry: Vec<(f64, f64)> = marks.iter().map(|m| (m.left, m.width)).collect();
            for block in blocks(&geometry) {
                let held = &marks[block.marks.clone()];
                let (laid, full) = lay(&block, &geometry[block.marks.clone()]);
                let left = block.left;
                o.push_str(&format!(
                    "<div class=\"run\" style=\"left:{left:.3}%;width:{full:.3}%\">"
                ));
                // Boundaries are drawn after the tracks, so they sit above
                // them whatever order the tracks were painted in.
                let mut cuts = String::new();
                // Measured from the block's own edge, so the first mark does
                // not land against the rounded corner.
                let mut marked = left;
                // Each track runs to where the next began. The gaps
                // between them are what put them in one block: seconds, a
                // fifth of a pixel, and drawing them anyway put a line of
                // a different width at every boundary, none of them
                // meaning anything.
                //
                // Unless two overlap -- two players on one host play at
                // once, or two tracks fall inside one bar's width -- and
                // then each keeps its own edge and the later is drawn
                // over the earlier.
                for (i, (mark, &(a, b))) in held.iter().zip(laid.iter()).enumerate() {
                    let span = mark.of.head();
                    let (strong, soft) = (
                        series_var(players, &span.player, false),
                        series_var(players, &span.player, true),
                    );
                    // Every boundary there is room for is marked, and
                    // marked the same way. A record of three-minute songs
                    // is the case this is for: the marks come out small,
                    // but a reader counting them is reading the evening
                    // right, where leaving them joined says one long thing
                    // played.
                    if i > 0 && a - marked >= spacing && left + full - a >= spacing {
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
                    let from = mark.of.start().clone().max(day_start.clone());
                    let to = mark.of.end().clone().min(day_end.clone());
                    let (profile, gains) = gain_profile(report, &span.host, &from, &to, ceiling);
                    o.push_str(&format!(
                        "<div class=\"bar\" data-host=\"{host}\" data-player=\"{player}\" \
                         data-device=\"{device}\" data-date=\"{date}\" \
                         data-during=\"{during}\" data-vol=\"{vol}\" \
                         data-ml=\"{ml:.5}\" data-mw=\"{mw:.5}\" \
                         style=\"left:{l:.3}%;width:calc({wide:.3}% + 1px);\
                         --c:{strong};--cs:{soft}\" \
                         data-g=\"{gains}\" data-s0=\"{s0}\" data-s1=\"{s1}\" \
                         title=\"{title}\">{profile}{label}</div>",
                        l = (a - left) / full * 100.0,
                        wide = (b - a) / full * 100.0,
                        ml = mark.left,
                        mw = mark.width,
                        host = esc(&span.host),
                        player = esc(&span.player),
                        device = esc(span
                            .sink_label
                            .as_deref()
                            .or(span.sink.as_deref())
                            .unwrap_or("")),
                        date = mark.of.start().date(),
                        during = mark.of.during(quiet),
                        vol = mark
                            .of
                            .levels()
                            .map(|l| format!("{:.4}", l.max))
                            .unwrap_or_default(),
                        s0 = from.timestamp().as_second(),
                        s1 = to.timestamp().as_second(),
                        title = esc(&format!(
                            "{} · {} · {}–{}{}",
                            span.label(),
                            span.player,
                            hhmmss(mark.of.start()),
                            hhmmss(mark.of.end()),
                            mark.of
                                .parts()
                                .map(|p| format!(" · {p}"))
                                .unwrap_or_default()
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
pub(super) fn busy_on(report: &Report, date: jiff::civil::Date, tz: &jiff::tz::TimeZone) -> bool {
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
pub(super) fn gain_profile(
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
    /// A block is as wide as the playing it holds, however many marks
    /// that is, and every mark in it opens where its playing began.
    ///
    /// The track has the hours ruled under it. A night of a hundred and
    /// thirty-three short videos, an hour and fifty-three minutes of them,
    /// was drawn as a block seven and a quarter hours wide: each mark was
    /// floored at two pixels and pushed along to open where the last one
    /// closed, and two pixels is three minutes and a quarter.
    #[test]
    fn a_block_is_no_wider_than_the_time_it_covers() {
        let mins = |m: f64| m / 1440.0 * 100.0;
        // One every fifty-one seconds from 00:42, each a few seconds long.
        let night: Vec<(f64, f64)> = (0..133)
            .map(|i| (mins(42.0 + i as f64 * 0.85), FLOOR))
            .collect();
        let got = blocks(&night);
        assert_eq!(got.len(), 1, "nothing between them to read as a gap");
        let (laid, full) = lay(&got[0], &night);

        let closes = got[0].left + full;
        assert!(
            closes <= mins(42.0 + 132.0 * 0.85) + FLOOR + 1e-9,
            "the block must end with the last mark, not at {:.0} minutes past midnight",
            closes / 100.0 * 1440.0
        );
        for (i, (&(a, b), &(left, _))) in laid.iter().zip(&night).enumerate() {
            assert_eq!(a, left, "mark {i} opens where its playing began");
            // And none is drawn and then hidden, which is what pushing
            // them along was for: `overflow: hidden` keeps a bar inside
            // its block, and twenty-one of twenty-four once fell outside.
            assert!(b <= closes + 1e-9, "mark {i} ends past the box");
            assert!(b - a >= FLOOR - 1e-9, "mark {i} is thinner than FLOOR");
        }

        // Where nothing is crowded each mark runs to where the next began,
        // and the box is the marks' own extent.
        let roomy = [(10.0, 5.0), (15.0, 5.0)];
        let block = Block {
            marks: 0..2,
            left: 10.0,
            right: 20.0,
        };
        let (laid, full) = lay(&block, &roomy);
        assert_eq!(full, 10.0);
        assert_eq!(laid, vec![(10.0, 15.0), (15.0, 20.0)]);
    }
}
