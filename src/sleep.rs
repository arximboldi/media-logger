//! When the machine slept, and for how long.
//!
//! Two sources, because they answer different halves of the question and
//! neither is enough alone.
//!
//! logind says a sleep is *coming*, and will hold it back for a moment while
//! the daemon writes that down. Nothing else can put a line in the log
//! before the gap rather than after it. But it is a courtesy: the bus can be
//! absent, the lock refused, the signal late, and a suspend that is called
//! off looks the same on the way in as one that is not.
//!
//! The kernel says a sleep *happened*. It keeps one clock that counts a
//! suspend and one that does not, and the distance between them is the time
//! spent suspended -- nobody has to tell the daemon, and nothing it missed
//! while frozen can make the figure wrong. But it is only knowable
//! afterwards.
//!
//! So `suspend` is written from the first and `resume` from the second, and
//! a log holding only one of the two still says something true.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use rustix::time::{clock_gettime, ClockId};
use tokio::sync::mpsc;
use zbus::{proxy, Connection, OwnedFd};

/// The two members of logind's manager this needs, declared through the bus
/// library's own macro rather than taken from a crate wrapping all of them.
#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;

    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// A sleep that is being held back until this is dropped.
///
/// Holds nothing where logind refused the lock. The sleep is announced all
/// the same, and the line written for it then races the freeze instead of
/// preceding it.
pub struct Pending(#[allow(dead_code)] Option<OwnedFd>);

/// What logind says about a sleep: that one is about to begin, and that
/// one is over.
pub enum Transition {
    /// About to begin, and held back until the [`Pending`] is dropped.
    Going(Pending),
    /// Over. Carries nothing: how long it lasted is the kernel's to say,
    /// through [`Slept`]. It is passed on only so that whoever is waiting
    /// stops waiting and asks. The first sleep under this daemon was
    /// recorded as resuming 8.6 seconds after the machine woke, because
    /// nothing was playing, so nothing stirred, and the daemon did not
    /// look at the clocks until its next beat came round.
    Back,
}

/// Announces each sleep as it is about to begin, and again when it is over.
///
/// The receiver gets one [`Pending`] per sleep and decides when the machine
/// may go by dropping it; logind stops waiting after a few seconds either
/// way, so a daemon that hangs cannot keep a laptop awake in a bag.
pub async fn watch() -> Result<mpsc::Receiver<Transition>> {
    // The system bus, not the session bus the players are on: sleep is the
    // machine's business.
    let conn = Connection::system()
        .await
        .context("connecting to the system bus")?;
    let manager = ManagerProxy::new(&conn).await.context("reaching logind")?;
    let mut signals = manager
        .receive_prepare_for_sleep()
        .await
        .context("subscribing to PrepareForSleep")?;

    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        let mut lock = delay(&manager).await;
        while let Some(signal) = signals.next().await {
            let Ok(args) = signal.args() else { continue };
            let told = if args.start {
                Transition::Going(Pending(lock.take()))
            } else {
                Transition::Back
            };
            if tx.send(told).await.is_err() {
                break;
            }
            if !args.start {
                // A lock is spent by the sleep it delayed; the next one
                // needs its own.
                lock = delay(&manager).await;
            }
        }
    });
    Ok(rx)
}

async fn delay(manager: &ManagerProxy<'_>) -> Option<OwnedFd> {
    manager
        .inhibit(
            "sleep",
            "media-logger",
            "Recording the suspend in the log",
            "delay",
        )
        .await
        .inspect_err(|e| eprintln!("media-logger: no delay lock on sleep: {e:#}"))
        .ok()
}

/// How long the machine has been suspended since it was last asked.
pub struct Slept {
    seen: Duration,
}

/// Shortest suspend worth a line. The two clocks are read one after the
/// other, so their difference wobbles by the microseconds between the
/// reads, and the shortest real suspend is seconds long.
const SHORTEST: Duration = Duration::from_secs(1);

impl Slept {
    pub fn new() -> Self {
        Slept { seen: suspended() }
    }

    /// Time suspended since the last sleep this reported, if there was one.
    pub fn take(&mut self) -> Option<Duration> {
        let total = suspended();
        let slept = total.saturating_sub(self.seen);
        if slept < SHORTEST {
            return None;
        }
        self.seen = total;
        Some(slept)
    }
}

impl Default for Slept {
    fn default() -> Self {
        Self::new()
    }
}

/// Total time this boot has spent suspended: the clock that counts a
/// suspend less the one that stops for it.
fn suspended() -> Duration {
    let read = |clock| {
        let t = clock_gettime(clock);
        Duration::new(t.tv_sec.max(0) as u64, t.tv_nsec as u32)
    };
    read(ClockId::Boottime).saturating_sub(read(ClockId::Monotonic))
}
