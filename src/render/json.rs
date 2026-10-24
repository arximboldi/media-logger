//! The report as one json document, for anything that reads reports.

use super::*;

pub(super) fn json(report: &Report, integrity: &Integrity, quiet: &QuietHours) -> Result<String> {
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
