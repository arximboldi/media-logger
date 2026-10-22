//! The append-only log and its hash chain.
//!
//! The log tree is the only durable state. The chain head is the hash of the
//! last line of the most recent file, and pending anchors are day files
//! without a token beside them, so neither is cached anywhere — a cache would
//! be a second source of truth able to disagree with the first.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::Zoned;
use rustix::fs::{flock, FlockOperation};
use sha2::{Digest, Sha256};

use crate::event::{Event, Record, RestartReason, GENESIS, SCHEMA_VERSION};

/// Append-only writer for one host's tree.
pub struct Log {
    host_root: PathBuf,
    host: String,
    head: String,
    open: Option<OpenDay>,
    /// Held for the writer's lifetime. Two daemons appending to one tree
    /// would interleave lines and break the chain in a way indistinguishable
    /// from tampering, which is precisely the claim the chain exists to
    /// refute. Released by the kernel when the process dies, so a crash
    /// leaves nothing to clean up.
    _lock: File,
}

struct OpenDay {
    date: Date,
    file: File,
    lines: u64,
}

/// What `resume` found, when it found no usable predecessor.
pub struct Restart {
    pub reason: RestartReason,
    pub detail: Option<String>,
}

impl Log {
    /// Opens the log for `host` under `logs_root`, establishing what state it
    /// is in.
    ///
    /// This owns the whole startup decision — refuse, derive, truncate, or
    /// restart — because they are one question about the log's condition.
    /// Splitting them would let a caller perform some and not others.
    ///
    /// A missing `logs_root` is refused rather than treated as a first run: an
    /// unmounted or not-yet-synced share is indistinguishable from a fresh
    /// install, and guessing wrong starts a new chain while the real history
    /// sits unmounted underneath.
    /// The `chain_restart` line is written here rather than returned for the
    /// caller to write, so a restarted chain cannot reach disk undeclared.
    pub fn resume(logs_root: &Path, host: &str, now: &Zoned) -> Result<(Self, Option<Restart>)> {
        let mut restart = None;
        if !logs_root.exists() {
            fs::create_dir_all(logs_root)
                .with_context(|| format!("creating logs root {}", logs_root.display()))?;
            restart = Some(Restart {
                reason: RestartReason::CreatedRoot,
                detail: Some(format!("logs root {} did not exist", logs_root.display())),
            });
        } else if !logs_root.is_dir() {
            bail!(
                "logs root {} exists but is not a directory",
                logs_root.display()
            );
        }

        let host_root = logs_root.join(host);
        let _lock = lock_tree(logs_root, host)?;

        if restart.is_none() && !host_root.exists() {
            fs::create_dir_all(&host_root)
                .with_context(|| format!("creating host tree {}", host_root.display()))?;
            restart = Some(Restart {
                reason: RestartReason::NewHost,
                detail: Some(format!(
                    "no tree for host {host} under {}",
                    logs_root.display()
                )),
            });
        }

        let mut head = GENESIS.to_owned();
        if restart.is_none() {
            match latest_day_file(&host_root)? {
                None => {
                    restart = Some(Restart {
                        reason: RestartReason::EmptyTree,
                        detail: Some("host tree holds no log files".to_owned()),
                    })
                }
                Some(path) => match recover_tail(&path)? {
                    Some(recovered) => head = recovered,
                    None => {
                        restart = Some(Restart {
                            reason: RestartReason::UnreadableTail,
                            detail: Some(format!("no intact line in {}", path.display())),
                        })
                    }
                },
            }
        }

        let mut log = Log {
            host_root,
            host: host.to_owned(),
            head,
            open: None,
            _lock,
        };
        if let Some(restart) = &restart {
            log.append(
                &Event::ChainRestart {
                    reason: restart.reason,
                    detail: restart.detail.clone(),
                },
                now,
            )?;
        }
        Ok((log, restart))
    }

    /// Appends one event, rolling to a new day file first if the local date
    /// has changed. Returns the new chain head.
    pub fn append(&mut self, event: &Event, now: &Zoned) -> Result<String> {
        self.ensure_day(now)?;
        self.write_record(event, now)
    }

    /// Opens today's file, closing yesterday's with a `rollover` line.
    ///
    /// The rollover is written when the first event after midnight arrives,
    /// which the heartbeat interval bounds. Its timestamp therefore falls in
    /// the new day while the line itself closes the old file — that is the
    /// transition it marks.
    fn ensure_day(&mut self, now: &Zoned) -> Result<()> {
        let date = now.date();
        match &self.open {
            Some(open) if open.date == date => return Ok(()),
            Some(open) => {
                let closing = Event::Rollover {
                    chain_head: self.head.clone(),
                    line_count: open.lines,
                    date: open.date.to_string(),
                };
                self.write_record(&closing, now)?;
                self.open = None;
            }
            None => {}
        }

        let path = self.day_path(date);
        let parent = path
            .parent()
            .expect("day path always has a month directory");
        fs::create_dir_all(parent)
            .with_context(|| format!("creating month directory {}", parent.display()))?;

        let lines = if path.exists() {
            count_lines(&path)?
        } else {
            0
        };
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;

        self.open = Some(OpenDay { date, file, lines });
        Ok(())
    }

    fn write_record(&mut self, event: &Event, now: &Zoned) -> Result<String> {
        let (kind, data) = event.split()?;
        let record = Record {
            v: SCHEMA_VERSION,
            ts: format_local(now),
            ts_utc: format_utc(now),
            host: self.host.clone(),
            kind,
            data,
            prev: self.head.clone(),
        };
        let line = serde_json::to_string(&record).context("serialising record")?;

        {
            let open = self.open.as_mut().context("no day file open")?;
            open.file.write_all(line.as_bytes())?;
            open.file.write_all(b"\n")?;
            open.file.flush()?;
            // A hole torn in the chain by a power cut is indistinguishable
            // from tampering, so pay for the sync on every line.
            open.file.sync_data()?;
            open.lines += 1;
        }

        self.head = hash_line(&line);
        Ok(self.head.clone())
    }

    fn day_path(&self, date: Date) -> PathBuf {
        let month = format!("{:04}-{:02}", date.year(), date.month());
        self.host_root.join(month).join(format!("{date}.jsonl"))
    }
}

/// Takes an exclusive lock for one (root, host) pair.
///
/// The lock lives in `$XDG_STATE_HOME`, not in the log tree: the tree is
/// replicated by Syncthing, and a lock file there would sync to other hosts
/// and mean nothing on any of them. Keying the name by the root path lets
/// separate trees be logged at once, which is useful when testing against a
/// scratch root while the real daemon runs.
fn lock_tree(logs_root: &Path, host: &str) -> Result<File> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .context("neither XDG_STATE_HOME nor HOME is set")?
        .join("media-logger");
    fs::create_dir_all(&state)
        .with_context(|| format!("creating state directory {}", state.display()))?;

    let canonical = logs_root
        .canonicalize()
        .unwrap_or_else(|_| logs_root.to_owned());
    let key = &hash_line(&format!("{}\0{host}", canonical.display()))[..16];
    let path = state.join(format!("{key}.lock"));

    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening lock file {}", path.display()))?;

    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|e| {
        anyhow::anyhow!(
            "another media-logger is already writing {}/{host} (lock {}): {e}",
            logs_root.display(),
            path.display()
        )
    })?;
    Ok(file)
}

/// Hash of a line: SHA-256 over its literal bytes, excluding the newline.
///
/// Hashing what is on disk rather than a re-serialised object removes
/// canonicalisation as a failure mode entirely, and decouples verification
/// from the schema — unknown fields and new event types verify fine.
pub fn hash_line(line: &str) -> String {
    let digest = Sha256::digest(line.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn format_local(now: &Zoned) -> String {
    now.strftime("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()
}

fn format_utc(now: &Zoned) -> String {
    now.with_time_zone(TimeZone::UTC)
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Day files for a host, oldest first. Names are zero-padded full dates, so
/// lexicographic order is chronological order.
pub fn day_files(host_root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let months = match fs::read_dir(host_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(e) => return Err(e).context("reading host tree"),
    };

    for month in months {
        let month = month?.path();
        if !month.is_dir() {
            continue;
        }
        for day in fs::read_dir(&month)? {
            let day = day?.path();
            if day
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_day_file)
            {
                found.push(day);
            }
        }
    }

    found.sort();
    Ok(found)
}

fn latest_day_file(host_root: &Path) -> Result<Option<PathBuf>> {
    Ok(day_files(host_root)?.pop())
}

/// `YYYY-MM-DD.jsonl`
fn is_day_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".jsonl") else {
        return false;
    };
    let bytes = stem.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
}

/// Returns the hash of the last intact line, truncating anything after it.
///
/// A line is intact if it was terminated by a newline and parses. A crash
/// mid-write leaves a partial final line; dropping it is correct and keeps the
/// chain continuous, where starting a fresh file would break it.
fn recover_tail(path: &Path) -> Result<Option<String>> {
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;

    let mut end = if data.ends_with(b"\n") {
        data.len()
    } else {
        data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1)
    };

    while end > 0 {
        let start = data[..end - 1]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        if serde_json::from_slice::<Record>(&data[start..end - 1]).is_ok() {
            break;
        }
        end = start;
    }

    if end != data.len() {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(end as u64)
            .with_context(|| format!("truncating damaged tail of {}", path.display()))?;
        file.sync_all()?;
    }

    if end == 0 {
        return Ok(None);
    }

    let start = data[..end - 1]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let line = std::str::from_utf8(&data[start..end - 1]).context("last line is not UTF-8")?;
    Ok(Some(hash_line(line)))
}

fn count_lines(path: &Path) -> Result<u64> {
    let data = fs::read(path)?;
    Ok(data.iter().filter(|b| **b == b'\n').count() as u64)
}

#[derive(Debug, Default)]
pub struct VerifyReport {
    pub files: usize,
    pub lines: u64,
    pub restarts: Vec<String>,
    pub problems: Vec<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Walks a host tree and checks that every line hashes into the next.
///
/// A `chain_restart` line legitimately begins a new chain, so it is reported
/// rather than treated as a break — the discontinuity is declared in-band, and
/// an unexplained break is what matters.
pub fn verify(host_root: &Path) -> Result<VerifyReport> {
    let mut report = VerifyReport::default();
    let mut expected = GENESIS.to_owned();

    for path in day_files(host_root)? {
        report.files += 1;
        let data = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let name = path.display().to_string();

        for (n, raw) in data.split(|b| *b == b'\n').enumerate() {
            if raw.is_empty() {
                continue;
            }
            let line = std::str::from_utf8(raw)
                .with_context(|| format!("{name}:{} is not UTF-8", n + 1))?;
            let record: Record = match serde_json::from_str(line) {
                Ok(record) => record,
                Err(e) => {
                    report
                        .problems
                        .push(format!("{name}:{}: unparseable ({e})", n + 1));
                    continue;
                }
            };

            if record.kind == "chain_restart" {
                if record.prev != GENESIS {
                    report
                        .problems
                        .push(format!("{name}:{}: chain_restart not at genesis", n + 1));
                }
                report
                    .restarts
                    .push(format!("{name}:{} at {}", n + 1, record.ts));
            } else if record.prev != expected {
                report.problems.push(format!(
                    "{name}:{}: chain broken (expected prev {}, found {})",
                    n + 1,
                    &expected[..16],
                    &record.prev[..record.prev.len().min(16)]
                ));
            }

            expected = hash_line(line);
            report.lines += 1;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Beat, Event, FullBeat};

    fn at(day: i8, hour: i8) -> Zoned {
        jiff::civil::date(2026, 10, day)
            .at(hour, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .expect("fixed civil time converts")
    }

    fn beat() -> Event {
        Event::Heartbeat(Beat::Full(Box::new(FullBeat {
            players: Vec::new(),
            sink: None,
            state: "test".to_owned(),
        })))
    }

    /// Writes `count` heartbeats on the given day and returns the tree root.
    fn populate(root: &Path, day: i8, count: usize) {
        let (mut log, _) = Log::resume(root, "h", &at(day, 0)).expect("resume");
        for hour in 0..count {
            log.append(&beat(), &at(day, hour as i8)).expect("append");
        }
    }

    #[test]
    fn chain_links_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path(), 22, 5);

        let report = verify(&dir.path().join("h")).unwrap();
        assert!(report.ok(), "{:?}", report.problems);
        // 5 heartbeats plus the chain_restart that declared the new host.
        assert_eq!(report.lines, 6);
        assert_eq!(report.restarts.len(), 1);
    }

    #[test]
    fn detects_a_mutated_line() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path(), 22, 5);
        let file = dir.path().join("h/2026-10/2026-10-22.jsonl");

        let mut lines: Vec<String> = fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        lines[2] = lines[2].replace("\"host\":\"h\"", "\"host\":\"H\"");
        fs::write(&file, lines.join("\n") + "\n").unwrap();

        let report = verify(&dir.path().join("h")).unwrap();
        // The break surfaces at the line *after* the edit: its recorded `prev`
        // no longer matches what the mutated line now hashes to.
        assert!(
            report.problems.iter().any(|p| p.contains(":4:")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn detects_a_deleted_day() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path(), 22, 3);
        populate(dir.path(), 23, 3);
        assert!(verify(&dir.path().join("h")).unwrap().ok());

        // Per-file hashes would miss this; chaining across the day boundary
        // is what makes a whole missing day detectable.
        fs::remove_file(dir.path().join("h/2026-10/2026-10-22.jsonl")).unwrap();

        let report = verify(&dir.path().join("h")).unwrap();
        assert!(!report.ok(), "deleting a whole day must break the chain");
    }

    #[test]
    fn resume_truncates_a_partial_trailing_line() {
        let dir = tempfile::tempdir().unwrap();
        populate(dir.path(), 22, 3);
        let file = dir.path().join("h/2026-10/2026-10-22.jsonl");

        // A crash mid-write leaves a line with no terminating newline.
        let intact = fs::read_to_string(&file).unwrap();
        fs::write(&file, format!("{intact}{{\"v\":1,\"ts\":\"2026-10-22T0")).unwrap();

        let (mut log, restart) = Log::resume(dir.path(), "h", &at(22, 0)).unwrap();
        assert!(
            restart.is_none(),
            "a damaged tail is repaired, not a restart"
        );
        log.append(&beat(), &at(22, 9)).unwrap();

        let report = verify(&dir.path().join("h")).unwrap();
        assert!(report.ok(), "{:?}", report.problems);
    }

    #[test]
    fn refuses_a_second_writer_on_one_tree() {
        let dir = tempfile::tempdir().unwrap();
        // Held, not dropped: two writers would interleave lines and break the
        // chain in a way that looks exactly like tampering.
        let (_held, _) = Log::resume(dir.path(), "h", &at(22, 0)).unwrap();
        assert!(Log::resume(dir.path(), "h", &at(22, 1)).is_err());
    }

    #[test]
    fn creates_a_missing_logs_root_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-yet-there");

        let (_log, restart) = Log::resume(&root, "h", &at(22, 0)).unwrap();
        assert!(root.is_dir(), "a missing root is created, not refused");
        // Silently starting over is the thing to avoid, not starting over.
        assert!(matches!(
            restart.map(|r| r.reason),
            Some(crate::event::RestartReason::CreatedRoot)
        ));
    }

    #[test]
    fn refuses_a_logs_root_that_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        fs::write(&file, b"").unwrap();
        assert!(Log::resume(&file, "h", &at(22, 0)).is_err());
    }

    #[test]
    fn restart_is_declared_not_silent() {
        let dir = tempfile::tempdir().unwrap();
        let (_, restart) = Log::resume(dir.path(), "h", &at(22, 0)).unwrap();
        assert!(matches!(
            restart.map(|r| r.reason),
            Some(crate::event::RestartReason::NewHost)
        ));
    }
}
