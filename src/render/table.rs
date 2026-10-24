//! The spans table, and the two encodings that carry it.
//!
//! csv and xlsx are the same table in two encodings, so the columns and the
//! values are defined once here and each renderer only decides how to write
//! them. They were written twice before and drifted: the xlsx kept a "Peak
//! volume" header after the data underneath had become min/median/max.
//!
//! The document formats (text, html, pdf) are not this table -- they group
//! and stack columns for a printed page -- so they stay separate on purpose.

use super::*;

pub struct Column {
    /// Machine name, used as the csv header.
    key: &'static str,
    /// Human name, used as the spreadsheet header.
    title: &'static str,
    width: f64,
}

/// A value, not a rendering: each format decides how to write it.
pub(super) enum Cell {
    Text(String),
    Num(f64),
    Count(u64),
    /// Seconds. Raw in csv for analysis, human in xlsx for reading.
    Duration(i64),
    /// Seconds inside quiet hours; zero means none, and is flagged when not.
    Quiet(i64),
    Empty,
}

pub(super) const COLUMNS: &[Column] = &[
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

pub(super) fn rows(report: &Report, quiet: &QuietHours) -> Vec<Vec<Cell>> {
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

pub(super) fn csv(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
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

pub(super) fn xlsx(
    report: &Report,
    integrity: &Integrity,
    quiet: &QuietHours,
    path: &Path,
) -> Result<()> {
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
