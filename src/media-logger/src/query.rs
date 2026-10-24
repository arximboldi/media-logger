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

use crate::event::{FullBeat, PlayerState, Playing, Record, Sink, Stream, Track};
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

/// Effective linear gain: every stage multiplied together.
///
/// Three of them — the sink's node, the sink's device route, and the
/// player's own. `Sink::peak` folds the first two; this adds the third. None
/// alone describes the output, and reading only the node recorded a constant
/// on every device with a hardware mixer.
///
/// Still software gain: on a device whose level is set by a physical knob it
/// says nothing about loudness in the room.
fn effective(sink: Option<&Sink>, player: &PlayerState) -> Option<f64> {
    Some(sink?.peak()? * player.player_volume.unwrap_or(1.0))
}

/// A continuous stretch of one player playing one track.
pub struct Span {
    pub start: Zoned,
    pub end: Zoned,
    /// Which machine played it. Two hosts playing at once are two spans,
    /// because that is two sources of sound.
    pub host: String,
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
    /// Loudest level this stretch held for five minutes, by the same rule
    /// the calendar uses. Carried per span so a filtered page can recolour
    /// the calendar from what survived, rather than from the whole day.
    pub sustained: Option<f64>,
    /// The track identity this stretch was opened with, so a later sample
    /// is compared against the same value that `track_key` produces rather
    /// than against a second reading of the fields kept above.
    key: String,
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
    /// Every host the window covers. A report naming one machine while a
    /// second played is the omission this exists to prevent.
    pub hosts: Vec<String>,
    /// Effective gain sampled over the window, for the chart's gain lane.
    /// Only while something plays: gain with nothing playing is not a level.
    pub levels: Vec<(String, Zoned, f64)>,
    pub window: Window,
    /// The state in force when the window opened, per host.
    pub at_start: Vec<(String, Snapshot)>,
    pub records: Vec<Record>,
    pub spans: Vec<Span>,
    /// When each host was demonstrably running, within the window.
    pub coverage: Vec<Coverage>,
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
        let tail = b.trim();
        let end = parse_point(tail, &tz)?;
        // A bare day or month at the end means the whole of it.
        let to = if month_start(tail).is_some() {
            end.checked_add(JiffSpan::new().months(1))?
        } else if tail.len() == 10 {
            end.checked_add(JiffSpan::new().days(1))?
        } else {
            end
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

    // A whole month: 2026-10.
    if let Some(first) = month_start(spec) {
        let from = first.to_zoned(tz.clone())?;
        return Ok(Window {
            to: from.checked_add(JiffSpan::new().months(1))?,
            label: first.strftime("%B %Y").to_string(),
            from,
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

/// `2026-10` as the first of that month, or nothing.
fn month_start(spec: &str) -> Option<Date> {
    let (year, month) = spec.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    Some(jiff::civil::date(
        year.parse().ok()?,
        month.parse().ok()?,
        1,
    ))
}

fn parse_point(s: &str, tz: &TimeZone) -> Result<Zoned> {
    if let Some(first) = month_start(s) {
        return Ok(first.to_zoned(tz.clone())?);
    }
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

/// Identity of a state for change detection.
///
/// Deliberately excludes track position, which advances continuously: folding
/// it in would mean no two beats ever matched, defeating abbreviation exactly
/// while a track plays and lines are at their largest.
pub fn fingerprint(players: &[PlayerState], streams: &[Stream], sink: &Option<Sink>) -> String {
    let key: Vec<_> = players
        .iter()
        .map(|p| {
            serde_json::json!({
                "player": p.player,
                "status": p.status,
                "volume": p.player_volume,
                "url": p.track.url,
                "trackid": p.track.trackid,
                "title": p.track.title,
            })
        })
        .collect();
    // Identity only, as for a player: what is sending audio is the state,
    // and a stream's own volume moving would break a run of beats that are
    // otherwise saying the same thing.
    let heard: Vec<_> = streams
        .iter()
        .map(|s| serde_json::json!({ "app": s.app, "pid": s.pid, "media": s.media }))
        .collect();
    let state = serde_json::json!({ "players": key, "streams": heard, "sink": sink });
    crate::log::hash_line(&state.to_string())[..16].to_owned()
}

/// The sample times one record stands for.
///
/// A collapsed heartbeat is a single line covering a run of identical
/// samples. Treating it as one point leaves the minutes it covers with no
/// sample at all — holes in the gain profile, and level statistics weighted
/// toward whatever happened to be changing. Expanding it back to the samples
/// it reports is not invention: the line asserts that state held across that
/// span, which is precisely what is being restored.
fn sample_times(record: &Record, at: &Zoned, tz: &TimeZone) -> Vec<Zoned> {
    if record.kind != "heartbeat" {
        return vec![at.clone()];
    }
    let beats = record
        .data
        .get("beats")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
        // A log claiming an absurd count must not cost unbounded memory.
        .min(4096);
    let since = record
        .data
        .get("since")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if beats < 2 || since.is_empty() {
        return vec![at.clone()];
    }
    let Ok(start) = since.parse::<Timestamp>() else {
        return vec![at.clone()];
    };

    let start = start.to_zoned(tz.clone());
    let Ok(total) = (at - &start).total(jiff::Unit::Second) else {
        return vec![at.clone()];
    };
    if total <= 0.0 {
        return vec![at.clone()];
    }

    let step = total / (beats - 1) as f64;
    (0..beats)
        .filter_map(|i| {
            start
                .checked_add(JiffSpan::new().milliseconds((step * i as f64 * 1000.0) as i64))
                .ok()
        })
        .collect()
}

/// Builds the report for one window.
/// When a host's daemon was demonstrably running.
///
/// The whole point of a heartbeat is that silence in the log means silence
/// in the room. That only holds while the daemon is up: a day with no
/// playback because nothing played and a day with no playback because
/// nothing was recorded look identical in a report that does not track
/// this, and only one of them is evidence.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    pub host: String,
    /// Merged and in order, each the stretch between two beats close enough
    /// together that the daemon cannot have been down in between.
    pub up: Vec<(Zoned, Zoned)>,
    /// Longest gap between two level samples that is not a break in the
    /// playing. A sample lands every beat while something plays, so two
    /// further apart than this mean the playing stopped -- not that the
    /// level was held through the silence. The same tolerance the beats
    /// are merged with, and taken from the log for the same reason.
    pub gap: i64,
}

impl Coverage {
    /// Seconds of `[from, to)` the daemon was running for.
    pub fn secs(&self, from: &Zoned, to: &Zoned) -> i64 {
        self.up
            .iter()
            .map(|(a, b)| {
                let lo = a.max(from);
                let hi = b.min(to);
                if hi > lo {
                    (hi - lo).total(jiff::Unit::Second).unwrap_or(0.0) as i64
                } else {
                    0
                }
            })
            .sum()
    }

    pub fn first(&self) -> Option<&Zoned> {
        self.up.first().map(|(a, _)| a)
    }

    /// Was the daemon running at this moment?
    pub fn covers(&self, at: &Zoned) -> bool {
        self.up.iter().any(|(a, b)| a <= at && at <= b)
    }
}

/// Beats closer together than this are one unbroken stretch of running.
///
/// Taken from the log rather than assumed: a collapsed heartbeat says
/// exactly how often this host beats, and three missed beats is a gap. A log
/// with no collapsed run to learn from falls back to three minutes, which is
/// longer than any sensible heartbeat.
const GAP_FALLBACK: f64 = 180.0;

/// Three beats, which is what `merge_beats` calls an unbroken stretch.
fn beat_gap(period: Option<f64>) -> i64 {
    let step = period.unwrap_or(GAP_FALLBACK / 3.0);
    (3.0 * step).clamp(60.0, 3600.0) as i64
}

fn merge_beats(mut points: Vec<Zoned>, period: Option<f64>) -> Vec<(Zoned, Zoned)> {
    let step = period.unwrap_or(GAP_FALLBACK / 3.0);
    let tolerance = (3.0 * step).clamp(60.0, 3600.0);
    points.sort();
    let mut out: Vec<(Zoned, Zoned)> = Vec::new();
    for p in points {
        // A beat stands for the interval it samples -- that is what makes it
        // a heartbeat rather than a timestamp -- so it covers until the next
        // one is due, and consecutive beats join into one unbroken stretch.
        let Ok(until) = p.checked_add(JiffSpan::new().milliseconds((step * 1000.0) as i64)) else {
            continue;
        };
        match out.last_mut() {
            Some((_, end))
                if (&p - &*end).total(jiff::Unit::Second).unwrap_or(f64::MAX) <= tolerance =>
            {
                if until > *end {
                    *end = until;
                }
            }
            _ => out.push((p, until)),
        }
    }
    out
}

/// Builds the report for one window, across every host given.
pub fn build(logs_root: &Path, hosts: &[String], window: Window) -> Result<Report> {
    let mut report = Report {
        hosts: hosts.to_vec(),
        levels: Vec::new(),
        at_start: Vec::new(),
        records: Vec::new(),
        spans: Vec::new(),
        coverage: Vec::new(),
        unreadable: 0,
        window,
    };

    for host in hosts {
        let one = build_host(&logs_root.join(host), host, &report.window)?;
        report.at_start.push((host.clone(), one.at_start));
        report.coverage.push(one.coverage);
        report.records.extend(one.records);
        report.spans.extend(one.spans);
        report.levels.extend(one.levels);
        report.unreadable += one.unreadable;
    }

    // Each span's own sustained level, so the calendar can be recomputed
    // from whichever spans a filter leaves behind.
    for i in 0..report.spans.len() {
        let (host, from, to) = {
            let s = &report.spans[i];
            (s.host.clone(), s.start.clone(), s.end.clone())
        };
        // The host's whole timeline, not the span's slice of it: the
        // stretch of playing this track sat in is what the figure is
        // about, and it reaches past both ends of the track.
        let gap = report
            .coverage
            .iter()
            .find(|c| c.host == host)
            .map(|c| c.gap)
            .unwrap_or_else(|| beat_gap(None));
        let mut all: Vec<(i64, f64)> = report
            .levels
            .iter()
            .filter(|(h, _, _)| *h == host)
            .map(|(_, ts, v)| {
                (
                    (ts - &from).total(jiff::Unit::Second).unwrap_or(0.0) as i64,
                    *v,
                )
            })
            .collect();
        all.sort_by_key(|(at, _)| *at);
        let span_secs = (&to - &from).total(jiff::Unit::Second).unwrap_or(0.0) as i64;
        let run = run_around(&all, 0, span_secs, gap);
        report.spans[i].sustained = sustained_max(&run, gap);
    }

    // Merged, so the reader sees one chronology rather than one per machine.
    report.records.sort_by(|a, b| a.ts.cmp(&b.ts));
    report.spans.sort_by(|a, b| a.start.cmp(&b.start));
    report.levels.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(report)
}

struct HostReport {
    at_start: Snapshot,
    coverage: Coverage,
    records: Vec<Record>,
    spans: Vec<Span>,
    levels: Vec<(String, Zoned, f64)>,
    unreadable: usize,
}

/// Reconstructs one host. State is per machine and never merged: two hosts
/// have independent players, sinks and chains.
fn build_host(host_root: &Path, host: &str, window: &Window) -> Result<HostReport> {
    let mut state = Snapshot::default();
    let mut records = Vec::new();
    // Full beats carry a fingerprint that abbreviated ones refer back to.
    let mut by_fingerprint: std::collections::HashMap<String, FullBeat> =
        std::collections::HashMap::new();
    let mut timeline: Vec<(Zoned, Snapshot)> = Vec::new();
    // Every moment the daemon is known to have been running, and how often
    // it beats -- which the log states itself, in any collapsed heartbeat.
    let mut beats: Vec<Zoned> = Vec::new();
    let mut period: Option<f64> = None;
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
            // A line stamped after the window can still be *about* it: the
            // open run's heartbeat is rewritten in place, so while a session
            // is under way the one line standing for every beat since it
            // began carries the time of the latest. Stopping at the stamp
            // threw the whole run away, and a window ending inside it came
            // back empty -- "nothing was playing" about a track that was.
            let past = ts > window.to;

            if !past && ts >= window.from && at_start.is_none() {
                at_start = Some(state.clone());
            }
            apply(&record, &mut state, &mut by_fingerprint);
            // One line, but possibly many moments: a collapsed heartbeat
            // stands for every beat it replaced, and each is proof the
            // daemon was alive then.
            let samples = sample_times(&record, &ts, &tz);
            if let [a, b, ..] = samples.as_slice() {
                let step = (b - a).total(jiff::Unit::Second).unwrap_or(0.0);
                if step > 0.0 {
                    period = Some(period.map_or(step, |p: f64| p.max(step)));
                }
            }
            // The lead-in counts too: whether the daemon was running at the
            // moment a window opens is answered by the beat before it.
            for when in &samples {
                if *when <= window.to {
                    beats.push(when.clone());
                }
            }

            if ts >= window.from || past {
                for when in samples {
                    if when >= window.from && when <= window.to {
                        timeline.push((when, state.clone()));
                    }
                }
                // The record itself lies outside the window even where the
                // beats it stands for do not.
                if !window.instant && !past {
                    records.push(record);
                }
            } else {
                // Only records *preceding* the window establish the opening
                // state, so only they advance its as-of time.
                state.as_of = Some(record.ts.clone());
            }

            if past {
                break;
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

    Ok(HostReport {
        coverage: Coverage {
            host: host.to_owned(),
            gap: beat_gap(period),
            up: merge_beats(beats, period),
        },
        spans: spans_from(&timeline, host),
        levels: levels_from(&timeline, host),
        at_start,
        records,
        unreadable,
    })
}

/// The name a report gives a program that was not on the bus.
///
/// One name for all of them rather than one per binary: the filter row
/// would otherwise grow an entry for every game and utility that ever made
/// a sound, and the colour slots run out at three. Which program it was is
/// the media, where a player's track title goes -- which is the truth of
/// it, since the program is all PipeWire could tell us.
pub const GENERIC: &str = "generic";

/// Everything that was sounding: the players the bus named, and the
/// programs only PipeWire saw.
///
/// The merge happens here, at the boundary where a log becomes a report,
/// and not in the log itself. The record keeps the two apart because they
/// are different evidence from different sources; a reader asking what was
/// playing wants one answer.
fn heard(players: &[PlayerState], streams: &[Stream]) -> Vec<PlayerState> {
    let mut out = players.to_vec();
    for s in streams {
        out.push(PlayerState {
            player: GENERIC.to_owned(),
            identity: Some(s.app.clone()),
            status: "Playing".to_owned(),
            // The stream's own gain is recorded in the log and deliberately
            // not used here: the effective level is the sink's, and this
            // log has already paid once for multiplying two gain stages.
            player_volume: None,
            position_us: None,
            track: Track {
                title: Some(s.app.clone()),
                album: s.media.clone().filter(|m| *m != s.app),
                url: s.binary.clone(),
                ..Track::default()
            },
        });
    }
    out
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
                    state.players = heard(&full.players, &full.streams);
                    state.sink = full.sink.clone();
                }
            } else if let Ok(full) = serde_json::from_value::<FullBeat>(record.data.clone()) {
                by_fingerprint.insert(full.state.clone(), full.clone());
                state.players = heard(&full.players, &full.streams);
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

/// A player's name without the instance it happens to be running as.
///
/// MPRIS lets a program own `org.mpris.MediaPlayer2.firefox.instance_1_181`,
/// where everything after the program is a token for that particular
/// process. Kept, it would split one browser into as many players as it had
/// sessions -- a filter list growing a line per restart, a new colour in the
/// timeline each time, and `--player firefox` matching none of them. The log
/// keeps the full name; reports are about the program.
pub fn player_name(raw: &str) -> &str {
    raw.split('.').next().unwrap_or(raw)
}

/// Collapses the per-record timeline into stretches of continuous playback.
fn spans_from(timeline: &[(Zoned, Snapshot)], host: &str) -> Vec<Span> {
    let mut open: Vec<Span> = Vec::new();
    let mut done: Vec<Span> = Vec::new();

    for (ts, state) in timeline {
        let mut still: Vec<(String, String)> = Vec::new();
        for player in state.playing() {
            let key = track_key(player);
            let name = player_name(&player.player).to_owned();
            still.push((name.clone(), key.clone()));

            match open.iter_mut().find(|s| s.player == name && s.key == key) {
                Some(span) => {
                    span.end = ts.clone();
                    span.samples.extend(effective(state.sink.as_ref(), player));
                }
                None => open.push(Span {
                    start: ts.clone(),
                    end: ts.clone(),
                    host: host.to_owned(),
                    player: name,
                    title: player.track.title.clone(),
                    artist: player.track.artist.clone(),
                    url: player.track.url.clone(),
                    sink: state.sink.as_ref().map(|s| s.name.clone()),
                    sink_label: state
                        .sink
                        .as_ref()
                        .map(|s| s.description.clone().unwrap_or_else(|| s.name.clone())),
                    levels: None,
                    sustained: None,
                    key: key.clone(),
                    samples: effective(state.sink.as_ref(), player).into_iter().collect(),
                }),
            }
        }

        // Anything no longer playing has ended.
        let (keep, ended): (Vec<Span>, Vec<Span>) = open
            .into_iter()
            .partition(|s| still.iter().any(|(p, k)| *p == s.player && *k == s.key));
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
fn levels_from(timeline: &[(Zoned, Snapshot)], host: &str) -> Vec<(String, Zoned, f64)> {
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
                .map(|v| (host.to_owned(), ts.clone(), v))
        })
        .collect()
}

/// Identity of what is playing: every field the player offered, together.
///
/// No single field will do, and the first one present is not the strongest.
/// Only mpd sends a `url`. Firefox and Telegram reuse one `trackid` for
/// every track they ever play -- `/org/mpris/MediaPlayer2/firefox` -- so
/// keying on it alone would fold a whole browsing session into one stretch.
/// A `title` is what a private-browsing window refuses to fill in, leaving
/// a constant "Zen is playing media" that only the trackid and the title
/// *together* distinguish from a real track.
///
/// Comparing the whole identity continues a stretch exactly while nothing
/// about the track changed, which is the question being asked. The fields
/// are joined on a byte no metadata contains, so a value ending where the
/// next begins cannot forge a match.
fn track_key(p: &PlayerState) -> String {
    [
        p.track.url.as_deref(),
        p.track.trackid.as_deref(),
        p.track.title.as_deref(),
    ]
    .map(Option::unwrap_or_default)
    .join("\u{1}")
}

/// What one day looked like, for the calendar.
pub struct DaySummary {
    pub date: Date,
    /// Loudest level held continuously for at least `SUSTAINED`.
    ///
    /// Not the peak: thirty seconds at full volume while the knob is being
    /// turned back down is not what anyone is complaining about, and a peak
    /// cannot tell that from an evening held at the same level. A sustained
    /// maximum cannot be moved by the brief one at all.
    pub sustained: Option<f64>,
    pub peak: Option<f64>,
    pub playing_secs: i64,
    pub quiet_secs: i64,
    /// Of the part of this day the window covers, how much of it the daemon
    /// was running for, and how long that part is.
    pub recorded_secs: i64,
    pub window_secs: i64,
}

/// How much of a day the log can speak for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// Nothing at all: the day is blank because there is no record of it,
    /// which is not the same as nothing having played.
    None,
    Partial,
    Full,
}

impl DaySummary {
    pub fn recorded(&self) -> Recorded {
        // The same slack at both ends: a few minutes short of the whole day
        // is a complete record, and a few minutes of it is none. The minutes
        // at the bottom are a daemon stopping as the day turned over, and
        // calling that day partly recorded would hide, among the days that
        // genuinely are, the ones where nobody was watching at all.
        if self.recorded_secs <= MISSABLE {
            Recorded::None
        } else if self.recorded_secs + MISSABLE >= self.window_secs {
            Recorded::Full
        } else {
            Recorded::Partial
        }
    }
}

/// Slack before a day counts as only partly recorded.
///
/// A restart, a suspend of a few seconds, the gap between the last beat of
/// one run and the first of the next: real, logged, and not worth marking a
/// day incomplete over. Five minutes is also the window a sustained level
/// has to hold, so nothing loud enough to matter fits inside it.
const MISSABLE: i64 = 300;

/// How long a level must hold to count toward the sustained maximum.
const SUSTAINED: i64 = 300;

/// Per-day summaries across the report's window.
pub fn days(report: &Report, quiet: &crate::quiet::QuietHours) -> Result<Vec<DaySummary>> {
    let tz = report.window.from.time_zone().clone();
    let mut out = Vec::new();
    let mut date = report.window.from.date();
    let last = report.window.to.date();

    while date <= last && out.len() < 3700 {
        let start = date.to_zoned(tz.clone())?;
        let end = start.checked_add(JiffSpan::new().days(1))?;
        // A window ending at midnight names the next day without covering
        // any of it. Summarising that day would invent an unrecorded one at
        // the end of every report.
        if start >= report.window.to && report.window.to > report.window.from {
            break;
        }

        // Per host, then the loudest of them. A level is held by a
        // machine: pooling two machines' samples into one series lets each
        // fill the other's silences and manufactures a stretch neither of
        // them played, which is the same bridging a gap commits.
        let sustained = report
            .coverage
            .iter()
            .filter_map(|c| {
                let mut levels: Vec<(i64, f64)> = report
                    .levels
                    .iter()
                    .filter(|(h, ts, _)| *h == c.host && *ts >= start && *ts < end)
                    .map(|(_, ts, v)| {
                        (
                            (ts - &start).total(jiff::Unit::Second).unwrap_or(0.0) as i64,
                            *v,
                        )
                    })
                    .collect();
                levels.sort_by_key(|(at, _)| *at);
                sustained_max(&levels, c.gap)
            })
            .fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a: f64| a.max(v)))
            });
        let mut levels: Vec<(i64, f64)> = report
            .levels
            .iter()
            .filter(|(_, ts, _)| *ts >= start && *ts < end)
            .map(|(_, ts, v)| {
                (
                    (ts - &start).total(jiff::Unit::Second).unwrap_or(0.0) as i64,
                    *v,
                )
            })
            .collect();
        levels.sort_by_key(|(at, _)| *at);

        let (mut playing, mut quiet_secs) = (0i64, 0i64);
        for span in &report.spans {
            let lo = span.start.clone().max(start.clone());
            let hi = span.end.clone().min(end.clone());
            if hi > lo {
                playing += (&hi - &lo).total(jiff::Unit::Second).unwrap_or(0.0) as i64;
            }
        }
        for (qs, qe) in quiet.intervals(date, &tz).unwrap_or_default() {
            for span in &report.spans {
                let lo = span.start.clone().max(qs.clone());
                let hi = span.end.clone().min(qe.clone());
                if hi > lo {
                    quiet_secs += (&hi - &lo).total(jiff::Unit::Second).unwrap_or(0.0) as i64;
                }
            }
        }

        // Clipped to the window: the first and last day of a window are
        // usually partial by construction, and calling them under-recorded
        // because of that would cry wolf on every report.
        let lo = start.clone().max(report.window.from.clone());
        let hi = end.clone().min(report.window.to.clone());
        let window_secs = if hi > lo {
            (&hi - &lo).total(jiff::Unit::Second).unwrap_or(0.0) as i64
        } else {
            0
        };
        // The weakest link: a day is only as recorded as its least covered
        // host, because playback on the one that was down leaves no trace.
        let recorded_secs = report
            .coverage
            .iter()
            .map(|c| c.secs(&lo, &hi))
            .min()
            .unwrap_or(0);

        out.push(DaySummary {
            date,
            sustained,
            peak: levels
                .iter()
                .map(|(_, v)| *v)
                .fold(None, |a: Option<f64>, v| {
                    Some(a.map_or(v, |a: f64| a.max(v)))
                }),
            playing_secs: playing,
            quiet_secs,
            recorded_secs,
            window_secs,
        });
        match date.tomorrow() {
            Ok(next) => date = next,
            Err(_) => break,
        }
    }
    Ok(out)
}

/// The loudest level held continuously for at least `SUSTAINED` seconds.
///
/// Every window of that length contributes its quietest moment; the answer
/// is the loudest of those. A brief spike raises no window's minimum, so it
/// cannot move the result.
/// The loudest level held for `SUSTAINED` **without the playing stopping**.
///
/// Level samples exist only while something plays, so a gap between two of
/// them is a gap in the room. Bridging one reports a level nobody held: a
/// minute of music, three hours of silence and a minute more came out as a
/// sustained maximum, which is the one claim a sustained figure exists to
/// refuse.
///
/// A track changing is not a break. The samples run straight through it --
/// the level is what the room heard, and the room does not care where one
/// track ended. That is why a window is bounded by silence rather than by
/// the thing being played, and why an evening of three-minute tracks holds
/// a level that no single track of it does.
fn sustained_max(levels: &[(i64, f64)], gap: i64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for (i, (from, _)) in levels.iter().enumerate() {
        let mut floor = f64::INFINITY;
        let mut prev = *from;
        for (at, v) in &levels[i..] {
            if at - prev > gap {
                break;
            }
            prev = *at;
            floor = floor.min(*v);
            if at - from >= SUSTAINED {
                best = Some(best.map_or(floor, |b: f64| b.max(floor)));
                break;
            }
        }
    }
    best
}

/// The stretch of uninterrupted playing a span belongs to, as offsets from
/// its own start.
///
/// A span is one track, and five minutes is longer than most tracks, so a
/// span asked about itself alone almost never has a level to report. What
/// the question means is the stretch of playing the track sat in, which is
/// the same stretch the timeline draws it inside.
fn run_around(levels: &[(i64, f64)], from: i64, to: i64, gap: i64) -> Vec<(i64, f64)> {
    let Some(first) = levels.iter().position(|(at, _)| *at >= from && *at <= to) else {
        return Vec::new();
    };
    let mut lo = first;
    while lo > 0 && levels[lo].0 - levels[lo - 1].0 <= gap {
        lo -= 1;
    }
    let mut hi = first;
    while hi + 1 < levels.len() && levels[hi + 1].0 - levels[hi].0 <= gap {
        hi += 1;
    }
    levels[lo..=hi].to_vec()
}

/// Chain and anchor status, for the footer every rendered report carries.
///
/// Per host throughout. Each machine writes its own chain and they are never
/// merged, so a break belongs to one host and must be attributed to it.
pub struct Integrity {
    pub chains: Vec<HostChain>,
    pub anchors: Vec<(String, String, String)>,
}

pub struct HostChain {
    pub host: String,
    pub ok: bool,
    pub lines: u64,
    pub problems: Vec<String>,
}

impl Integrity {
    pub fn ok(&self) -> bool {
        self.chains.iter().all(|c| c.ok)
    }

    pub fn lines(&self) -> u64 {
        self.chains.iter().map(|c| c.lines).sum()
    }

    pub fn problems(&self) -> Vec<String> {
        self.chains
            .iter()
            .flat_map(|c| c.problems.iter().map(|p| format!("{}: {p}", c.host)))
            .collect()
    }
}

pub fn integrity(logs_root: &Path, hosts: &[String], window: &Window) -> Result<Integrity> {
    let mut out = Integrity {
        chains: Vec::new(),
        anchors: Vec::new(),
    };

    for host in hosts {
        let host_root = logs_root.join(host);
        let report = log::verify(&host_root)?;
        out.chains.push(HostChain {
            host: host.clone(),
            ok: report.ok(),
            lines: report.lines,
            problems: report.problems,
        });

        for path in day_files(&host_root)? {
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
                    crate::anchor::TokenState::Valid { at } => {
                        format!("{} verified {at}", tsa.name)
                    }
                    crate::anchor::TokenState::Missing => format!("{} not yet stamped", tsa.name),
                    crate::anchor::TokenState::Invalid(why) => {
                        format!("{} INVALID: {why}", tsa.name)
                    }
                })
                .collect();
            out.anchors
                .push((host.clone(), name.to_owned(), states.join(", ")));
        }
    }

    out.anchors.sort_by(|a, b| (&a.1, &a.0).cmp(&(&b.1, &b.0)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, FullBeat, PlayerState, Track};
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
                &Event::Heartbeat(Box::new(FullBeat {
                    streams: Vec::new(),
                    players: vec![playing(title)],
                    sink: None,
                    state: format!("fp-{title}"),
                    beats: 1,
                    since: String::new(),
                })),
                &at,
            )
            .unwrap();
        }
        dir
    }

    /// A program that never reached the bus still has to reach the report.
    ///
    /// The log keeps players and streams apart because they are different
    /// evidence; a reader asking what was playing wants one answer, and
    /// gets it under one name with the program as the media. Two names
    /// would be two colours and two filter entries for every game and
    /// utility that ever made a sound.
    #[test]
    fn a_stream_reaches_the_report_as_a_generic_player() {
        let streams = vec![crate::event::Stream {
            app: "Mixxx".to_owned(),
            binary: Some("mixxx".to_owned()),
            pid: Some(4321),
            media: Some("main out".to_owned()),
            volume: Some(0.5),
        }];
        let merged = heard(&[], &streams);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].player, GENERIC);
        assert_eq!(merged[0].track.title.as_deref(), Some("Mixxx"));
        assert_eq!(merged[0].track.album.as_deref(), Some("main out"));
        // The stream's own gain is in the log and stays out of the report:
        // the effective level is the sink's, and multiplying two stages is
        // the mistake this log has already paid for once.
        assert_eq!(merged[0].player_volume, None);
    }

    /// Players already on the bus are untouched, and the two kinds sit
    /// side by side rather than one replacing the other.
    #[test]
    fn a_stream_does_not_displace_a_player() {
        let streams = vec![crate::event::Stream {
            app: "Mixxx".to_owned(),
            binary: None,
            pid: None,
            media: None,
            volume: None,
        }];
        let player = PlayerState {
            player: "mpd".to_owned(),
            identity: None,
            status: "Playing".to_owned(),
            player_volume: None,
            position_us: None,
            track: Track::default(),
        };
        let merged = heard(std::slice::from_ref(&player), &streams);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].player, "mpd");
        assert_eq!(merged[1].player, GENERIC);
        // Where the media says nothing the program's name is not repeated.
        assert_eq!(merged[1].track.album, None);
    }

    /// The open run's heartbeat is rewritten in place, so while a session
    /// is under way one line stands for every beat since it began and
    /// carries the time of the latest. A window ending inside that run used
    /// to stop at the stamp and throw the run away -- answering "nothing was
    /// playing" about a track that was, which is the one answer this log
    /// exists to get right.
    #[test]
    fn a_window_ending_inside_an_open_run_sees_it() {
        let dir = tempfile::tempdir().unwrap();
        let day = |h: i8, m: i8| {
            jiff::civil::date(2026, 10, 20)
                .at(h, m, 0, 0)
                .to_zoned(TimeZone::UTC)
                .unwrap()
        };
        let (mut log, _) = Log::resume(dir.path(), "h", &day(20, 0)).unwrap();
        // One line, stamped 21:00, standing for every minute since 20:00.
        log.append(
            &Event::Heartbeat(Box::new(FullBeat {
                streams: Vec::new(),
                players: vec![playing("Track")],
                sink: None,
                state: "fp".to_owned(),
                beats: 61,
                since: day(20, 0).timestamp().to_string(),
            })),
            &day(21, 0),
        )
        .unwrap();

        let window = parse_when("2026-10-20T20:30", &now()).unwrap();
        let report = build(dir.path(), &["h".to_owned()], window).unwrap();

        assert_eq!(
            report.at_start[0].1.playing().len(),
            1,
            "the run covers 20:30, so something was playing then"
        );
        assert!(
            report.coverage[0].covers(&report.window.from),
            "and the logger was demonstrably running"
        );
    }

    /// One browser must not become one player per restart.
    #[test]
    fn a_player_is_a_program_not_a_process() {
        assert_eq!(player_name("firefox.instance_1_181"), "firefox");
        assert_eq!(player_name("chromium.instance12345"), "chromium");
        assert_eq!(player_name("mpd"), "mpd");
        assert_eq!(player_name(""), "");
    }

    /// Silence in the log means silence in the room only while the daemon
    /// is running. A report that cannot tell the two apart invites exactly
    /// the inference this log exists to prevent, so the line between them
    /// is pinned here.
    #[test]
    fn a_day_with_no_beats_is_not_a_quiet_day() {
        // Six beats in the evening: the rest of the day is unrecorded.
        let dir = tree(&[18, 19, 20, 21, 22, 23], "Track");
        let window = parse_when("2026-10-20", &now()).unwrap();
        let report = build(dir.path(), &["h".to_owned()], window).unwrap();
        let days = days(&report, &crate::quiet::QuietHours::default()).unwrap();

        assert_eq!(days.len(), 1);
        assert_eq!(days[0].recorded(), Recorded::Partial);
        // Seven records -- the chain opening at midnight and the six beats
        // -- each standing for the minute it samples. Nothing claims the
        // eighteen hours in between, which nobody was watching.
        assert_eq!(days[0].recorded_secs, 420);

        // Hours apart, so each beat stands alone rather than bridging.
        let apart = merge_beats(
            vec![now(), now().checked_add(JiffSpan::new().hours(5)).unwrap()],
            Some(20.0),
        );
        assert_eq!(apart.len(), 2, "a five-hour silence is not coverage");

        // Within three beats of each other, so the daemon never stopped.
        let together = merge_beats(
            vec![
                now(),
                now().checked_add(JiffSpan::new().seconds(20)).unwrap(),
                now().checked_add(JiffSpan::new().seconds(40)).unwrap(),
            ],
            Some(20.0),
        );
        assert_eq!(together.len(), 1);
        assert_eq!(
            together[0].1.timestamp().as_second() - together[0].0.timestamp().as_second(),
            60,
            "three beats twenty seconds apart cover a minute"
        );
    }

    /// The log writes RFC 3339 with an offset, which is a Timestamp; jiff's
    /// Zoned refuses it for want of an IANA annotation. Parsing it as Zoned
    /// made every record silently unreadable and every report empty, with no
    /// error anywhere — so this is the regression that matters most.
    #[test]
    fn records_with_offset_timestamps_are_read() {
        let dir = tree(&[21, 22, 23], "Track");
        let window = parse_when("2026-10-20", &now()).unwrap();
        let report = build(dir.path(), &["h".to_owned()], window).unwrap();

        assert_eq!(report.unreadable, 0, "no record should be unreadable");
        assert!(!report.spans.is_empty(), "playback must be detected");
    }

    #[test]
    fn a_continuous_track_is_one_span() {
        let dir = tree(&[21, 22, 23], "Track");
        let window = parse_when("2026-10-20", &now()).unwrap();
        let report = build(dir.path(), &["h".to_owned()], window).unwrap();

        assert_eq!(report.spans.len(), 1);
        assert_eq!(report.spans[0].start.hour(), 21);
        assert_eq!(report.spans[0].end.hour(), 23);
    }

    /// A stretch continues exactly while the whole track identity is
    /// unchanged -- which is only one rule, so it has to be read the same
    /// way opening a span and continuing one.
    ///
    /// Reading it two ways is what a private browsing window exposed.
    /// Firefox sends no `url`, one `trackid` for every track it will ever
    /// play, and -- with the page private -- the constant title "Zen is
    /// playing media". Opening a span on the trackid and continuing it on
    /// the title meant nothing ever matched: two minutes of playing came
    /// out as thirteen rows of 0s. Keying on the trackid alone would have
    /// fixed those rows by folding a whole evening of browsing into one.
    #[test]
    fn a_track_identified_without_a_url_is_still_one_span() {
        let track = |title: &str| PlayerState {
            player: "firefox.instance_1_476".to_owned(),
            identity: Some("Mozilla zen".to_owned()),
            status: "Playing".to_owned(),
            player_volume: Some(1.0),
            position_us: Some(0),
            track: Track {
                title: Some(title.to_owned()),
                trackid: Some("/org/mpris/MediaPlayer2/firefox".to_owned()),
                ..Track::default()
            },
        };
        let at = |min: i64| now().checked_add(JiffSpan::new().minutes(min)).unwrap();
        let snapshot = |p: PlayerState| Snapshot {
            players: vec![p],
            ..Snapshot::default()
        };

        let held: Vec<_> = (0..4)
            .map(|i| (at(i), snapshot(track("Zen is playing media"))))
            .collect();
        let spans = spans_from(&held, "h");
        assert_eq!(spans.len(), 1, "one stretch, not one span per sample");
        assert_eq!(spans[0].start, at(0));
        assert_eq!(spans[0].end, at(3));

        // The same trackid over a title that changed is two tracks, which
        // is what keying on the trackid alone would have lost.
        let changed = vec![
            (at(0), snapshot(track("A Hymn For The Haunted"))),
            (at(1), snapshot(track("Neon Dreams"))),
        ];
        assert_eq!(spans_from(&changed, "h").len(), 2);
    }

    /// The opening state must come from before the window, not from the
    /// window's own first event. Taking it from after made a day report
    /// claim, at 00:00, a state that only became true that afternoon.
    #[test]
    fn opening_state_precedes_the_window() {
        let dir = tree(&[9, 15, 20], "Track");
        let window = parse_when("2026-10-20T16:00", &now()).unwrap();
        let instant = build(dir.path(), &["h".to_owned()], window).unwrap();
        // An instant sees everything up to and including itself.
        assert!(instant.at_start[0].1.as_of.is_some());

        let window = parse_when("2026-10-20", &now()).unwrap();
        let day = build(dir.path(), &["h".to_owned()], window).unwrap();
        // Nothing precedes the day, so the opening state is empty rather
        // than borrowed from 09:00.
        assert!(
            day.at_start[0].1.players.is_empty(),
            "opening state must not be taken from inside the window"
        );
    }

    /// A collapsed heartbeat stands for a run of samples. Reading it as a
    /// single point left the minutes it covered with no sample at all, which
    /// showed up as holes in the gain profile and skewed the level
    /// statistics toward whatever happened to be changing.
    #[test]
    fn a_collapsed_beat_expands_to_its_samples() {
        let one = Record {
            v: 1,
            ts: "2026-10-20T15:55:00.000+02:00".to_owned(),
            ts_utc: "2026-10-20T13:55:00.000Z".to_owned(),
            host: "h".to_owned(),
            kind: "heartbeat".to_owned(),
            data: serde_json::json!({ "beats": 3, "since": "2026-10-20T15:45:00.000+02:00" }),
            prev: String::new(),
        };
        let at: Zoned = "2026-10-20T15:55:00.000+02:00"
            .parse::<Timestamp>()
            .unwrap()
            .to_zoned(TimeZone::UTC);

        let times = sample_times(&one, &at, &TimeZone::UTC);
        assert_eq!(times.len(), 3, "three samples, not one point");
        assert_eq!(times[0].minute(), 45);
        assert_eq!(times[2].minute(), 55);

        // A line that stands for one sample stays one sample.
        let mut plain = one.clone();
        plain.data = serde_json::json!({ "beats": 1 });
        assert_eq!(sample_times(&plain, &at, &TimeZone::UTC).len(), 1);

        // So does anything that is not a heartbeat.
        let mut other = one.clone();
        other.kind = "playback".to_owned();
        assert_eq!(sample_times(&other, &at, &TimeZone::UTC).len(), 1);
    }

    /// A month is its whole span, and a month at the end of a range
    /// includes all of it rather than stopping on the first.
    #[test]
    fn a_month_is_a_window() {
        let n = now();
        let october = parse_when("2026-10", &n).unwrap();
        assert_eq!(october.label, "October 2026");
        assert_eq!(october.from.date().to_string(), "2026-10-01");
        assert_eq!(october.to.date().to_string(), "2026-11-01");
        assert!(!october.instant);

        let span = parse_when("2026-08..2026-09", &n).unwrap();
        assert_eq!(span.from.date().to_string(), "2026-08-01");
        assert_eq!(span.to.date().to_string(), "2026-10-01");

        // A month start with a date end still works.
        let mixed = parse_when("2026-09..2026-10-05", &n).unwrap();
        assert_eq!(mixed.from.date().to_string(), "2026-09-01");
        assert_eq!(mixed.to.date().to_string(), "2026-10-06");

        // Not every seven characters is a month.
        assert!(parse_when("20xx-10", &n).is_err());
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

#[cfg(test)]
mod sustained_tests {
    use super::*;

    /// Three beats at the twenty-second interval the daemon keeps.
    const GAP: i64 = 60;

    /// The whole point: a brief spike must not move the figure a calendar
    /// shows, while a level actually held must.
    #[test]
    fn a_brief_spike_cannot_raise_the_sustained_maximum() {
        // Ten minutes at 0.3, with one sample at 0.9 in the middle.
        let mut levels: Vec<(i64, f64)> = (0..=60).map(|i| (i * 10, 0.3)).collect();
        levels[30].1 = 0.9;
        assert_eq!(sustained_max(&levels, GAP), Some(0.3));
    }

    #[test]
    fn a_level_actually_held_does_raise_it() {
        // Five minutes at 0.3, then ten at 0.6.
        let mut levels: Vec<(i64, f64)> = (0..=30).map(|i| (i * 10, 0.3)).collect();
        levels.extend((31..=90).map(|i| (i * 10, 0.6)));
        assert_eq!(sustained_max(&levels, GAP), Some(0.6));
    }

    #[test]
    fn too_short_to_sustain_yields_nothing() {
        // Two minutes of playback never completes a five-minute window.
        let levels: Vec<(i64, f64)> = (0..=12).map(|i| (i * 10, 0.5)).collect();
        assert_eq!(sustained_max(&levels, GAP), None);
    }

    /// Silence is not a level being held. Two bursts either side of an
    /// evening once reported a sustained maximum of the louder, which is
    /// the one claim this figure exists to refuse -- it says *held*, and
    /// nothing was held across three hours of nothing.
    #[test]
    fn a_gap_is_not_bridged() {
        let mut levels: Vec<(i64, f64)> = (0..=6).map(|i| (i * 10, 0.9)).collect();
        levels.extend((0..=6).map(|i| (10800 + i * 10, 0.9)));
        assert_eq!(sustained_max(&levels, GAP), None);
        // Two isolated moments are not a stretch either.
        assert_eq!(sustained_max(&[(0, 0.9), (7200, 0.9)], GAP), None);
    }

    /// A track changing is not a break. The room hears one level across a
    /// whole side of a record, and the samples run straight through the
    /// boundaries; a figure that stopped at each one would report nothing
    /// for an evening of three-minute tracks.
    #[test]
    fn a_track_boundary_does_not_break_the_stretch() {
        // Twelve minutes at 0.7, sampled every twenty seconds. Where the
        // tracks fall makes no difference: the samples do not know.
        let levels: Vec<(i64, f64)> = (0..=36).map(|i| (i * 20, 0.7)).collect();
        assert_eq!(sustained_max(&levels, GAP), Some(0.7));
    }

    /// And that is what a span is asked about: not its own three minutes,
    /// but the stretch of playing it sat in.
    #[test]
    fn a_span_is_measured_over_the_stretch_it_belongs_to() {
        // Twenty minutes of continuous playing at 0.6.
        let levels: Vec<(i64, f64)> = (0..=60).map(|i| (i * 20, 0.6)).collect();
        // A three-minute track inside it holds no level of its own...
        let alone: Vec<(i64, f64)> = levels
            .iter()
            .copied()
            .filter(|(at, _)| (300..=480).contains(at))
            .collect();
        assert_eq!(sustained_max(&alone, GAP), None);
        // ...and the stretch around it holds one.
        let run = run_around(&levels, 300, 480, GAP);
        assert_eq!(sustained_max(&run, GAP), Some(0.6));
        // A track in a stretch that is itself too short still has none.
        let brief: Vec<(i64, f64)> = (0..=6).map(|i| (i * 20, 0.6)).collect();
        assert_eq!(sustained_max(&run_around(&brief, 0, 60, GAP), GAP), None);
    }
}
