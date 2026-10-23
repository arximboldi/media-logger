//! media-logger — a tamper-evident log of media playback, output device and
//! volume.

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

use media_logger::event::{Event, Playing, Sink, SinkChange, Stream};
use media_logger::log::Log;
use media_logger::{anchor, audio, log, mpris, query, quiet, render};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often closed days are checked for a missing timestamp. Rollover makes
/// a day pending; this picks it up, and retries whatever the network refused
/// last time without any queue to keep.
const ANCHOR_EVERY: Duration = Duration::from_secs(3600);

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

    /// Host name to log under, or to report on. Repeatable.
    ///
    /// The daemon writes under exactly one. Reporting defaults to *every*
    /// host in the tree, because a document that quietly omits a machine is
    /// worse than one that is too broad.
    #[arg(long, env = "MEDIA_LOGGER_HOST")]
    host: Vec<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the logging daemon.
    Run {
        /// Seconds between heartbeats.
        ///
        /// A run of unchanged beats collapses into one line that is rewritten
        /// in place, so a short interval costs resolution, not storage.
        #[arg(long, default_value_t = 20)]
        heartbeat: u64,
    },
    /// Check the hash chain and the anchors over this host's tree.
    Verify,
    /// Report on a moment, a day, or a range.
    ///
    /// <WHEN> accepts an instant (2026-10-22T23:14), a day (2026-10-22), a
    /// range (2026-10-01..2026-10-31), or today / yesterday / last-night.
    Show {
        /// Defaults to everything the tree holds.
        when: Option<String>,

        #[arg(long, value_enum, default_value_t = render::Format::Text)]
        format: render::Format,

        /// Write to a file instead of stdout. Required for pdf and xlsx.
        #[arg(long, short)]
        out: Option<PathBuf>,

        /// Render to a temporary file and open it in a browser.
        ///
        /// The interactive view: filters, the calendar, and gain readouts on
        /// hover, none of which survive into a PDF.
        #[arg(long, conflicts_with_all = ["format", "out"])]
        web: bool,

        /// Only this player. Repeatable.
        #[arg(long, value_name = "NAME")]
        player: Vec<String>,

        /// Only this output device, matched against its name or description.
        /// Repeatable.
        #[arg(long, value_name = "NAME")]
        device: Vec<String>,

        /// Only playback that touched quiet hours, or only playback that did
        /// not. A session partly inside counts as inside.
        #[arg(long, value_enum)]
        during: Option<During>,

        /// Calendar colour scale: "auto", or LOW:HIGH on the slider's
        /// scale, where 0.5 is a slider half way up.
        ///
        /// Auto spans the days shown, which is what makes their differences
        /// visible without knowing what ordinary and loud read as on the
        /// device. Fixed bounds are for comparing two reports to each other.
        #[arg(long, value_name = "AUTO|LOW:HIGH", default_value = "auto")]
        gain_scale: render::GainScale,

        /// Quiet hours, repeatable. Defaults to German Nachtruhe.
        ///
        /// Accepts "22:00-06:00", "sun 00:00-24:00", "mon-fri 13:00-15:00".
        /// Mittagsruhe and Sunday rules come from a building's Hausordnung
        /// and are not applied unless given; common additions are
        /// 13:00-15:00 (Mittagsruhe) and sun 00:00-24:00 (Sundays).
        ///
        /// Also read from MEDIA_LOGGER_QUIET_HOURS as a ";"-separated list,
        /// which the NixOS module sets from its own option.
        #[arg(long = "quiet-hours", value_name = "SPEC")]
        quiet_hours: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Verify => verify(&cli.root, &reporting_hosts(&cli.root, &cli.host)?),
        Command::Show {
            when,
            format,
            out,
            web,
            player,
            device,
            during,
            gain_scale,
            quiet_hours,
        } => show(
            &cli.root,
            &reporting_hosts(&cli.root, &cli.host)?,
            when.as_deref(),
            if web { render::Format::Html } else { format },
            out.as_deref(),
            web,
            Filters {
                player,
                device,
                during,
            },
            gain_scale,
            &quiet_hours,
        ),
        Command::Run { heartbeat } => {
            let host = cli
                .host
                .first()
                .cloned()
                .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(run(cli.root, host, Duration::from_secs(heartbeat)))
        }
    }
}

/// The range spanning every day file the given hosts hold.
fn whole_tree(root: &std::path::Path, hosts: &[String]) -> Result<String> {
    let mut days: Vec<String> = Vec::new();
    for host in hosts {
        for path in log::day_files(&root.join(host))? {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                days.push(name.to_owned());
            }
        }
    }
    days.sort();
    match (days.first(), days.last()) {
        // Names are zero-padded dates, so the extremes sort out directly.
        (Some(first), Some(last)) => Ok(format!("{first}..{last}")),
        _ => anyhow::bail!("no log files under {}", root.display()),
    }
}

/// Hosts a report covers: those named, or every one in the tree.
///
/// Defaulting to all of them is the point. Reporting on the local hostname
/// silently omitted any other machine sharing the tree, in a document whose
/// value depends on being complete.
fn reporting_hosts(root: &std::path::Path, named: &[String]) -> Result<Vec<String>> {
    let present = log::hosts(root)?;
    if named.is_empty() {
        if present.is_empty() {
            anyhow::bail!("no host trees under {}", root.display());
        }
        return Ok(present);
    }
    for want in named {
        if !present.contains(want) {
            anyhow::bail!(
                "no log tree for host {want} under {} (present: {})",
                root.display(),
                present.join(", ")
            );
        }
    }
    Ok(named.to_vec())
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum During {
    /// Touched Ruhezeiten at all.
    Quiet,
    /// Did not.
    Other,
}

/// Narrowing applied to a report.
pub struct Filters {
    player: Vec<String>,
    device: Vec<String>,
    during: Option<During>,
}

fn verify(root: &std::path::Path, hosts: &[String]) -> Result<()> {
    let mut o = String::new();
    let mut invalid = 0;
    let mut unstamped = 0;
    let mut broken = 0;
    let today = Zoned::now().date().to_string();

    for host in hosts {
        let host_root = root.join(host);
        let report = log::verify(&host_root)?;
        o.push_str(&format!(
            "{host}: {} files, {} lines — chain {}\n",
            report.files,
            report.lines,
            if report.ok() { "intact" } else { "BROKEN" }
        ));
        if !report.ok() {
            broken += 1;
        }
        for restart in &report.restarts {
            o.push_str(&format!("  chain restart: {restart}\n"));
        }
        for problem in &report.problems {
            o.push_str(&format!("  PROBLEM: {problem}\n"));
        }

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
            o.push_str(&format!("  {name}  {}{note}\n", states.join(" | ")));
        }
    }

    if unstamped > 0 {
        o.push_str(&format!(
            "\n{unstamped} closed day(s) still awaiting a token\n"
        ));
    }
    render::to_stdout(o.trim_end())?;

    if invalid > 0 {
        anyhow::bail!("{invalid} invalid token(s)");
    }
    if broken > 0 {
        anyhow::bail!("{broken} host(s) with a broken chain");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn show(
    root: &std::path::Path,
    hosts: &[String],
    when: Option<&str>,
    format: render::Format,
    out: Option<&std::path::Path>,
    web: bool,
    filters: Filters,
    scale: render::GainScale,
    quiet_hours: &[String],
) -> Result<()> {
    let specs: Vec<String> = if !quiet_hours.is_empty() {
        quiet_hours.to_vec()
    } else {
        match std::env::var("MEDIA_LOGGER_QUIET_HOURS") {
            Ok(v) if !v.trim().is_empty() => v
                .split(';')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => vec![quiet::DEFAULT.to_owned()],
        }
    };
    let quiet = quiet::QuietHours::parse(&specs)?;

    let spec = match when {
        Some(w) if !w.eq_ignore_ascii_case("all") => w.to_owned(),
        // No window given: every day the tree holds. Asking for a report of
        // everything should not require knowing when "everything" started.
        _ => whole_tree(root, hosts)?,
    };
    let window = query::parse_when(&spec, &Zoned::now())?;
    let integrity = query::integrity(root, hosts, &window)?;
    let mut report = query::build(root, hosts, window)?;
    filters.apply(&mut report, &quiet);

    if !web {
        return render::render(&mut report, &integrity, &quiet, format, out, scale);
    }

    // A real file rather than a handle that dies with this process: the
    // browser opens it after we have exited. The pid is in the name because
    // the second is not: two reports a moment apart are two windows, and the
    // second must not overwrite a page the browser is still loading.
    let page = std::env::temp_dir().join(format!(
        "media-logger-{}-{}.html",
        Zoned::now().strftime("%Y%m%d-%H%M%S"),
        std::process::id()
    ));
    render::render(&mut report, &integrity, &quiet, format, Some(&page), scale)?;
    open_in_browser(&page)
}

/// Hands a file to whatever the desktop uses to open it.
fn open_in_browser(page: &std::path::Path) -> Result<()> {
    match std::process::Command::new("xdg-open").arg(page).status() {
        Ok(status) if status.success() => Ok(()),
        // Failing to launch is not failing to report: the file is written
        // and named, so say where it is rather than throwing it away.
        other => {
            eprintln!(
                "could not open a browser ({}); the report is at {}",
                match other {
                    Ok(s) => format!("xdg-open exited with {s}"),
                    Err(e) => e.to_string(),
                },
                page.display()
            );
            Ok(())
        }
    }
}

impl Filters {
    /// Narrows a built report.
    ///
    /// Applied after reconstruction, never during: state has to be replayed
    /// in full or the reconstruction is wrong, and a filter is about what the
    /// reader is shown, not about what happened.
    fn apply(&self, report: &mut query::Report, quiet: &quiet::QuietHours) {
        if self.player.is_empty() && self.device.is_empty() && self.during.is_none() {
            return;
        }
        let matches = |span: &query::Span| {
            let player_ok = self.player.is_empty()
                || self
                    .player
                    .iter()
                    .any(|p| span.player.eq_ignore_ascii_case(p));
            let device_ok = self.device.is_empty()
                || self.device.iter().any(|d| {
                    let hay = format!(
                        "{} {}",
                        span.sink.as_deref().unwrap_or(""),
                        span.sink_label.as_deref().unwrap_or("")
                    );
                    hay.to_lowercase().contains(&d.to_lowercase())
                });
            let during_ok = match self.during {
                None => true,
                Some(During::Quiet) => render::when_of(span, quiet) == "quiet hours",
                Some(During::Other) => render::when_of(span, quiet) == "other times",
            };
            player_ok && device_ok && during_ok
        };
        report.spans.retain(matches);

        // Levels belong to spans that survived, or the gain lanes would
        // outlive the bars they describe.
        let kept: Vec<_> = report
            .spans
            .iter()
            .map(|s| (s.host.clone(), s.start.clone(), s.end.clone()))
            .collect();
        report.levels.retain(|(host, ts, _)| {
            kept.iter()
                .any(|(h, from, to)| h == host && ts >= from && ts <= to)
        });
    }
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

/// Rate-limited view of the audio graph: the output device, and what is
/// sending sound to it.
struct SinkCache {
    sampled: Option<Instant>,
    sink: Option<Sink>,
    streams: Vec<Stream>,
}

/// What one refresh found: the sink now, the programs sending audio now,
/// and the sink before, when it changed.
struct Current {
    sink: Option<Sink>,
    streams: Vec<Stream>,
    previous: Option<Sink>,
}

impl SinkCache {
    fn new() -> Self {
        SinkCache {
            sampled: None,
            sink: None,
            streams: Vec::new(),
        }
    }

    fn refresh(&mut self) -> Current {
        let held = |me: &Self, previous| Current {
            sink: me.sink.clone(),
            streams: me.streams.clone(),
            previous,
        };
        if self.sampled.is_some_and(|at| at.elapsed() < SINK_CACHE) {
            return held(self, None);
        }
        self.sampled = Some(Instant::now());
        let sampled = match audio::sample() {
            Ok(sampled) => sampled,
            Err(e) => {
                eprintln!("media-logger: sampling audio failed: {e:#}");
                return held(self, None);
            }
        };
        let changed = sampled.sink != self.sink;
        let previous = changed.then(|| self.sink.clone()).flatten();
        self.sink = sampled.sink;
        self.streams = sampled.streams;
        held(self, previous)
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
                if let Err(e) = beat(&conn, &mut log, &players, &mut sinks).await {
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
    // A playback event is about a player, so only the sink travels with it;
    // what else was sending audio is the heartbeat's business.
    let Current { sink, previous, .. } = sinks.refresh();
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

/// Writes the periodic record of what was happening, including when nothing
/// was. The empty case is the one that lets the log prove a negative.
async fn beat(
    conn: &Connection,
    log: &mut Log,
    players: &HashMap<String, Player>,
    sinks: &mut SinkCache,
) -> Result<()> {
    let mut states = Vec::new();
    for (bus_name, player) in players {
        match mpris::snapshot(conn, bus_name, player.identity.clone()).await {
            Ok(state) => states.push(state),
            Err(e) => eprintln!("media-logger: reading {bus_name} failed: {e:#}"),
        }
    }
    states.sort_by(|a, b| a.player.cmp(&b.player));

    let Current {
        sink,
        streams,
        previous,
    } = sinks.refresh();
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
    // Only what no player accounts for. A browser announces itself on the
    // bus and also shows up as a stream; recorded twice it would double a
    // quiet evening into a loud one.
    //
    // Asked of the bus each beat rather than remembered: a player's pid is
    // only stable while that player is, and a stale one would silence a
    // stream that had taken over the number.
    let mut owned: Vec<i64> = Vec::new();
    for bus_name in players.keys() {
        if let Some(pid) = mpris::pid_of(conn, bus_name).await {
            owned.push(pid as i64);
        }
    }
    let streams: Vec<Stream> = streams
        .into_iter()
        .filter(|s| s.pid.is_none_or(|p| !owned.contains(&p)))
        .collect();
    let state = query::fingerprint(&states, &streams, &sink);
    log.beat(states, streams, sink, state, &now)
        .context("recording heartbeat")?;
    Ok(())
}
