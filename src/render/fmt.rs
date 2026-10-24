//! How a value is written down.
//!
//! Durations, clock times, dates, and the escaping that makes a track
//! title safe inside a tag. Together because a report that spells one
//! quantity two ways reads as two measurements -- which is also why
//! `human_secs` has a twin in the filter script, named in its doc and
//! pinned by the test below.

use super::*;

/// A date as a reader parses it, rather than as a machine sorts one.
///
/// Day first: between two rows of the same month the day is the field that
/// differs, and it should be the one the eye lands on. ISO stays wherever
/// the order is taken from the text itself -- the `data-` attributes the
/// filters group by, the two exports, the JSON -- because there the leading
/// year is the sort.
pub(super) fn human_date(d: jiff::civil::Date) -> String {
    format!("{:02}-{:02}-{:04}", d.day(), d.month(), d.year())
}

/// The same, for a date already written ISO.
///
/// Anything that is not one passes through: the integrity table's days come
/// from the names of files in the tree, and a name that is not a date is a
/// fact about the tree worth showing exactly as it is.
pub(super) fn human_iso(s: &str) -> String {
    match *s.split('-').collect::<Vec<_>>() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => format!("{d}-{m}-{y}"),
        _ => s.to_owned(),
    }
}
/// Whole seconds between two instants.
///
/// `Span::get_seconds` returns only the seconds *component* of a calendar
/// span, so a 40 minute stretch reads as zero. Totalling in seconds is what
/// is actually meant.
pub fn secs_between(a: &Zoned, b: &Zoned) -> i64 {
    (b - a).total(jiff::Unit::Second).unwrap_or(0.0).round() as i64
}

/// Sub-second resolution, which a gap needs and a duration does not.
pub(super) fn millis_between(a: &Zoned, b: &Zoned) -> i64 {
    (b - a)
        .total(jiff::Unit::Millisecond)
        .unwrap_or(0.0)
        .round() as i64
}

/// Seconds, as a figure to read.
///
/// Has a twin in the filter script, which rewrites the quiet-hours total
/// as the table narrows. The format must not change under a filter, so a
/// change here is a change there; `a_figure_reads_the_same_either_side`
/// pins the cases that twin was written against.
pub(super) fn human_secs(secs: i64) -> String {
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

pub(super) fn hhmmss(z: &Zoned) -> String {
    z.strftime("%H:%M:%S").to_string()
}

/// Times carry their date once a window spans more than one day.
///
/// A bare "21:10:00" in a three-day table says nothing about which day, which
/// is exactly the ambiguity a noise complaint turns on.
pub(super) fn clock(z: &Zoned, multiday: bool) -> String {
    if multiday {
        z.strftime("%d-%m %H:%M:%S").to_string()
    } else {
        z.strftime("%H:%M:%S").to_string()
    }
}

pub(super) fn multiday(report: &Report) -> bool {
    days_in(report).len() > 1
}

/// RFC 3339, matching the log's own `ts` field.
///
/// Not jiff's `Display`, which appends the IANA annotation
/// (`...+02:00[Europe/Berlin]`) -- correct but something spreadsheets and
/// dataframe libraries refuse to parse, which defeats the point of csv and
/// xlsx existing at all.
pub(super) fn rfc3339(z: &Zoned) -> String {
    z.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}
pub(super) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quiet-hours total is printed by Rust and rewritten by the
    /// filter script, so the two formatters have to agree to the
    /// character. These are the cases the script's twin was written
    /// against; a change here that is not mirrored there shows up as a
    /// figure that changes shape the moment a reader narrows the table.
    #[test]
    fn a_figure_reads_the_same_either_side() {
        assert_eq!(human_secs(0), "0s");
        assert_eq!(human_secs(9), "9s");
        assert_eq!(human_secs(59), "59s");
        assert_eq!(human_secs(60), "1m00s");
        assert_eq!(human_secs(520), "8m40s");
        assert_eq!(human_secs(3599), "59m59s");
        assert_eq!(human_secs(3600), "1h00m");
        assert_eq!(human_secs(3660), "1h01m");
        assert_eq!(human_secs(47 * 3600 + 3599), "47h59m");
        assert_eq!(human_secs(48 * 3600), "2d00h");
        assert_eq!(human_secs(49 * 3600), "2d01h");
    }
}
