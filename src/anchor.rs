//! RFC 3161 timestamping of closed day files.
//!
//! A hash chain proves internal consistency but nothing against the person
//! holding every input — it can always be recomputed. Its value comes from
//! anchoring to a third party, which is what this does.
//!
//! Only a SHA-256 digest leaves the machine. The log content never does,
//! which is what this has over committing the tree to a remote.
//!
//! Shells out to `openssl ts` and `curl` rather than taking an RFC 3161
//! crate: both are proven, carry no dependency risk, and a third party
//! verifies a token with the same standard command we produce it with.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use jiff::civil::{date, Date};
use jiff::tz::TimeZone;
use jiff::{Unit, Zoned};
use sha2::{Digest, Sha256};

use crate::log::day_files;

pub struct Tsa {
    pub name: &'static str,
    pub url: &'static str,
    /// Pinned trust anchor, embedded so it travels with the binary and needs
    /// no runtime path. Verification months from now must not depend on
    /// hunting for a root certificate.
    pub ca: &'static [u8],
}

/// DFN first: a named German institution (DFN-CERT Services GmbH), which is
/// what counts when the audience is a German landlord or court.
///
/// DigiCert is not merely belt-and-braces. The DFN Community Root 2022 is in
/// no system trust store and is published at no URL we could find, so the
/// only copy available came from a token itself — making that anchor
/// self-referential. DigiCert's root was fetched from its official
/// distribution point, so it is the one independently rooted attestation.
pub const TSAS: &[Tsa] = &[
    Tsa {
        name: "dfn",
        url: "http://zeitstempel.dfn.de",
        ca: include_bytes!("../anchors/dfn.pem"),
    },
    Tsa {
        name: "digicert",
        url: "http://timestamp.digicert.com",
        ca: include_bytes!("../anchors/digicert.pem"),
    },
];

/// How long a closed day has gone without the tokens it is owed.
///
/// `None` while the day is still open, or when every token is in.
///
/// Said in one place because two readers ask it -- `verify` and the
/// footer every report carries -- and a figure that is worded two ways
/// reads as two measurements.
///
/// Measured from the end of the day it covers, which is when the file
/// stopped growing and the token fell due. Not from the file's mtime: a
/// replicated tree rewrites that, and a day's own date is the one fact
/// about it that cannot drift.
///
/// There is no threshold here and no word for "too long". The number
/// grows, which is the whole of what needs saying: an hour of being
/// offline and a fortnight of being broken look the same in a report
/// that only says "not yet stamped", and they do not look the same here.
pub fn awaiting(day: Date, missing: &[&str], now: &Zoned) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    let waited = waited(day, now)?;
    let names = match missing {
        [one] => (*one).to_owned(),
        [most @ .., last] => format!("{} and {last}", most.join(", ")),
        [] => unreachable!("checked above"),
    };
    Some(format!("awaiting {names} for {waited}"))
}

/// How long since a day closed, in words. `None` while it is still open.
///
/// Split out so the two sentences that quote it -- one per day and one
/// over the whole tree -- say the same thing the same way.
pub fn waited(day: Date, now: &Zoned) -> Option<String> {
    let closed = day
        .tomorrow()
        .ok()?
        .to_zoned(now.time_zone().clone())
        .ok()?;
    let seconds = (now - &closed).total(Unit::Second).ok()? as i64;
    // Not yet closed, or a tree carried over from a machine whose clock
    // disagrees. Either way there is no duration worth asserting.
    if seconds <= 0 {
        return None;
    }
    let count = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit}")
        } else {
            format!("{n} {unit}s")
        }
    };
    Some(match seconds {
        s if s < 60 => "under a minute".to_owned(),
        s if s < 3600 => count(s / 60, "minute"),
        s if s < 2 * 86400 => count(s / 3600, "hour"),
        s => count(s / 86400, "day"),
    })
}

pub fn token_path(day: &Path, tsa: &Tsa) -> PathBuf {
    let mut name = day.file_stem().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tsr", tsa.name));
    day.with_file_name(name)
}

/// Closed day files still missing a token, paired with the TSAs that owe one.
///
/// This *is* the queue. Nothing is cached: a day file without a token beside
/// it is pending by definition, so there is no second source of truth able to
/// disagree with the tree.
pub fn pending(host_root: &Path, today: &str) -> Result<Vec<(PathBuf, Vec<&'static Tsa>)>> {
    let mut work = Vec::new();
    for day in day_files(host_root)? {
        // Today's file is still being appended to; a token over it would be
        // stale the moment the next line lands.
        if day.file_stem().and_then(|s| s.to_str()) == Some(today) {
            continue;
        }
        let missing: Vec<_> = TSAS
            .iter()
            .filter(|tsa| !token_path(&day, tsa).exists())
            .collect();
        if !missing.is_empty() {
            work.push((day, missing));
        }
    }
    Ok(work)
}

pub struct Stamped {
    /// The time the authority asserts, not our observation of it.
    pub at: String,
}

/// Obtains a token over `day` from `tsa` and stores it beside the file.
/// Puts a verified token beside the day file it covers.
///
/// Copied in and then renamed, rather than renamed in from the scratch
/// directory. A rename cannot cross a mount point, and the scratch
/// directory is the one `StateDirectory=` makes, which systemd bind-mounts
/// into the unit's own namespace: the same disk, a different mount, and
/// `EXDEV` once an hour for every day still pending. The rename was wrong
/// about where the tree is rather than about systemd -- a log tree on
/// another disk, which is an ordinary thing to want, failed the same way.
///
/// Still a rename at the end, within the destination directory, so a
/// reader sees a whole token or none. A half-written `.tsr` verifies as a
/// broken one, and a broken token sitting in the tree looking like
/// evidence is worse than no token at all.
fn store(from: &Path, into: &Path) -> Result<()> {
    let mut staging = into.as_os_str().to_owned();
    staging.push(".part");
    let staging = PathBuf::from(staging);

    let mut src = fs::File::open(from).context("reading the token back")?;
    let mut dst =
        fs::File::create(&staging).with_context(|| format!("writing {}", staging.display()))?;
    std::io::copy(&mut src, &mut dst).context("copying the token into the tree")?;
    // Flushed before it is named: the rename could otherwise land while the
    // bytes behind it had not, leaving an empty file that reads as a broken
    // token rather than as a missing one.
    dst.sync_all().context("flushing the token")?;
    drop(dst);

    if let Err(e) = fs::rename(&staging, into) {
        let _ = fs::remove_file(&staging);
        return Err(e).with_context(|| format!("naming {}", into.display()));
    }
    // The scratch copy has served its purpose; the tree is the record.
    let _ = fs::remove_file(from);
    Ok(())
}

pub fn stamp(day: &Path, tsa: &Tsa) -> Result<Stamped> {
    let scratch = scratch_dir()?;
    let request = scratch.join(format!("{}.tsq", tsa.name));
    let reply = scratch.join(format!("{}.tsr", tsa.name));
    let ca = pinned_root(&scratch, tsa).context("writing the pinned root")?;

    // -cert embeds the signer certificate, so the token stays self-contained
    // once "Zeitstempel 2026" expires.
    run(
        Command::new("openssl")
            .args(["ts", "-query", "-data"])
            .arg(day)
            .args(["-sha256", "-cert", "-out"])
            .arg(&request),
        "building the timestamp request",
    )?;

    run(
        Command::new("curl")
            .args(["-sS", "--max-time", "30", "-H"])
            .arg("Content-Type: application/timestamp-query")
            .arg("--data-binary")
            .arg(format!("@{}", request.display()))
            .arg(tsa.url)
            .arg("-o")
            .arg(&reply),
        "sending the timestamp request",
    )?;

    let text = run(
        Command::new("openssl")
            .args(["ts", "-reply", "-in"])
            .arg(&reply)
            .arg("-text"),
        "reading the timestamp reply",
    )?;
    if !text.contains("Status: Granted") {
        bail!("{} did not grant a token: {}", tsa.name, first_line(&text));
    }

    // Verify before storing. A truncated or error response that merely looks
    // like a token is worse than none: it would sit in the tree looking like
    // evidence until someone tried to rely on it.
    verify_token(day, &reply, &ca)
        .with_context(|| format!("token from {} failed verification on arrival", tsa.name))?;

    store(&reply, &token_path(day, tsa)).context("storing the token")?;
    Ok(Stamped {
        at: parse_stamp_time(&text)?,
    })
}

pub enum TokenState {
    Valid { at: String },
    Invalid(String),
    Missing,
}

/// Checks the stored tokens of many day files, several at a time, and
/// answers in the order asked.
///
/// Every token is still checked every time: that is what the line in the
/// report is worth. But each check is `openssl` started twice, and a
/// report over a month starts it two hundred times, one after another,
/// while every core but one waits. They share nothing but the pinned
/// roots, which are made so that they can be shared.
pub fn check_all(days: &[PathBuf]) -> Vec<Vec<(&'static Tsa, TokenState)>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(days.len());
    let mut checked: Vec<(usize, Vec<(&'static Tsa, TokenState)>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(day) = days.get(i) else { break };
                        mine.push((i, check(day)));
                    }
                    mine
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("checking a token panicked"))
            .collect()
    });
    checked.sort_by_key(|(i, _)| *i);
    checked.into_iter().map(|(_, states)| states).collect()
}

/// Checks the stored tokens for one day file.
pub fn check(day: &Path) -> Vec<(&'static Tsa, TokenState)> {
    let scratch = scratch_dir().ok();
    TSAS.iter()
        .map(|tsa| {
            let token = token_path(day, tsa);
            if !token.exists() {
                return (tsa, TokenState::Missing);
            }
            let Some(scratch) = &scratch else {
                return (tsa, TokenState::Invalid("no scratch directory".to_owned()));
            };
            let ca = match pinned_root(scratch, tsa) {
                Ok(ca) => ca,
                Err(e) => return (tsa, TokenState::Invalid(format!("{e:#}"))),
            };
            match verify_token(day, &token, &ca) {
                Ok(()) => match read_stamp_time(&token) {
                    Ok(at) => (tsa, TokenState::Valid { at }),
                    Err(e) => (tsa, TokenState::Invalid(e.to_string())),
                },
                Err(e) => (
                    tsa,
                    TokenState::Invalid(first_line(&e.to_string()).to_owned()),
                ),
            }
        })
        .collect()
}

fn verify_token(day: &Path, token: &Path, ca: &Path) -> Result<()> {
    let out = Command::new("openssl")
        .args(["ts", "-verify", "-data"])
        .arg(day)
        .arg("-in")
        .arg(token)
        .arg("-CAfile")
        .arg(ca)
        .output()
        .context("running openssl ts -verify")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() && combined.contains("Verification: OK") {
        return Ok(());
    }
    bail!("{}", first_line(&combined));
}

fn read_stamp_time(token: &Path) -> Result<String> {
    let text = run(
        Command::new("openssl")
            .args(["ts", "-reply", "-in"])
            .arg(token)
            .arg("-text"),
        "reading a stored token",
    )?;
    parse_stamp_time(&text)
}

/// `Time stamp: Oct 22 00:36:14 2026 GMT` → `2026-10-22T00:36:14Z`.
///
/// Parsed by hand because the day is space-padded for single digits, which
/// the obvious format strings disagree about.
fn parse_stamp_time(text: &str) -> Result<String> {
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Time stamp:"))
        .context("no timestamp in the reply")?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    let [month, day, time, year, ..] = parts.as_slice() else {
        bail!("unexpected timestamp format: {line}");
    };

    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS
        .iter()
        .position(|m| m == month)
        .with_context(|| format!("unknown month {month}"))? as i8
        + 1;
    let hms: Vec<&str> = time.split(':').collect();
    let [h, m, s] = hms.as_slice() else {
        bail!("unexpected time {time}");
    };

    let stamped = date(year.parse()?, month, day.parse()?)
        .at(h.parse()?, m.parse()?, s.parse()?, 0)
        .to_zoned(TimeZone::UTC)?;
    Ok(stamped.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// The pinned root of `tsa`, as a file `openssl` can be pointed at.
///
/// The root travels in the binary and `openssl` wants a path, so it has to
/// be put on disk; the question is how, with other processes wanting the
/// same file. It used to be written out before every single check, in
/// place, under one fixed name. A write in place empties the file and then
/// fills it, and anything reading in between finds a root with nothing in
/// it. So two reports run at once, or a report run while the daemon was
/// stamping, could each catch the other mid-write, and the reader printed
///
///   dfn INVALID (Verification: FAILED)
///
/// about a token that was sound. Fourteen of thirty concurrent `verify`
/// runs said so, about a different day each time -- in the one line of a
/// report that exists to be believed.
///
/// A file here is never changed once it has a name. It is named for what
/// it holds, so two versions of this program with two different roots do
/// not take turns overwriting each other; it is written whole under a name
/// of that one write's own and only then renamed, so a reader sees all of
/// it or none; and where the right file is already there it is left alone.
/// It is still read back and compared first: the directory is the user's
/// to write, and a root is not something to take on a filename's word.
fn pinned_root(dir: &Path, tsa: &Tsa) -> Result<PathBuf> {
    let digest: String = Sha256::digest(tsa.ca)
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = dir.join(format!("{}-ca-{digest}.pem", tsa.name));
    if fs::read(&path).is_ok_and(|held| held == tsa.ca) {
        return Ok(path);
    }

    // A name of this write's own: the process, and a count within it,
    // since several threads of one process check tokens at once.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let staging = dir.join(format!(
        "{}-ca-{digest}.{}-{}.part",
        tsa.name,
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::write(&staging, tsa.ca).with_context(|| format!("writing {}", staging.display()))?;
    if let Err(e) = fs::rename(&staging, &path) {
        let _ = fs::remove_file(&staging);
        return Err(e).with_context(|| format!("naming {}", path.display()));
    }
    Ok(path)
}

/// Scratch space for request files and the unpacked root.
///
/// In `$XDG_STATE_HOME`, never the log tree: the tree is replicated, and
/// transient files there would sync to other hosts.
fn scratch_dir() -> Result<PathBuf> {
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .context("neither XDG_STATE_HOME nor HOME is set")?
        .join("media-logger/anchor");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn run(cmd: &mut Command, what: &str) -> Result<String> {
    let out = cmd.output().with_context(|| what.to_owned())?;
    if !out.status.success() {
        bail!(
            "{what}: {}",
            first_line(&String::from_utf8_lossy(&out.stderr))
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The root on disk is put there once and never written over.
    ///
    /// Writing it in place before every check emptied it under whoever
    /// else was reading it, and a sound token came back INVALID. What
    /// cannot be seen half-written is a file that is only ever renamed
    /// into place, so that is what is pinned: the same file is left alone,
    /// and a damaged one is replaced by a new file, not refilled.
    #[test]
    fn the_pinned_root_is_never_rewritten_in_place() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let tsa = &TSAS[0];
        let inode = |p: &Path| fs::metadata(p).unwrap().ino();

        let first = pinned_root(dir.path(), tsa).unwrap();
        assert_eq!(fs::read(&first).unwrap(), tsa.ca);
        let made = inode(&first);

        let again = pinned_root(dir.path(), tsa).unwrap();
        assert_eq!(again, first);
        assert_eq!(inode(&again), made, "a root already there is not touched");

        // Cut short, as a full disk or an interrupted write would leave it.
        // It is held open here the way a reader would hold it.
        fs::write(&first, &tsa.ca[..tsa.ca.len() / 2]).unwrap();
        let reader = fs::File::open(&first).unwrap();
        let mended = pinned_root(dir.path(), tsa).unwrap();
        assert_eq!(fs::read(&mended).unwrap(), tsa.ca);
        assert_ne!(
            inode(&mended),
            reader.metadata().unwrap().ino(),
            "replaced by a whole file, not filled back in under a reader"
        );
        // Nothing is left behind but the root itself.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// A report that only says "not yet stamped" reads the same after an
    /// hour offline and after a fortnight broken. It said exactly that
    /// for three days while every stamp was being granted and dropped.
    #[test]
    fn a_pending_day_says_how_long_it_has_been_pending() {
        // A fixed offset rather than a named zone: the build sandbox has
        // no time zone database, and nothing here turns on a transition.
        // Not UTC either, so a day boundary taken in the wrong zone still
        // shows up.
        let tz = TimeZone::fixed(jiff::tz::Offset::constant(2));
        let at = |d: i8, h: i8| {
            date(2026, 10, d)
                .at(h, 0, 0, 0)
                .to_zoned(tz.clone())
                .unwrap()
        };
        let day = date(2026, 10, 22);

        // Measured from the end of the day it covers, not from its start.
        assert_eq!(waited(day, &at(23, 9)).as_deref(), Some("9 hours"));
        assert_eq!(waited(day, &at(25, 0)).as_deref(), Some("2 days"));
        // Still open, so nothing is owed yet and nothing is claimed.
        assert_eq!(waited(day, &at(22, 23)), None);
        // Hours up to two days, because "1 day" hides most of a day.
        assert_eq!(waited(day, &at(24, 20)).as_deref(), Some("44 hours"));

        // The sentence names what is missing, and says nothing when the
        // day is fully stamped.
        assert_eq!(
            awaiting(day, &["dfn", "digicert"], &at(25, 0)).as_deref(),
            Some("awaiting dfn and digicert for 2 days")
        );
        assert_eq!(
            awaiting(day, &["digicert"], &at(23, 9)).as_deref(),
            Some("awaiting digicert for 9 hours")
        );
        assert_eq!(awaiting(day, &[], &at(25, 0)), None);
    }

    /// A token is put beside its day file whole, or not at all.
    ///
    /// It used to get there by renaming it in from the scratch directory,
    /// which cannot cross a mount point -- and under `StateDirectory=` the
    /// scratch directory is one. Every stamp was granted, verified, and
    /// then dropped on the floor with EXDEV, once an hour, for days.
    #[test]
    fn a_token_is_stored_without_renaming_across_a_mount() {
        let dir = tempfile::tempdir().unwrap();
        let (scratch, tree) = (dir.path().join("state"), dir.path().join("tree"));
        fs::create_dir_all(&scratch).unwrap();
        fs::create_dir_all(&tree).unwrap();

        let from = scratch.join("dfn.tsr");
        fs::write(&from, b"a token").unwrap();
        let into = tree.join("2026-10-22.dfn.tsr");
        store(&from, &into).unwrap();

        assert_eq!(fs::read(&into).unwrap(), b"a token");
        assert!(!from.exists(), "the scratch copy is left behind");
        assert!(
            !tree.join("2026-10-22.dfn.tsr.part").exists(),
            "the staging file is left behind, where a reader could mistake it"
        );

        // And a second stamp of the same day replaces the first rather
        // than failing on a name already taken.
        fs::write(&from, b"a later token").unwrap();
        store(&from, &into).unwrap();
        assert_eq!(fs::read(&into).unwrap(), b"a later token");
    }

    #[test]
    fn parses_a_padded_day() {
        // openssl space-pads single-digit days.
        let text = "Status: Granted.\nTime stamp: Oct  2 00:36:14 2026 GMT\n";
        assert_eq!(parse_stamp_time(text).unwrap(), "2026-10-02T00:36:14Z");
    }

    #[test]
    fn parses_a_two_digit_day() {
        let text = "Time stamp: Dec 31 23:59:59 2026 GMT\n";
        assert_eq!(parse_stamp_time(text).unwrap(), "2026-12-31T23:59:59Z");
    }

    #[test]
    fn pending_skips_today_and_stamped_days() {
        let dir = tempfile::tempdir().unwrap();
        let month = dir.path().join("2026-10");
        fs::create_dir_all(&month).unwrap();
        for d in ["2026-10-20", "2026-10-21", "2026-10-22"] {
            fs::write(month.join(format!("{d}.jsonl")), b"{}\n").unwrap();
        }
        // The 20th is fully stamped; the 22nd is today and still open.
        for tsa in TSAS {
            fs::write(month.join(format!("2026-10-20.{}.tsr", tsa.name)), b"x").unwrap();
        }

        let work = pending(dir.path(), "2026-10-22").unwrap();
        let days: Vec<_> = work
            .iter()
            .map(|(p, _)| p.file_stem().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(days, vec!["2026-10-21"]);
        assert_eq!(work[0].1.len(), TSAS.len());
    }
}
