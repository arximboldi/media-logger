//! What the page is coloured with, and the scale those colours stand for.
//!
//! One home for the palette, the calendar's ramp, and the cube root that
//! turns a linear gain into the figure every slider shows. The conversion
//! runs once, at the top of `render`: a dozen places print a gain, and one
//! of them left in the other scale is the eighteen-decibel mistake the
//! conversion exists to stop.

use super::*;

/// Categorical slots, assigned per player in fixed order and never cycled.
///
/// Eight, in the order the method documents, validated against both
/// surfaces on the adjacent pairlist that bars take. Three was enough
/// while a machine ran a music player, a video player and a browser; a
/// ninth program is not a ninth hue, because there is no ninth hue that
/// can be told from the other eight. It takes the neutral below.
pub(super) const SERIES_LIGHT: [&str; 8] = [
    "#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#4a3aa7", "#e34948",
];
pub(super) const SERIES_DARK: [&str; 8] = [
    "#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767",
];

/// What a ninth player is drawn in.
///
/// Achromatic on purpose. Cycling the hues would give two programs the
/// same colour and say they were the same program, which is worse than a
/// colour that says nothing: this one is legible as *not one of the
/// eight*, and the legend, the table and the hover all still name it.
pub(super) const OTHER_LIGHT: &str = "#6d6a62";
pub(super) const OTHER_DARK: &str = "#9a988f";

/// The slot a player is drawn in, or `None` past the eighth.
///
/// Position in the report's own player list, so the assignment is fixed
/// for a given report rather than following whatever survived a filter.
pub(super) fn slot_of(players: &[String], player: &str) -> Option<usize> {
    players
        .iter()
        .position(|p| p == player)
        .filter(|i| *i < SERIES_LIGHT.len())
        .map(|i| i + 1)
}

/// The CSS variable naming a player's colour, in its strong or soft form.
pub(super) fn series_var(players: &[String], player: &str, soft: bool) -> String {
    let suffix = if soft { "-soft" } else { "" };
    match slot_of(players, player) {
        Some(slot) => format!("var(--series-{slot}{suffix})"),
        None => format!("var(--series-other{suffix})"),
    }
}
/// Sequential ramp for magnitude: one hue, light to dark.
///
/// Not green-yellow-red. That is a rainbow for what is a magnitude, and
/// red/green is the worst pair for colour vision deficiency — around 8% of
/// men, which may well include the person the document has to persuade. One
/// hue also survives being printed in greyscale, which a traffic light does
/// not.
pub(super) const RAMP_LIGHT: [&str; 5] = ["#cde2fb", "#9ec5f4", "#5598e7", "#2a78d6", "#184f95"];
pub(super) const RAMP_DARK: [&str; 5] = ["#1c3252", "#21456f", "#256abf", "#3987e5", "#86b6ef"];
/// Ink that stays legible as the ramp darkens.
pub(super) const RAMP_INK: [&str; 5] = ["#0b0b0b", "#0b0b0b", "#ffffff", "#ffffff", "#ffffff"];

pub(super) const PANEL_LIGHT: &str = "#ffffff";
pub(super) const PANEL_DARK: &str = "#232322";
/// `hex` blended `pct` of the way toward `onto`.
///
/// Computed here rather than with `color-mix()` so the value reaches the PDF:
/// the renderer resolves literal hex and little else.
pub(super) fn mix(hex: &str, onto: &str, pct: f64) -> String {
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
pub(super) fn cubic(linear: f64) -> f64 {
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
pub(super) fn tick_ladder() -> &'static str {
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
pub(super) fn tick_opacity() -> (f64, f64) {
    match std::env::var("MEDIA_LOGGER_TICK_OPACITY")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
    {
        Some(v) => (v, v),
        None => (0.60, 0.40),
    }
}

/// A volume as a reader sees it on a slider: a percentage of the way up.
///
/// One decimal, which is what the live checks in the design notes are
/// written to and finer than any slider's own steps, so two levels a
/// listener set apart do not print the same.
pub(super) fn pct(cubic: f64) -> String {
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
pub(super) fn to_cubic(report: &mut Report) {
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
        for v in span.samples_mut() {
            *v = cubic(*v);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    /// Two programs must never be drawn in one colour. Cycling a short
    /// palette does exactly that, and says they were the same program.
    #[test]
    fn a_ninth_player_takes_the_neutral_rather_than_a_repeated_hue() {
        let many: Vec<String> = (0..12).map(|i| format!("player{i}")).collect();
        let used: Vec<String> = many.iter().map(|p| series_var(&many, p, false)).collect();
        // The first eight each get their own slot, in order.
        for (i, v) in used.iter().take(8).enumerate() {
            assert_eq!(*v, format!("var(--series-{})", i + 1));
        }
        // Everything past them shares one achromatic slot, which is a
        // statement that they are not among the eight -- not a claim that
        // they are each other.
        for v in used.iter().skip(8) {
            assert_eq!(*v, "var(--series-other)");
        }
        let distinct: std::collections::HashSet<_> = used.iter().take(8).collect();
        assert_eq!(distinct.len(), 8, "the eight must not repeat");
    }
    /// The soft tint is the same slot as the solid fill, or a bar's extent
    /// and its gain would belong to two different players.
    #[test]
    fn a_player_is_the_same_slot_soft_or_solid() {
        let players = vec!["mpd".to_owned(), "mpv".to_owned()];
        assert_eq!(series_var(&players, "mpv", false), "var(--series-2)");
        assert_eq!(series_var(&players, "mpv", true), "var(--series-2-soft)");
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
}
