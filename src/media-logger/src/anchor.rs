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
use jiff::civil::date;
use jiff::tz::TimeZone;

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
        ca: include_bytes!("../../../anchors/dfn.pem"),
    },
    Tsa {
        name: "digicert",
        url: "http://timestamp.digicert.com",
        ca: include_bytes!("../../../anchors/digicert.pem"),
    },
];

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
pub fn stamp(day: &Path, tsa: &Tsa) -> Result<Stamped> {
    let scratch = scratch_dir()?;
    let request = scratch.join(format!("{}.tsq", tsa.name));
    let reply = scratch.join(format!("{}.tsr", tsa.name));
    let ca = scratch.join(format!("{}-ca.pem", tsa.name));
    fs::write(&ca, tsa.ca).context("writing the pinned root")?;

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

    fs::rename(&reply, token_path(day, tsa)).context("storing the token")?;
    Ok(Stamped {
        at: parse_stamp_time(&text)?,
    })
}

pub enum TokenState {
    Valid { at: String },
    Invalid(String),
    Missing,
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
            let ca = scratch.join(format!("{}-ca.pem", tsa.name));
            if let Err(e) = fs::write(&ca, tsa.ca) {
                return (tsa, TokenState::Invalid(e.to_string()));
            }
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
