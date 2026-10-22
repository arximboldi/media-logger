//! media-logger — a tamper-evident log of media playback, output device and
//! volume.

mod anchor;
mod audio;
mod event;
mod log;
mod mpris;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use jiff::Zoned;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zbus::Connection;

use crate::event::{Beat, Event, FullBeat, PlayerState, Playing, Sink, SinkChange};
use crate::log::Log;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often closed days are checked for a missing timestamp. Rollover makes
/// a day pending; this picks it up, and retries whatever the network refused
/// last time without any queue to keep.
const ANCHOR_EVERY: Duration = Duration::from_secs(3600);

/// How often a heartbeat writes the full picture even though nothing
/// changed, so the log keeps self-contained anchors and refreshes position
/// rather than requiring an unbounded walk backwards.
const FULL_BEAT_EVERY: Duration = Duration::from_secs(60);

/// Longest a sink sample is reused. Bounds `pw-dump` spawns when a player
/// emits property changes rapidly, without making the record meaningfully
/// staler than the event it accompanies.
const SINK_CACHE: Duration = Duration::from_secs(1);

#[derive(Parser)]
#[command(name = "media-logger", version, about)]
struct Cli {
    /// Root of the log tree. Each host writes only to <root>/<hostname>/.
    #[arg(long, env = "MEDIA_LOGGER_ROOT")]
    root: PathBuf,

    /// Host name to log under. Defaults to this machine's hostname.
    #[arg(long, env = "MEDIA_LOGGER_HOST")]
    host: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the logging daemon.
    Run {
        /// Seconds between heartbeats. Unchanged beats are abbreviated, so a
        /// short interval costs far less than it appears to.
        #[arg(long, default_value_t = 10)]
        heartbeat: u64,
    },
    /// Check the hash chain over this host's tree.
    Verify,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let host = cli
        .host
        .clone()
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());

    match cli.command {
        Command::Verify => verify(&cli.root, &host),
        Command::Run { heartbeat } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(cli.root, host, Duration::from_secs(heartbeat))),
    }
}

fn verify(root: &std::path::Path, host: &str) -> Result<()> {
    let host_root = root.join(host);
    let report = log::verify(&host_root)?;
    println!("{} files, {} lines", report.files, report.lines);
    for restart in &report.restarts {
        println!("  chain restart: {restart}");
    }
    println!("chain {}", if report.ok() { "intact" } else { "BROKEN" });
    for problem in &report.problems {
        println!("  PROBLEM: {problem}");
    }

    // A chain nobody anchored proves only internal consistency, so the
    // anchors are reported alongside it rather than behind a second command.
    println!("\nanchors:");
    let today = Zoned::now().date().to_string();
    let mut unstamped = 0;
    let mut invalid = 0;
    for day in log::day_files(&host_root)? {
        let name = day
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let open = name == today;
        let states: Vec<String> = anchor::check(&day)
            .into_iter()
            .map(|(tsa, state)| match state {
                anchor::TokenState::Valid { at } => format!("{} OK {at}", tsa.name),
                anchor::TokenState::Missing => {
                    if !open {
                        unstamped += 1;
                    }
                    format!("{} —", tsa.name)
                }
                anchor::TokenState::Invalid(why) => {
                    invalid += 1;
                    format!("{} INVALID ({why})", tsa.name)
                }
            })
            .collect();
        let note = if open { "  (today, still open)" } else { "" };
        println!("  {name}  {}{note}", states.join(" | "));
    }

    if invalid > 0 {
        anyhow::bail!("{invalid} invalid token(s)");
    }
    if !report.ok() {
        anyhow::bail!("{} chain problem(s)", report.problems.len());
    }
    if unstamped > 0 {
        println!("\n{unstamped} closed day(s) still awaiting a token");
    }
    Ok(())
}

/// Stamps any closed day that lacks a token.
///
/// The work runs on a blocking thread: each request can take seconds, and
/// stalling the main loop would tear a hole in the heartbeat sequence —
/// which is exactly the gap the heartbeat exists to rule out.
async fn anchor_pending(log: &mut Log, host_root: PathBuf) -> Result<()> {
    let today = Zoned::now().date().to_string();
    let stamped = tokio::task::spawn_blocking(move || {
        let mut done = Vec::new();
        for (day, tsas) in anchor::pending(&host_root, &today)? {
            for tsa in tsas {
                match anchor::stamp(&day, tsa) {
                    Ok(s) => done.push((day.clone(), tsa.name, s.at)),
                    // Offline is the ordinary case, not an error: the day
                    // stays pending and the next sweep retries it. Never
                    // backdate, and never record a stamp we did not get.
                    Err(e) => eprintln!(
                        "media-logger: stamping {} with {}: {e:#}",
                        day.display(),
                        tsa.name
                    ),
                }
            }
        }
        anyhow::Ok(done)
    })
    .await??;

    for (day, tsa, at) in stamped {
        let date = day
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        log.append(
            &Event::Anchor {
                queued: is_late(&date, &at),
                date,
                tsa: tsa.to_owned(),
                stamped_at: at,
            },
            &Zoned::now(),
        )?;
    }
    Ok(())
}

/// A day is normally stamped the morning after it closes. Later than that
/// means the sweep could not reach a TSA when it should have.
fn is_late(date: &str, stamped_at: &str) -> bool {
    let (Ok(day), Ok(stamp)) = (
        date.parse::<jiff::civil::Date>(),
        stamped_at
            .get(..10)
            .unwrap_or("")
            .parse::<jiff::civil::Date>(),
    ) else {
        return false;
    };
    day.tomorrow().is_ok_and(|next| stamp > next)
}

/// What the watcher tasks report back to the single writer.
enum Signal {
    Appeared(String),
    Gone(String),
    Changed(String),
}

struct Player {
    identity: Option<String>,
    watcher: JoinHandle<()>,
}

/// Rate-limited view of the current output device.
struct SinkCache {
    sampled: Option<Instant>,
    sink: Option<Sink>,
}

impl SinkCache {
    fn new() -> Self {
        SinkCache {
            sampled: None,
            sink: None,
        }
    }

    /// Returns the current sink, and the previous one when it changed.
    fn refresh(&mut self) -> (Option<Sink>, Option<Sink>) {
        if self.sampled.is_some_and(|at| at.elapsed() < SINK_CACHE) {
            return (self.sink.clone(), None);
        }
        self.sampled = Some(Instant::now());
        let sampled = match audio::sample() {
            Ok(sink) => sink,
            Err(e) => {
                eprintln!("media-logger: sampling sink failed: {e:#}");
                return (self.sink.clone(), None);
            }
        };
        let changed = sampled != self.sink;
        let previous = changed.then(|| self.sink.clone()).flatten();
        self.sink = sampled.clone();
        (sampled, changed.then_some(previous).flatten())
    }
}

async fn run(root: PathBuf, host: String, heartbeat: Duration) -> Result<()> {
    let now = Zoned::now();
    // resume declares any discontinuity in-band itself, so verification
    // reports a restart rather than corruption.
    let (mut log, restart) = Log::resume(&root, &host, &now)?;
    if let Some(restart) = &restart {
        eprintln!("media-logger: chain restarted ({:?})", restart.reason);
    }
    log.append(
        &Event::DaemonStart {
            version: VERSION.to_owned(),
        },
        &now,
    )?;

    let conn = mpris::connect().await?;
    let (tx, mut rx) = mpsc::channel::<Signal>(64);
    let mut players: HashMap<String, Player> = HashMap::new();
    let mut sinks = SinkCache::new();
    let mut beats = Beats::new();

    spawn_name_watcher(&conn, tx.clone()).await?;
    for bus_name in mpris::list_players(&conn).await? {
        tx.send(Signal::Appeared(bus_name)).await.ok();
    }

    let host_root = root.join(&host);
    if let Err(e) = anchor_pending(&mut log, host_root.clone()).await {
        eprintln!("media-logger: anchoring failed: {e:#}");
    }

    let mut anchors = tokio::time::interval(ANCHOR_EVERY);
    anchors.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    anchors.reset(); // the startup sweep already ran

    let mut ticks = tokio::time::interval(heartbeat);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    let reason = loop {
        tokio::select! {
            Some(signal) = rx.recv() => {
                if let Err(e) =
                    handle(&conn, &mut log, &mut players, &mut sinks, &tx, signal).await
                {
                    // A player can vanish between its signal and our read.
                    // That is ordinary, and must not take the daemon down.
                    eprintln!("media-logger: {e:#}");
                }
            }
            _ = ticks.tick() => {
                if let Err(e) = beat(&conn, &mut log, &players, &mut sinks, &mut beats).await {
                    eprintln!("media-logger: heartbeat failed: {e:#}");
                }
            }
            _ = anchors.tick() => {
                if let Err(e) = anchor_pending(&mut log, host_root.clone()).await {
                    eprintln!("media-logger: anchoring failed: {e:#}");
                }
            }
            _ = sigint.recv() => break "sigint",
            _ = sigterm.recv() => break "sigterm",
        }
    };

    for player in players.values() {
        player.watcher.abort();
    }
    log.append(
        &Event::DaemonStop {
            reason: reason.to_owned(),
        },
        &Zoned::now(),
    )?;
    Ok(())
}

async fn spawn_name_watcher(conn: &Connection, tx: mpsc::Sender<Signal>) -> Result<()> {
    let mut stream = mpris::name_changes(conn).await?;
    tokio::spawn(async move {
        while let Some(signal) = stream.next().await {
            let Ok(args) = signal.args() else { continue };
            let name = args.name().as_str().to_owned();
            if !name.starts_with(mpris::PREFIX) {
                continue;
            }
            let signal = if args.new_owner().is_some() {
                Signal::Appeared(name)
            } else {
                Signal::Gone(name)
            };
            if tx.send(signal).await.is_err() {
                break;
            }
        }
    });
    Ok(())
}

async fn handle(
    conn: &Connection,
    log: &mut Log,
    players: &mut HashMap<String, Player>,
    sinks: &mut SinkCache,
    tx: &mpsc::Sender<Signal>,
    signal: Signal,
) -> Result<()> {
    match signal {
        Signal::Appeared(bus_name) => {
            if players.contains_key(&bus_name) {
                return Ok(());
            }
            let identity = mpris::identity(conn, &bus_name).await.unwrap_or(None);
            let watcher = spawn_player_watcher(conn, &bus_name, tx.clone()).await?;
            players.insert(
                bus_name.clone(),
                Player {
                    identity: identity.clone(),
                    watcher,
                },
            );

            log.append(
                &Event::PlayerAppeared {
                    player: mpris::short_name(&bus_name).to_owned(),
                    identity,
                },
                &Zoned::now(),
            )?;
            // Record what it is already doing: a player that was playing
            // before we attached emits no change of its own.
            record_playback(conn, log, players, sinks, &bus_name).await
        }
        Signal::Gone(bus_name) => {
            if let Some(player) = players.remove(&bus_name) {
                player.watcher.abort();
                log.append(
                    &Event::PlayerGone {
                        player: mpris::short_name(&bus_name).to_owned(),
                    },
                    &Zoned::now(),
                )?;
            }
            Ok(())
        }
        Signal::Changed(bus_name) => record_playback(conn, log, players, sinks, &bus_name).await,
    }
}

async fn spawn_player_watcher(
    conn: &Connection,
    bus_name: &str,
    tx: mpsc::Sender<Signal>,
) -> Result<JoinHandle<()>> {
    let mut stream = mpris::changes(conn, bus_name).await?;
    let bus_name = bus_name.to_owned();
    Ok(tokio::spawn(async move {
        while stream.next().await.is_some() {
            if tx.send(Signal::Changed(bus_name.clone())).await.is_err() {
                break;
            }
        }
    }))
}

async fn record_playback(
    conn: &Connection,
    log: &mut Log,
    players: &HashMap<String, Player>,
    sinks: &mut SinkCache,
    bus_name: &str,
) -> Result<()> {
    let identity = players.get(bus_name).and_then(|p| p.identity.clone());
    let state = mpris::snapshot(conn, bus_name, identity).await?;
    let (sink, previous) = sinks.refresh();
    let now = Zoned::now();

    if let Some(current) = &sink {
        if previous.is_some() {
            log.append(
                &Event::SinkChanged(Box::new(SinkChange {
                    sink: current.clone(),
                    previous,
                })),
                &now,
            )?;
        }
    }
    log.append(&Event::Playback(Box::new(Playing { state, sink })), &now)?;
    Ok(())
}

/// Identity of a state for change detection.
///
/// Deliberately excludes track position, which advances continuously: folding
/// it in would mean no two beats ever matched, defeating abbreviation exactly
/// while a track plays and lines are at their largest.
fn fingerprint(players: &[PlayerState], sink: &Option<Sink>) -> String {
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
    let state = serde_json::json!({ "players": key, "sink": sink });
    log::hash_line(&state.to_string())[..16].to_owned()
}

/// Tracks what the last heartbeat said, so an unchanged one can be
/// abbreviated instead of repeated in full.
struct Beats {
    last: Option<String>,
    last_full: Option<Instant>,
}

impl Beats {
    fn new() -> Self {
        Beats {
            last: None,
            last_full: None,
        }
    }

    fn next(&mut self, players: Vec<PlayerState>, sink: Option<Sink>) -> Beat {
        let state = fingerprint(&players, &sink);
        let stale = self
            .last_full
            .is_none_or(|at| at.elapsed() >= FULL_BEAT_EVERY);
        if !stale && self.last.as_deref() == Some(state.as_str()) {
            return Beat::Repeat { unchanged: state };
        }
        self.last = Some(state.clone());
        self.last_full = Some(Instant::now());
        Beat::Full(Box::new(FullBeat {
            players,
            sink,
            state,
        }))
    }
}

/// Writes the periodic record of what was happening, including when nothing
/// was. The empty case is the one that lets the log prove a negative.
async fn beat(
    conn: &Connection,
    log: &mut Log,
    players: &HashMap<String, Player>,
    sinks: &mut SinkCache,
    beats: &mut Beats,
) -> Result<()> {
    let mut states = Vec::new();
    for (bus_name, player) in players {
        match mpris::snapshot(conn, bus_name, player.identity.clone()).await {
            Ok(state) => states.push(state),
            Err(e) => eprintln!("media-logger: reading {bus_name} failed: {e:#}"),
        }
    }
    states.sort_by(|a, b| a.player.cmp(&b.player));

    let (sink, previous) = sinks.refresh();
    let now = Zoned::now();
    if let Some(current) = &sink {
        if previous.is_some() {
            log.append(
                &Event::SinkChanged(Box::new(SinkChange {
                    sink: current.clone(),
                    previous,
                })),
                &now,
            )?;
        }
    }
    log.append(&Event::Heartbeat(beats.next(states, sink)), &now)
        .context("appending heartbeat")?;
    Ok(())
}
