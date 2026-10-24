//! Consecutive spans of one media, shown as one row.
//!
//! The reading formats group what the data formats keep apart.
//!
//! A player that pauses for a fifth of a second between two voice messages
//! has not stopped, but it has ended a span. Telegram does it between every
//! message and Firefox does it about once a minute, so twelve minutes came
//! out as twenty-five rows, most of them a second long. Every one of those
//! breaks is real and the spans keep all of them -- csv, xlsx and json still
//! carry one row per span, because that is what those formats are for.
//!
//! But text, html and pdf are read, and what a reader is asking is how long
//! the sound filled the room. So those three group consecutive spans of one
//! media into one row. Nothing is smoothed away by it: a row built from more
//! than one span prints how many and how far apart they were, so the silence
//! it closed over is on the page beside it.

use super::*;

/// Consecutive spans of one media, shown as one row.
pub(super) struct Stretch<'a> {
    /// In start order, never empty.
    spans: Vec<&'a Span>,
}

impl<'a> Stretch<'a> {
    /// The span the row takes everything it does not pool from: player,
    /// media, output, host.
    pub(super) fn head(&self) -> &'a Span {
        self.spans[0]
    }

    pub(super) fn start(&self) -> &'a Zoned {
        &self.head().start
    }

    pub(super) fn end(&self) -> &'a Zoned {
        self.spans
            .iter()
            .map(|s| &s.end)
            .max()
            .expect("never empty")
    }

    pub(super) fn duration(&self) -> String {
        human_secs(secs_between(self.start(), self.end()).max(0))
    }

    pub(super) fn levels(&self) -> Option<crate::query::Levels> {
        crate::query::Levels::pooled(self.spans.iter().copied())
    }

    /// Summed over the spans, not measured across the extent: the silences
    /// the row closed over were silent in quiet hours too.
    pub(super) fn quiet_seconds(&self, quiet: &QuietHours) -> i64 {
        self.spans.iter().map(|s| quiet_seconds(s, quiet)).sum()
    }

    /// Seconds the sound was actually on, which is not the row's extent:
    /// a row made of several spans closed over the silences between them,
    /// and those were silent.
    ///
    /// The same measure the quiet-hours figure beside it uses, so the two
    /// are comparable and the smaller can never exceed the larger.
    pub(super) fn played(&self) -> i64 {
        self.spans
            .iter()
            .map(|s| secs_between(&s.start, &s.end).max(0))
            .sum()
    }

    pub(super) fn during(&self, quiet: &QuietHours) -> &'static str {
        if self.quiet_seconds(quiet) > 0 {
            "quiet hours"
        } else {
            "other times"
        }
    }

    pub(super) fn sustained(&self) -> Option<f64> {
        self.spans
            .iter()
            .filter_map(|s| s.sustained)
            .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v))))
    }

    /// How the row was assembled, where it was assembled from more than
    /// one span: the count, and the range of silences bridged.
    pub(super) fn parts(&self) -> Option<String> {
        if self.spans.len() < 2 {
            return None;
        }
        let gaps: Vec<i64> = self
            .spans
            .windows(2)
            .map(|w| millis_between(&w[0].end, &w[1].start).max(0))
            .collect();
        let (lo, hi) = (*gaps.iter().min()?, *gaps.iter().max()?);
        let n = self.spans.len();
        Some(if lo == hi {
            format!("in {n} parts, {} apart", brief(lo))
        } else {
            format!("in {n} parts, {}–{} apart", brief(lo), brief(hi))
        })
    }
}

/// A gap, at the precision worth reading.
///
/// Two voice messages are a fifth of a second apart and `human_secs` calls
/// that "0s" -- which is the very row this grouping exists to stop printing.
pub(super) fn brief(ms: i64) -> String {
    if ms < 1000 {
        format!("{:.2}s", ms as f64 / 1000.0)
    } else {
        human_secs(ms / 1000)
    }
}

/// The longest silence a row may close over.
///
/// Deliberately not `Coverage::gap`, the tolerance the level series uses.
/// That one is learnt from the log because it answers a question about the
/// log: samples arrive at the beat, so a hole shorter than a few beats may
/// be a beat that did not land rather than a room that went quiet. Playback
/// is evented to the millisecond, so a hole here is known silence, and
/// borrowing a tolerance built for unknown silence let one row swallow
/// three minutes of nothing and call it playing.
///
/// Half a minute is the longest pause that can fall between two parts of
/// one listening session -- a track changing, a voice message queueing, a
/// page reloading -- without the room having gone quiet in any sense a
/// complaint would recognise. Past it the row would be claiming sound that
/// was not there, and the spans go back to being separate rows.
pub(super) const JOIN: i64 = 30;

/// Groups the report's spans into the rows a reading format shows.
///
/// Two spans join when they are the same media on the same player and host,
/// on the same day, and the silence between them is shorter than `JOIN`.
///
/// Different media interleave freely in a window, so a series is followed
/// through the spans between it rather than along the vector: Telegram
/// playing underneath a browser must not stop the browser's row.
pub(super) fn stretches<'a>(report: &'a Report) -> Vec<Stretch<'a>> {
    let key = |s: &Span| {
        (
            s.host.clone(),
            s.player.clone(),
            s.label(),
            s.url.clone(),
            s.sink.clone(),
        )
    };
    let mut open: Vec<Stretch<'a>> = Vec::new();
    for span in &report.spans {
        // The most recent matching row, not the first: one media can have
        // two rows in a window, and only the later one is still open.
        let joins = open.iter_mut().rev().find(|st| {
            let last = st.spans[st.spans.len() - 1];
            key(last) == key(span)
                && last.end.date() == span.start.date()
                && secs_between(&last.end, &span.start) <= JOIN
        });
        match joins {
            Some(st) => st.spans.push(span),
            None => open.push(Stretch { spans: vec![span] }),
        }
    }
    open.sort_by(|a, b| a.start().cmp(b.start()));
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row is the block of time the sound filled, not one row per
    /// bounce of the player's own status.
    ///
    /// Telegram emits Paused and Playing a tenth of a second apart between
    /// every voice message, and Firefox does it about once a minute. Each
    /// is a real break and each ends a span, which turned twelve minutes
    /// of one window into twenty-five rows, most of them a second long.
    /// The spans are still all there -- csv and xlsx print every one -- and
    /// the row that groups them says how many it holds.
    #[test]
    fn a_player_bouncing_through_paused_is_one_row_that_says_so() {
        let tz = jiff::tz::TimeZone::UTC;
        let dir = tempfile::tempdir().unwrap();
        let at = |secs: i64| {
            jiff::civil::date(2026, 10, 20)
                .at(14, 0, 0, 0)
                .to_zoned(tz.clone())
                .unwrap()
                .checked_add(jiff::Span::new().seconds(secs))
                .unwrap()
        };
        let (mut log, _) = crate::log::Log::resume(dir.path(), "h", &at(0)).unwrap();
        let beat = |status: &str| {
            crate::event::Event::Playback(Box::new(crate::event::Playing {
                state: crate::event::PlayerState {
                    player: "TelegramDesktop".to_owned(),
                    identity: Some("Telegram Desktop".to_owned()),
                    status: status.to_owned(),
                    player_volume: Some(1.0),
                    position_us: Some(0),
                    track: crate::event::Track {
                        title: Some("Juanpe".to_owned()),
                        trackid: Some("/org/desktop_app/track/0".to_owned()),
                        ..crate::event::Track::default()
                    },
                },
                sink: None,
            }))
        };
        // Four messages, each ended by a bounce through Paused and
        // followed by the next. A span ends at the last moment the player
        // was seen playing, so the silences between them are seven
        // seconds -- well inside JOIN.
        let message = |log: &mut crate::log::Log, from: i64| {
            log.append(&beat("Playing"), &at(from)).unwrap();
            log.append(&beat("Playing"), &at(from + 3)).unwrap();
            log.append(&beat("Paused"), &at(from + 5)).unwrap();
        };
        for i in 0..4 {
            message(&mut log, i * 10);
        }
        // ...then a silence no listening session survives, and a fifth.
        message(&mut log, 400);
        drop(log);

        let window = crate::query::parse_when("2026-10-20", &at(0)).unwrap();
        let report = crate::query::build(dir.path(), &["h".to_owned()], window).unwrap();
        assert_eq!(report.spans.len(), 5, "every break is still a span");

        let rows = stretches(&report);
        assert_eq!(rows.len(), 2, "one row per stretch of listening");
        assert_eq!(rows[0].spans.len(), 4);
        assert_eq!(
            rows[0].parts().as_deref(),
            Some("in 4 parts, 7s apart"),
            "the row names the silences it closed over"
        );
        // Its extent runs to the last part, not only through the first.
        assert_eq!(rows[0].start(), &at(0));
        assert_eq!(rows[0].end(), &at(33));
        // The silence past JOIN was not closed over.
        assert_eq!(rows[1].spans.len(), 1);
        assert_eq!(rows[1].parts(), None, "a single span is not 'in 1 parts'");
    }
}
