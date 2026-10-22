//! Writes the synthetic tree the checked-in example reports are built from.
//!
//! It goes through the real `Log`, so the fixture is a genuine log — correctly
//! chained, in the current schema — rather than an imitation that drifts the
//! first time either changes. The hash chain exists in exactly one place and
//! this does not add a second.
//!
//!     cargo run --example demo -- examples/log
//!
//! Three days on one host, chosen to exercise what the reports have to show:
//! a quiet evening, an afternoon film and an evening session that climbs past
//! the start of Nachtruhe, and a short Bluetooth listen. Deterministic, so
//! regenerating changes nothing but the reports' own "generated" line.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use jiff::civil::date;
use jiff::tz::TimeZone;
use jiff::Zoned;

use media_logger::event::{Event, PlayerState, Playing, Sink, Track};
use media_logger::log::Log;
use media_logger::query::fingerprint;

const HOST: &str = "ce2";
/// Fixed so the fixture is reproducible; Berlin because Ruhezeiten are local.
const TZ: &str = "Europe/Berlin";

fn main() -> Result<()> {
    let root: PathBuf = std::env::args()
        .nth(1)
        .context("usage: demo <logs-root>")?
        .into();

    // Refuse to add to an existing tree: this writes fabricated playback, and
    // it must never be possible to mix that into a real log.
    if root.join(HOST).exists() {
        bail!(
            "{} already exists — remove it first; this writes synthetic data \
             and must never be mixed into a real log",
            root.join(HOST).display()
        );
    }

    let tz = TimeZone::get(TZ)?;
    let (mut log, _) = Log::resume(&root, HOST, &at(&tz, 2026, 8, 1, 0, 0))?;

    // Four months, so the calendar has months to lay out and a year's worth
    // of shape to show. Deterministic: the pattern is a function of the day
    // number, never a random draw, so regenerating changes nothing.
    for (year, month, last) in [(2026, 8, 31), (2026, 9, 30), (2026, 10, 31), (2026, 11, 15)] {
        for day_of in 1..=last {
            let sessions = plan(month, day_of);
            if sessions.is_empty() {
                continue;
            }
            day(&mut log, &tz, year, month, day_of, &sessions)?;
        }
    }

    println!("wrote {}", root.join(HOST).display());
    Ok(())
}

/// What was played on a given day, as a pure function of the date.
///
/// Roughly four evenings in five, with the occasional long loud one and
/// stretches of silence — enough variety for a calendar to be worth looking
/// at, and reproducible because nothing here is random.
fn plan(month: i8, day_of: i8) -> Vec<Session> {
    let seed = (month as i32 * 31 + day_of as i32) % 11;
    match seed {
        // Silent.
        0 | 7 => Vec::new(),
        // A loud evening running into Nachtruhe.
        3 => vec![session(
            (21, 10),
            (23, 40),
            mpd(
                "II (Continuous Mix)",
                "Voices from the Lake",
                "sync/vftl/11.flac",
            ),
            scarlett,
            |x| 0.42 + 0.26 * x,
        )],
        // An afternoon film.
        5 => vec![
            session(
                (15, 0),
                (17, 10),
                mpv(
                    "Stalker (1979)",
                    "Andrei Tarkovsky",
                    "/media/films/stalker.mkv",
                ),
                scarlett,
                |x| 0.33 + 0.02 * (x * 4.0).sin(),
            ),
            session(
                (20, 0),
                (21, 30),
                mpd(
                    "Music for 18 Musicians",
                    "Steve Reich",
                    "sync/reich/01.flac",
                ),
                scarlett,
                |x| 0.48 + 0.03 * (x * 6.0).sin(),
            ),
        ],
        // A short listen on the Bluetooth speaker.
        8 => vec![session(
            (20, 0),
            (20, 40),
            firefox(
                "Nils Frahm — Says (Live)",
                "Nils Frahm",
                "https://youtube.com/watch?v=dIww",
            ),
            sonos,
            |x| 0.36 + 0.04 * x,
        )],
        // An ordinary evening.
        n => vec![session(
            (19, 30),
            (21, 20 + n as i8 % 30),
            mpd(
                "Music for 18 Musicians",
                "Steve Reich",
                "sync/reich/01.flac",
            ),
            scarlett,
            move |x| 0.44 + 0.04 * (x * 5.0).sin(),
        )],
    }
}

struct Session {
    from: (i8, i8),
    to: (i8, i8),
    player: PlayerState,
    sink: fn(f64) -> Sink,
    gain: fn(f64) -> f64,
}

fn session(
    from: (i8, i8),
    to: (i8, i8),
    player: PlayerState,
    sink: fn(f64) -> Sink,
    gain: fn(f64) -> f64,
) -> Session {
    Session {
        from,
        to,
        player,
        sink,
        gain,
    }
}

/// Writes one day: a start, each session sampled every five minutes, and a stop.
fn day(
    log: &mut Log,
    tz: &TimeZone,
    year: i16,
    month: i8,
    day_of: i8,
    sessions: &[Session],
) -> Result<()> {
    let first = sessions
        .iter()
        .map(|s| s.from.0)
        .min()
        .unwrap_or(19)
        .saturating_sub(1);
    let last = sessions.iter().map(|s| s.to.0).max().unwrap_or(22).min(23);
    log.append(
        &Event::DaemonStart {
            version: "0.1.0".to_owned(),
        },
        &at(tz, year, month, day_of, first, 0),
    )?;

    for s in sessions {
        let (begin, end) = (
            s.from.0 as i32 * 60 + s.from.1 as i32,
            s.to.0 as i32 * 60 + s.to.1 as i32,
        );
        let span = (end - begin).max(1) as f64;

        let mut minute = begin;
        while minute <= end {
            let progress = (minute - begin) as f64 / span;
            let sink = (s.sink)(round((s.gain)(progress)));
            let when = at(
                tz,
                year,
                month,
                day_of,
                (minute / 60) as i8,
                (minute % 60) as i8,
            );

            if minute == begin {
                log.append(
                    &Event::PlayerAppeared {
                        player: s.player.player.clone(),
                        identity: s.player.identity.clone(),
                    },
                    &when,
                )?;
                log.append(
                    &Event::Playback(Box::new(Playing {
                        state: s.player.clone(),
                        sink: Some(sink.clone()),
                    })),
                    &when,
                )?;
            }

            let players = vec![s.player.clone()];
            let state = fingerprint(&players, &Some(sink.clone()));
            log.beat(players, Some(sink), state, &when)?;
            minute += 5;
        }

        // Idle again, so the span closes where it should.
        let after = at(
            tz,
            year,
            month,
            day_of,
            ((end + 5) / 60) as i8,
            ((end + 5) % 60) as i8,
        );
        log.append(
            &Event::PlayerGone {
                player: s.player.player.clone(),
            },
            &after,
        )?;
        let idle = (s.sink)(round((s.gain)(1.0)));
        let state = fingerprint(&[], &Some(idle.clone()));
        log.beat(Vec::new(), Some(idle), state, &after)?;
    }

    log.append(
        &Event::DaemonStop {
            reason: "sigterm".to_owned(),
        },
        &at(tz, year, month, day_of, last, 58),
    )
    .map(|_| ())
}

fn at(tz: &TimeZone, year: i16, month: i8, day_of: i8, hour: i8, minute: i8) -> Zoned {
    date(year, month, day_of)
        .at(hour, minute, 0, 0)
        .to_zoned(tz.clone())
        .expect("fixture times are valid")
}

/// Three decimals, matching what PipeWire reports in practice.
fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn player(name: &str, identity: &str, title: &str, artist: &str, url: &str) -> PlayerState {
    PlayerState {
        player: name.to_owned(),
        identity: Some(identity.to_owned()),
        status: "Playing".to_owned(),
        player_volume: Some(1.0),
        position_us: Some(0),
        track: Track {
            title: Some(title.to_owned()),
            artist: vec![artist.to_owned()],
            url: Some(url.to_owned()),
            trackid: Some("/t/1".to_owned()),
            length_us: Some(1_800_000_000),
            ..Track::default()
        },
    }
}

fn mpd(title: &str, artist: &str, url: &str) -> PlayerState {
    player(
        "mpd",
        "Music Player Daemon (mpdris2-rs)",
        title,
        artist,
        url,
    )
}

fn mpv(title: &str, artist: &str, url: &str) -> PlayerState {
    player("mpv", "mpv", title, artist, url)
}

fn firefox(title: &str, artist: &str, url: &str) -> PlayerState {
    player("firefox", "Firefox", title, artist, url)
}

/// Pro-audio: no device route, so the node carries the gain — the shape
/// `ce2` actually has.
fn scarlett(gain: f64) -> Sink {
    Sink {
        name: "alsa_output.usb-Focusrite_Scarlett_4i4_USB-00.pro-output-0".to_owned(),
        description: Some("Scarlett 4i4 USB Pro".to_owned()),
        device_api: Some("alsa".to_owned()),
        channel_volumes: vec![gain, gain],
        route_volumes: None,
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: None,
    }
}

/// A mixer-backed device: the node sits at 1.0 and the route carries the
/// gain. This is the shape that recorded a constant before 6a, so the
/// fixture exercises it deliberately.
fn sonos(gain: f64) -> Sink {
    Sink {
        name: "bluez_output.F8_4E_17_02_9A_C1.1".to_owned(),
        description: Some("Sonos Roam".to_owned()),
        device_api: Some("bluez5".to_owned()),
        channel_volumes: vec![1.0, 1.0],
        route_volumes: Some(vec![gain, gain]),
        volume_scale: "linear".to_owned(),
        mute: false,
        bluetooth: Some(media_logger::event::Bluetooth {
            address: Some("F8:4E:17:02:9A:C1".to_owned()),
            profile: Some("a2dp-sink".to_owned()),
            codec: Some("aac".to_owned()),
        }),
    }
}
