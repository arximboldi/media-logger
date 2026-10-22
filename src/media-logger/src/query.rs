//! Window selection and state reconstruction.
//!
//! Every report is the same three steps at a different width: select a
//! window, reconstruct the state in force when it opened, then list what
//! happened inside it. An instant is a zero-width window, which is why
//! "what was playing at 23:14" needs no separate code path.

use std::path::Path;

use anyhow::{bail, Context, Result};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span as JiffSpan, Timestamp, Zoned};

use crate::event::{FullBeat, PlayerState, Playing, Record, Sink};
use crate::log::{self, day_files};

pub struct Window {
    pub from: Zoned,
    pub to: Zoned,
    /// A moment rather than a period: the reconstruction is the answer and
    /// there is nothing to list.
    pub instant: bool,
    pub label: String,
}

/// State in force at a point in time.
#[derive(Default, Clone)]
pub struct Snapshot {
    pub players: Vec<PlayerState>,
    pub sink: Option<Sink>,
    /// Timestamp of the record this came from; `None` when nothing preceded
    /// the window, which is itself worth showing rather than hiding.
    pub as_of: Option<String>,
}

impl Snapshot {
    pub fn playing(&self) -> Vec<&PlayerState> {
        self.players
            .iter()
            .filter(|p| p.status == "Playing")
            .collect()
    }
}

/// Distribution of effective gain over a stretch.
///
/// A single peak hides the shape: a track played quietly with one loud
/// moment and a track played loudly throughout have the same maximum and
/// are not the same complaint.
#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub min: f64,
    pub median: f64,
    pub max: f64,
    pub samples: usize,
}

impl Levels {
    fn from(mut v: Vec<f64>) -> Option<Levels> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = v.len();
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        };
        Some(Levels {
            min: v[0],
            median,
            max: v[n - 1],
            samples: n,
        })
    }
}

/// Effective linear gain: the sink stage multiplied by the player's own.
///
/// The two stages multiply, so neither alone describes the output. This is
/// still software gain — on a device whose level is set by a physical knob
/// it says nothing about loudness in the room.
fn effective(sink: Option<&Sink>, player: &PlayerState) -> Option<f64> {
    let stage = sink?
        .channel_volumes
        .iter()
        .cloned()
        .fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a| a.max(v)))
        })?;
    Some(stage * player.player_volume.unwrap_or(1.0))
}

/// A continuous stretch of one player playing one track.
pub struct Span {
    pub start: Zoned,
    pub end: Zoned,
    pub player: String,
    pub title: Option<String>,
    pub artist: Vec<String>,
    pub url: Option<String>,
    pub sink: Option<String>,
    /// Human name for the same device. The stable node name stays in `sink`
    /// for the data formats; a table wants the one a person can read.
    pub sink_label: Option<String>,
    /// Effective gain over the stretch. Linear, never the cubic figure a
    /// slider shows.
    pub levels: Option<Levels>,
    samples: Vec<f64>,
}

impl Span {
    pub fn label(&self) -> String {
        match (&self.title, self.artist.first()) {
            // Browser titles often already read "Artist — Track"; prefixing
            // the artist again stutters.
            (Some(t), Some(a)) if t.to_lowercase().starts_with(&a.to_lowercase()) => t.clone(),
            (Some(t), Some(a)) => format!("{a} — {t}"),
            (Some(t), None) => t.clone(),
            (None, _) => self
                .url
                .as_deref()
                .and_then(|u| u.rsplit('/').next())
                .unwrap_or("(untitled)")
                .to_owned(),
        }
    }
}

pub struct Report {
    pub host: String,
    /// Effective gain sampled over the window, for the chart's gain lane.
    /// Only while something plays: gain with nothing playing is not a level.
    pub levels: Vec<(Zoned, f64)>,
    pub window: Window,
    pub at_start: Snapshot,
    pub records: Vec<Record>,
    pub spans: Vec<Span>,
    /// Lines whose timestamp could not be read. Surfaced rather than skipped
    /// quietly: a report that silently drops records is worse than one that
    /// says it could not read them.
    pub unreadable: usize,
}

/// Parses `<when>`: an instant, a day, a range, or a shorthand.
pub fn parse_when(spec: &str, now: &Zoned) -> Result<Window> {
    let tz = now.time_zone().clone();
    let spec = spec.trim();

    let day_window = |d: Date, label: String| -> Result<Window> {
        let from = d.to_zoned(tz.clone())?;
        Ok(Window {
            to: from.checked_add(JiffSpan::new().days(1))?,
            from,
            instant: false,
            label,
        })
    };

    match spec.to_ascii_lowercase().as_str() {
        "today" => return day_window(now.date(), "today".to_owned()),
        "yesterday" => {
            let d = now.date().yesterday()?;
            return day_window(d, format!("yesterday ({d})"));
        }
        // The window complaints are actually about.
        "last-night" | "lastnight" => {
            let start = now
                .date()
                .yesterday()?
                .to_zoned(tz.clone())?
                .checked_add(JiffSpan::new().hours(22))?;
            return Ok(Window {
                to: start.checked_add(JiffSpan::new().hours(8))?,
                from: start,
                instant: false,
                label: "last night (22:00–06:00)".to_owned(),
            });
        }
        _ => {}
    }

    if let Some((a, b)) = spec.split_once("..") {
        let from = parse_point(a.trim(), &tz)?;
        let to = match parse_point(b.trim(), &tz) {
            // A bare end date means the whole of that day.
            Ok(end) if b.trim().len() == 10 => end.checked_add(JiffSpan::new().days(1))?,
            Ok(end) => end,
            Err(e) => return Err(e),
        };
        if to < from {
            bail!("range ends before it starts: {spec}");
        }
        return Ok(Window {
            label: format!("{} .. {}", a.trim(), b.trim()),
            from,
            to,
            instant: false,
        });
    }

    if spec.len() == 10 {
        let d: Date = spec
            .parse()
            .with_context(|| format!("not a date: {spec}"))?;
        return day_window(d, spec.to_owned());
    }

    let at = parse_point(spec, &tz)?;
    Ok(Window {
        from: at.clone(),
        to: at,
        instant: true,
        label: spec.to_owned(),
    })
}

fn parse_point(s: &str, tz: &TimeZone) -> Result<Zoned> {
    if s.len() == 10 {
        let d: Date = s.parse().with_context(|| format!("not a date: {s}"))?;
        return Ok(d.to_zoned(tz.clone())?);
    }
    // Accept "2026-10-22T23:14" and "2026-10-22 23:14[:ss]".
    let normalised = s.replacen(' ', "T", 1);
    let padded = match normalised.len() {
        16 => format!("{normalised}:00"),
        _ => normalised,
    };
    let civil: jiff::civil::DateTime = padded
        .parse()
        .with_context(|| format!("not a date or timestamp: {s}"))?;
    Ok(civil.to_zoned(tz.clone())?)
}

/// Builds the report for one window.
pub fn build(host_root: &Path, host: &str, window: Window) -> Result<Report> {
    let mut state = Snapshot::default();
    let mut records = Vec::new();
    // Full beats carry a fingerprint that abbreviated ones refer back to.
    let mut by_fingerprint: std::collections::HashMap<String, FullBeat> =
        std::collections::HashMap::new();
    let mut timeline: Vec<(Zoned, Snapshot)> = Vec::new();
    let mut unreadable = 0usize;
    let tz = window.from.time_zone().clone();
    // Captured as the window opens, before the first record inside it is
    // applied: the opening state is what was already true, not what the
    // window's own first event made true.
    let mut at_start: Option<Snapshot> = None;

    // One day of lead-in is enough to find the state in force: a full beat
    // lands at least once a minute whenever the daemon runs, so only a gap
    // longer than a day leaves the opening state genuinely unknown.
    let lead_in = window.from.checked_sub(JiffSpan::new().days(1))?.date();

    for path in day_files(host_root)? {
        let Some(date) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(date) = date.parse::<Date>() else {
            continue;
        };
        if date < lead_in || date > window.to.date() {
            continue;
        }

        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(record) = serde_json::from_str::<Record>(line) else {
                continue;
            };
            // `ts` is RFC 3339 with an offset, which is a Timestamp; jiff's
            // Zoned demands an IANA annotation the log deliberately omits.
            let Ok(instant) = record.ts.parse::<Timestamp>() else {
                unreadable += 1;
                continue;
            };
            let ts = instant.to_zoned(tz.clone());
            if ts > window.to {
                break;
            }

            if ts >= window.from && at_start.is_none() {
                at_start = Some(state.clone());
            }
            apply(&record, &mut state, &mut by_fingerprint);
            if ts >= window.from {
                timeline.push((ts.clone(), state.clone()));
                if !window.instant {
                    records.push(record);
                }
            } else {
                // Only records *preceding* the window establish the opening
                // state, so only they advance its as-of time.
                state.as_of = Some(record.ts.clone());
            }
        }
    }

    let at_start = match (window.instant, at_start) {
        // For an instant the reconstruction *is* the answer, so it includes
        // everything up to and including that moment.
        (true, _) => state.clone(),
        (false, Some(opening)) => opening,
        (false, None) => state.clone(),
    };

    Ok(Report {
        host: host.to_owned(),
        levels: levels_from(&timeline),
        spans: spans_from(&timeline),
        at_start,
        records,
        window,
        unreadable,
    })
}

/// Folds one record into the running state.
fn apply(
    record: &Record,
    state: &mut Snapshot,
    by_fingerprint: &mut std::collections::HashMap<String, FullBeat>,
) {
    match record.kind.as_str() {
        "heartbeat" => {
            if let Some(key) = record.data.get("unchanged").and_then(|v| v.as_str()) {
                if let Some(full) = by_fingerprint.get(key) {
                    state.players = full.players.clone();
                    state.sink = full.sink.clone();
                }
            } else if let Ok(full) = serde_json::from_value::<FullBeat>(record.data.clone()) {
                by_fingerprint.insert(full.state.clone(), full.clone());
                state.players = full.players;
                state.sink = full.sink;
            }
        }
        "playback" => {
            if let Ok(playing) = serde_json::from_value::<Playing>(record.data.clone()) {
                state.players.retain(|p| p.player != playing.state.player);
                state.players.push(playing.state);
                state.players.sort_by(|a, b| a.player.cmp(&b.player));
                if playing.sink.is_some() {
                    state.sink = playing.sink;
                }
            }
        }
        "player_gone" => {
            if let Some(name) = record.data.get("player").and_then(|v| v.as_str()) {
                state.players.retain(|p| p.player != name);
            }
        }
        "sink_changed" => {
            if let Ok(sink) =
                serde_json::from_value::<Sink>(record.data.get("sink").cloned().unwrap_or_default())
            {
                state.sink = Some(sink);
            }
        }
        // daemon_stop means nothing is playing any more, which matters: it is
        // what turns a gap into a positive statement about silence.
        "daemon_stop" => state.players.clear(),
        _ => {}
    }
}

/// Collapses the per-record timeline into stretches of continuous playback.
fn spans_from(timeline: &[(Zoned, Snapshot)]) -> Vec<Span> {
    let mut open: Vec<Span> = Vec::new();
    let mut done: Vec<Span> = Vec::new();

    for (ts, state) in timeline {
        let mut still: Vec<(String, String)> = Vec::new();
        for player in state.playing() {
            let key = track_key(player);
            still.push((player.player.clone(), key.clone()));

            match open
                .iter_mut()
                .find(|s| s.player == player.player && track_key_of(s) == key)
            {
                Some(span) => {
                    span.end = ts.clone();
                    span.samples.extend(effective(state.sink.as_ref(), player));
                }
                None => open.push(Span {
                    start: ts.clone(),
                    end: ts.clone(),
                    player: player.player.clone(),
                    title: player.track.title.clone(),
                    artist: player.track.artist.clone(),
                    url: player.track.url.clone(),
                    sink: state.sink.as_ref().map(|s| s.name.clone()),
                    sink_label: state
                        .sink
                        .as_ref()
                        .map(|s| s.description.clone().unwrap_or_else(|| s.name.clone())),
                    levels: None,
                    samples: effective(state.sink.as_ref(), player).into_iter().collect(),
                }),
            }
        }

        // Anything no longer playing has ended.
        let (keep, ended): (Vec<Span>, Vec<Span>) = open.into_iter().partition(|s| {
            still
                .iter()
                .any(|(p, k)| *p == s.player && *k == track_key_of(s))
        });
        open = keep;
        done.extend(ended);
    }

    done.extend(open);
    for span in &mut done {
        span.levels = Levels::from(std::mem::take(&mut span.samples));
    }
    done.sort_by(|a, b| a.start.cmp(&b.start));
    done
}

/// Loudest effective gain at each sampled moment, for the chart's gain lane.
fn levels_from(timeline: &[(Zoned, Snapshot)]) -> Vec<(Zoned, f64)> {
    timeline
        .iter()
        .filter_map(|(ts, state)| {
            state
                .playing()
                .iter()
                .filter_map(|p| effective(state.sink.as_ref(), p))
                .fold(None, |acc: Option<f64>, v| {
                    Some(acc.map_or(v, |a: f64| a.max(v)))
                })
                .map(|v| (ts.clone(), v))
        })
        .collect()
}

fn track_key(p: &PlayerState) -> String {
    p.track
        .url
        .clone()
        .or_else(|| p.track.trackid.clone())
        .or_else(|| p.track.title.clone())
        .unwrap_or_default()
}

fn track_key_of(s: &Span) -> String {
    s.url
        .clone()
        .or_else(|| s.title.clone())
        .unwrap_or_default()
}

/// Chain and anchor status, for the footer every rendered report carries.
pub struct Integrity {
    pub chain_ok: bool,
    pub lines: u64,
    pub problems: Vec<String>,
    pub anchors: Vec<(String, String)>,
}

pub fn integrity(host_root: &Path, window: &Window) -> Result<Integrity> {
    let report = log::verify(host_root)?;
    let mut anchors = Vec::new();

    for path in day_files(host_root)? {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(date) = name.parse::<Date>() else {
            continue;
        };
        if date < window.from.date() || date > window.to.date() {
            continue;
        }
        let states: Vec<String> = crate::anchor::check(&path)
            .into_iter()
            .map(|(tsa, state)| match state {
                crate::anchor::TokenState::Valid { at } => format!("{} verified {at}", tsa.name),
                crate::anchor::TokenState::Missing => format!("{} not yet stamped", tsa.name),
                crate::anchor::TokenState::Invalid(why) => format!("{} INVALID: {why}", tsa.name),
            })
            .collect();
        anchors.push((name.to_owned(), states.join(", ")));
    }

    Ok(Integrity {
        chain_ok: report.ok(),
        lines: report.lines,
        problems: report.problems,
        anchors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Beat, Event, FullBeat, PlayerState, Track};
    use crate::log::Log;

    fn now() -> Zoned {
        jiff::civil::date(2026, 10, 20)
            .at(12, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap()
    }

    fn playing(title: &str) -> PlayerState {
        PlayerState {
            player: "mpd".to_owned(),
            identity: None,
            status: "Playing".to_owned(),
            player_volume: Some(1.0),
            position_us: Some(0),
            track: Track {
                title: Some(title.to_owned()),
                url: Some(format!("/music/{title}.flac")),
                ..Track::default()
            },
        }
    }

    /// Writes heartbeats at `hours` o'clock on 2026-10-20.
    fn tree(hours: &[i8], title: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let first = jiff::civil::date(2026, 10, 20)
            .at(0, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap();
        let (mut log, _) = Log::resume(dir.path(), "h", &first).unwrap();
        for h in hours {
            let at = jiff::civil::date(2026, 10, 20)
                .at(*h, 0, 0, 0)
                .to_zoned(TimeZone::UTC)
                .unwrap();
            log.append(
                &Event::Heartbeat(Beat::Full(Box::new(FullBeat {
                    players: vec![playing(title)],
                    sink: None,
                    state: format!("fp-{title}"),
                }))),
                &at,
            )
            .unwrap();
        }
        dir
    }

    /// The log writes RFC 3339 with an offset, which is a Timestamp; jiff's
    /// Zoned refuses it for want of an IANA annotation. Parsing it as Zoned
    /// made every record silently unreadable and every report empty, with no
    /// error anywhere — so this is the regression that matters most.
    #[test]
    fn records_with_offset_timestamps_are_read() {
        let dir = tree(&[21, 22, 23], "Track");
        let window = parse_when("2026-10-20", &now()).unwrap();
        let report = build(&dir.path().join("h"), "h", window).unwrap();

        assert_eq!(report.unreadable, 0, "no record should be unreadable");
        assert!(!report.spans.is_empty(), "playback must be detected");
    }

    #[test]
    fn a_continuous_track_is_one_span() {
        let dir = tree(&[21, 22, 23], "Track");
        let window = parse_when("2026-10-20", &now()).unwrap();
        let report = build(&dir.path().join("h"), "h", window).unwrap();

        assert_eq!(report.spans.len(), 1);
        assert_eq!(report.spans[0].start.hour(), 21);
        assert_eq!(report.spans[0].end.hour(), 23);
    }

    /// The opening state must come from before the window, not from the
    /// window's own first event. Taking it from after made a day report
    /// claim, at 00:00, a state that only became true that afternoon.
    #[test]
    fn opening_state_precedes_the_window() {
        let dir = tree(&[9, 15, 20], "Track");
        let window = parse_when("2026-10-20T16:00", &now()).unwrap();
        let instant = build(&dir.path().join("h"), "h", window).unwrap();
        // An instant sees everything up to and including itself.
        assert!(instant.at_start.as_of.is_some());

        let window = parse_when("2026-10-20", &now()).unwrap();
        let day = build(&dir.path().join("h"), "h", window).unwrap();
        // Nothing precedes the day, so the opening state is empty rather
        // than borrowed from 09:00.
        assert!(
            day.at_start.players.is_empty(),
            "opening state must not be taken from inside the window"
        );
    }

    #[test]
    fn window_forms_parse() {
        let n = now();
        assert!(!parse_when("2026-10-20", &n).unwrap().instant);
        assert!(parse_when("2026-10-20T23:14", &n).unwrap().instant);
        let range = parse_when("2026-10-01..2026-10-31", &n).unwrap();
        assert_eq!(range.from.date().day(), 1);
        // A bare end date means the whole of that day, not its midnight.
        assert_eq!(range.to.date().day(), 1);
        assert_eq!(range.to.date().month(), 11);
        assert!(parse_when("yesterday", &n).is_ok());
        assert!(parse_when("last-night", &n).is_ok());
        assert!(parse_when("not-a-date", &n).is_err());
    }
}
