//! NFR-55 — the local diagnostic log, and where D-35's crash report sits beside it.
//!
//! # Redaction by construction, not by a pass
//!
//! NFR-55 forbids message content, addresses, subjects, domains and credential material in a
//! local log. A log that accepted text and scrubbed it would have the property D-35 rejects for
//! minidumps: its compliance could not be shown, because the absence of a subject from a
//! formatted string is not something a scrubber can demonstrate. So **nothing here accepts
//! text**. An [`Event`] is built from a [`Subsystem`], a local account identifier, and
//! [`Word`]s — short `'static` tokens restricted to lowercase ASCII letters, digits and `_` —
//! plus counts and durations. An address needs `@`, a domain needs `.`, a subject needs a space
//! or a capital, and a bearer token needs characters outside that set; none of them can be a
//! `Word`, and the file grammar ([`line_is_permitted`]) is checked on every line before it is
//! written, so a word that somehow bypassed its constructor is refused rather than logged.
//!
//! # Bounded, oldest first, and retained for a stated period
//!
//! Two segments: the one being written and the one before it. A segment is closed and becomes
//! the previous one when appending would take it past half of [`BUDGET_BYTES`] (L-32), which
//! discards whatever the previous one held — so the file set never exceeds the budget and what
//! goes is always the oldest. The same rotation happens when the current segment's first line
//! is half of [`RETENTION`] (L-33) old, and the previous segment is deleted once its last line
//! is half of it old, which together keep every line for no longer than the retention period.
//! Both checks run whenever the log is opened or written, and [`Log::prune`] runs them for a
//! caller whose scheduler fires without anything to record.
//!
//! # The sink is a file, deliberately not the platform's unified log
//!
//! NFR-55 records why: the unified log is system-wide, readable by anything with the right
//! entitlement, and its redaction is opt-out per interpolation. A file in the container is
//! readable only by Sift and by the user, who attaches it to a report themselves.

use sift_subsystem::Subsystem;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The directory under the container that holds the log and the crash report.
///
/// The macOS shell names the same directory for the crash report it writes and for the Help
/// menu item that reveals both; the two spellings change together.
pub const DIRECTORY: &str = "Diagnostics";

/// The segment being written.
pub const CURRENT: &str = "diagnostic.log";

/// The segment before it — the next thing to be evicted.
pub const PREVIOUS: &str = "diagnostic.log.1";

/// D-114's single report. Only the most recent crash is kept, so the store is bounded by
/// construction rather than by a budget.
pub const CRASH_REPORT: &str = "crash-report.txt";

/// L-32 — the log's byte budget, across both segments.
pub const BUDGET_BYTES: u64 = 4 * 1024 * 1024;

/// L-33 — how long a line is retained.
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The most fields one event carries. Fixed so an event is a value with no allocation of its
/// own, and so a line has a length bound the budget arithmetic can rely on.
pub const MAX_FIELDS: usize = 6;

/// The longest a [`Word`] may be.
pub const MAX_WORD: usize = 32;

/// A token the log is permitted to write: one to [`MAX_WORD`] bytes of `a`–`z`, `0`–`9` and
/// `_`, with a `'static` lifetime.
///
/// The lifetime is what keeps runtime text out: a subject parsed from a message is never
/// `'static`. The character set is what keeps it out even if one were leaked into a `'static`
/// string on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word(&'static str);

impl Word {
    /// A word, refused at compile time when used in a `const` and at run time otherwise.
    ///
    /// # Panics
    ///
    /// If `text` is not a permitted word. Every call site passes a literal, so the panic is a
    /// defect caught by the first test that reaches it, never a runtime condition.
    #[must_use]
    pub const fn new(text: &'static str) -> Self {
        assert!(is_word(text), "not a permitted log word");
        Self(text)
    }

    /// The same check, as an answer rather than a panic.
    #[must_use]
    pub const fn try_new(text: &'static str) -> Option<Self> {
        if is_word(text) {
            Some(Self(text))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

const fn is_word(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_WORD {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        if !matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'_') {
            return false;
        }
        i += 1;
    }
    true
}

/// How a sync or a flush ended, by class — never by the provider's own words.
///
/// The classes are D-88's transient versus non-transient, the throttle D-87 schedules around,
/// and the cursor invalidation D-82 treats as progress. The provider's reason text is exactly
/// what this type exists not to carry: it quotes addresses and folder names back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Transient,
    NonTransient,
    Throttled,
    CursorInvalidated,
    /// The request went out and no answer came back.
    Unknown,
}

impl Outcome {
    #[must_use]
    pub const fn word(self) -> Word {
        Word::new(match self {
            Self::Ok => "ok",
            Self::Transient => "transient",
            Self::NonTransient => "non_transient",
            Self::Throttled => "throttled",
            Self::CursorInvalidated => "cursor_invalidated",
            Self::Unknown => "unknown",
        })
    }
}

/// One field's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Word(Word),
    Count(u64),
    /// A duration, written in whole milliseconds with an `ms` suffix.
    Millis(u64),
}

/// One line of the log, before it is formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    subsystem: Subsystem,
    name: Word,
    /// **The local identifier**, never an address: D-83's identity is assigned on this machine
    /// and means nothing to anyone the log is sent to.
    account: Option<u128>,
    fields: [Option<(Word, Value)>; MAX_FIELDS],
}

impl Event {
    #[must_use]
    pub const fn new(subsystem: Subsystem, name: Word) -> Self {
        Self {
            subsystem,
            name,
            account: None,
            fields: [None; MAX_FIELDS],
        }
    }

    /// Attribute the event to an account, by its local identifier.
    #[must_use]
    pub const fn account(mut self, local_id: u128) -> Self {
        self.account = Some(local_id);
        self
    }

    /// Append a field.
    ///
    /// # Panics
    ///
    /// Past [`MAX_FIELDS`]. Every event is assembled at a fixed call site, so this is a defect
    /// the first test through that site finds.
    #[must_use]
    pub fn field(mut self, key: Word, value: Value) -> Self {
        let slot = self
            .fields
            .iter_mut()
            .find(|f| f.is_none())
            .expect("an event carries at most MAX_FIELDS fields");
        *slot = Some((key, value));
        self
    }

    /// D-49 — an account's condition changed. The conditions are named by their own
    /// identifiers, which are words.
    #[must_use]
    pub fn condition(local_id: u128, from: Word, to: Word) -> Self {
        Self::new(Subsystem::Presentation, Word::new("condition"))
            .account(local_id)
            .field(Word::new("from"), Value::Word(from))
            .field(Word::new("to"), Value::Word(to))
    }

    /// A sync pass ended.
    #[must_use]
    pub fn sync(local_id: u128, outcome: Outcome, elapsed: Duration) -> Self {
        Self::new(Subsystem::Sync, Word::new("sync"))
            .account(local_id)
            .field(Word::new("outcome"), Value::Word(outcome.word()))
            .field(Word::new("elapsed"), Value::Millis(millis(elapsed)))
    }

    /// A flush of the mutation queue ended, with how many intents it issued.
    #[must_use]
    pub fn flush(local_id: u128, outcome: Outcome, issued: u64, elapsed: Duration) -> Self {
        Self::new(Subsystem::Mutations, Word::new("flush"))
            .account(local_id)
            .field(Word::new("outcome"), Value::Word(outcome.word()))
            .field(Word::new("issued"), Value::Count(issued))
            .field(Word::new("elapsed"), Value::Millis(millis(elapsed)))
    }

    /// D-93 — the governor entered a shed tier, by its ordinal.
    #[must_use]
    pub fn pressure(tier: u8) -> Self {
        Self::new(Subsystem::Governor, Word::new("pressure"))
            .field(Word::new("tier"), Value::Count(u64::from(tier)))
    }

    /// The process started, with D-35's core-dump attempt recorded as Q-20 asks.
    #[must_use]
    pub fn launch(core_dumps_disabled: bool) -> Self {
        Self::new(Subsystem::Runtime, Word::new("launch")).field(
            Word::new("core_dumps"),
            Value::Word(Word::new(if core_dumps_disabled {
                "disabled"
            } else {
                "enabled"
            })),
        )
    }

    /// A timing, for a stage that has no outcome of its own.
    #[must_use]
    pub fn timing(subsystem: Subsystem, what: Word, elapsed: Duration) -> Self {
        Self::new(subsystem, Word::new("timing"))
            .field(Word::new("what"), Value::Word(what))
            .field(Word::new("elapsed"), Value::Millis(millis(elapsed)))
    }

    /// Write this event as one line, `at` seconds since the epoch, onto `out`.
    pub fn format(&self, at: u64, out: &mut String) {
        use core::fmt::Write as _;
        // Writing to a String cannot fail.
        let _ = write!(out, "{at} {} {}", self.subsystem.name(), self.name.as_str());
        if let Some(account) = self.account {
            let _ = write!(out, " account={account:032x}");
        }
        for (key, value) in self.fields.iter().flatten() {
            let _ = match value {
                Value::Word(w) => write!(out, " {}={}", key.as_str(), w.as_str()),
                Value::Count(n) => write!(out, " {}={n}", key.as_str()),
                Value::Millis(n) => write!(out, " {}={n}ms", key.as_str()),
            };
        }
        out.push('\n');
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Whether a line is one this log may write: space-separated tokens of `a`–`z`, `0`–`9`, `_`
/// and `=`, the first all digits, ending in one newline.
///
/// Checked before every write, so the file's grammar is a property of the sink rather than a
/// hope about its callers. None of `@`, `.`, `/`, `+`, `-`, an uppercase letter or a second
/// space can appear, which is what rules out an address, a domain, a path, a subject and a
/// bearer token.
#[must_use]
pub fn line_is_permitted(line: &str) -> bool {
    let Some(body) = line.strip_suffix('\n') else {
        return false;
    };
    let mut tokens = body.split(' ');
    let Some(first) = tokens.next() else {
        return false;
    };
    if first.is_empty() || !first.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut count = 1;
    for token in tokens {
        count += 1;
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'='))
        {
            return false;
        }
    }
    count >= 3
}

fn epoch_seconds(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The log, open on its directory.
#[derive(Debug)]
pub struct Log {
    dir: PathBuf,
    budget: u64,
    retention: Duration,
    current: File,
    current_len: u64,
    /// Seconds since the epoch of the current segment's first line; `None` while it is empty.
    current_started: Option<u64>,
    current_written: Option<u64>,
    previous_len: u64,
    previous_written: Option<u64>,
    /// Reused across lines, so writing a line allocates nothing once warm. Tagged to the
    /// [`Logging`](Subsystem::Logging) row, which is the buffer that row accounts for.
    line: String,
}

impl Log {
    /// Open the log under `dir` with L-32's budget and L-33's retention.
    ///
    /// # Errors
    ///
    /// If the directory cannot be created or the current segment cannot be opened.
    pub fn open_default(dir: &Path, now: SystemTime) -> io::Result<Self> {
        Self::open(dir, BUDGET_BYTES, RETENTION, now)
    }

    /// Open the log under `dir`, creating it, and apply retention to whatever the last run left.
    ///
    /// # Errors
    ///
    /// If the directory cannot be created or the current segment cannot be opened.
    pub fn open(dir: &Path, budget: u64, retention: Duration, now: SystemTime) -> io::Result<Self> {
        sift_alloc::tagged(Subsystem::Logging, || {
            fs::create_dir_all(dir)?;
            let current_path = dir.join(CURRENT);
            let current = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&current_path)?;
            let meta = current.metadata()?;
            let current_len = meta.len();
            // A segment whose first line cannot be read is treated as already old, so it
            // rotates out rather than living forever under an unknown start.
            let (current_started, current_written) = if current_len == 0 {
                (None, None)
            } else {
                (
                    Some(first_timestamp(&current_path).unwrap_or(0)),
                    meta.modified().ok().map(epoch_seconds),
                )
            };
            let (previous_len, previous_written) = match fs::metadata(dir.join(PREVIOUS)) {
                Ok(m) => (m.len(), Some(m.modified().map_or(0, epoch_seconds))),
                Err(_) => (0, None),
            };
            let mut log = Self {
                dir: dir.to_path_buf(),
                budget,
                retention,
                current,
                current_len,
                current_started,
                current_written,
                previous_len,
                previous_written,
                line: String::with_capacity(256),
            };
            log.prune(now)?;
            Ok(log)
        })
    }

    /// Record one event at `now`.
    ///
    /// # Errors
    ///
    /// If the write, or a rotation it needed, failed. The caller decides whether a log it
    /// cannot write is worth reporting; it is never worth stopping for.
    pub fn record(&mut self, now: SystemTime, event: &Event) -> io::Result<()> {
        sift_alloc::tagged(Subsystem::Logging, || {
            self.prune(now)?;
            let at = epoch_seconds(now);
            self.line.clear();
            event.format(at, &mut self.line);
            if !line_is_permitted(&self.line) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a line outside the log's grammar was refused",
                ));
            }
            let len = self.line.len() as u64;
            let half = self.budget / 2;
            if len > half {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a line longer than a segment was refused",
                ));
            }
            if self.current_len + len > half {
                self.rotate()?;
            }
            self.current.write_all(self.line.as_bytes())?;
            self.current_len += len;
            self.current_started.get_or_insert(at);
            self.current_written = Some(at);
            Ok(())
        })
    }

    /// Apply retention at `now` without recording anything.
    ///
    /// # Errors
    ///
    /// If a segment could not be removed or rotated.
    pub fn prune(&mut self, now: SystemTime) -> io::Result<()> {
        let now = epoch_seconds(now);
        let half = self.retention.as_secs() / 2;
        if self
            .current_started
            .is_some_and(|s| now.saturating_sub(s) >= half)
        {
            self.rotate()?;
        }
        if self
            .previous_written
            .is_some_and(|w| now.saturating_sub(w) >= half)
        {
            match fs::remove_file(self.dir.join(PREVIOUS)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            self.previous_len = 0;
            self.previous_written = None;
        }
        Ok(())
    }

    /// Close the current segment as the previous one, discarding what that held.
    fn rotate(&mut self) -> io::Result<()> {
        if self.current_len == 0 {
            return Ok(());
        }
        self.current.flush()?;
        fs::rename(self.dir.join(CURRENT), self.dir.join(PREVIOUS))?;
        self.current = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(CURRENT))?;
        self.previous_len = self.current_len;
        self.previous_written = self.current_written.or(self.current_started);
        self.current_len = 0;
        self.current_started = None;
        self.current_written = None;
        Ok(())
    }

    /// Bytes on disk across both segments — the live size the runtime panel reports.
    #[must_use]
    pub const fn live_bytes(&self) -> u64 {
        self.current_len + self.previous_len
    }

    /// The declared budget.
    #[must_use]
    pub const fn budget(&self) -> u64 {
        self.budget
    }

    /// The directory the log is in.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.dir
    }
}

/// The timestamp that starts a segment's first line.
fn first_timestamp(path: &Path) -> Option<u64> {
    let mut head = [0u8; 24];
    let n = File::open(path).ok()?.read(&mut head).ok()?;
    let head = &head[..n];
    let end = head.iter().position(|b| *b == b' ')?;
    core::str::from_utf8(&head[..end]).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const DAY: u64 = 24 * 60 * 60;

    /// A scratch directory of the test's own, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sift-observe-log-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn contents(dir: &Path) -> String {
        let mut all = String::new();
        for name in [PREVIOUS, CURRENT] {
            if let Ok(text) = fs::read_to_string(dir.join(name)) {
                all.push_str(&text);
            }
        }
        all
    }

    /// Known correspondence metadata and credential material, the kinds NFR-55 names.
    const SENSITIVE: &[&str] = &[
        "alice@example.com",
        "Bob Smith <bob@corp.example.org>",
        "example.com",
        "corp.example.org",
        "Re: Quarterly invoice overdue",
        "invoice overdue",
        "ya29.a0AfH6SMBx-3kQ_zR9f",
        "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
        "1//0gL-refresh_token",
        "INBOX/Receipts",
        "",
    ];

    /// Every constructor a call site uses, with the most revealing values it can be given.
    fn every_event(account: u128) -> Vec<Event> {
        let words = ["needs_authentication", "healthy"].map(Word::new);
        let mut events = vec![
            Event::condition(account, words[0], words[1]),
            Event::pressure(3),
            Event::launch(true),
            Event::launch(false),
            Event::timing(
                Subsystem::Sanitize,
                Word::new("cascade"),
                Duration::from_millis(7),
            ),
        ];
        for outcome in [
            Outcome::Ok,
            Outcome::Transient,
            Outcome::NonTransient,
            Outcome::Throttled,
            Outcome::CursorInvalidated,
            Outcome::Unknown,
        ] {
            events.push(Event::sync(account, outcome, Duration::from_millis(1234)));
            events.push(Event::flush(account, outcome, 9, Duration::from_secs(2)));
        }
        events
    }

    #[test]
    fn nothing_nfr_55_forbids_can_become_a_word() {
        // The type is the redaction. If any of these were a word, a call site could log it.
        for text in SENSITIVE {
            assert!(
                Word::try_new(text).is_none(),
                "{text:?} was accepted as a word"
            );
        }
        // Including the tempting near-misses: a local part alone still carries no `@`, but a
        // subject word would pass the character set — which is why `Word` is `'static` too.
        assert!(Word::try_new(&"x".repeat(MAX_WORD + 1).leak()[..]).is_none());
    }

    #[test]
    fn no_sensitive_text_reaches_the_file_through_any_constructor() {
        let scratch = Scratch::new();
        let mut log = Log::open(&scratch.0, BUDGET_BYTES, RETENTION, at(1_000)).unwrap();
        let account = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef;
        for event in every_event(account) {
            log.record(at(1_000), &event).unwrap();
        }
        let text = contents(&scratch.0);
        for needle in SENSITIVE.iter().filter(|s| !s.is_empty()) {
            assert!(!text.contains(needle), "{needle:?} reached the log");
        }
        assert!(!text.contains('@') && !text.contains('.'));
        for line in text.split_inclusive('\n') {
            assert!(line_is_permitted(line), "{line:?} is outside the grammar");
        }
        assert!(text.contains(" account=0123456789abcdef0123456789abcdef "));
        assert!(text.contains("sync account="));
        assert!(text.contains("outcome=non_transient"));
        assert!(text.contains("elapsed=1234ms"));
        assert!(text.contains("governor pressure tier=3"));
        assert!(text.contains("runtime launch core_dumps=disabled"));
    }

    #[test]
    fn the_grammar_refuses_what_a_word_could_not_have_produced() {
        assert!(line_is_permitted("12 sync sync outcome=ok\n"));
        for line in [
            "12 sync sync to=alice@example\n",
            "12 sync sync to=example.com\n",
            "12 sync sync subject=Hello\n",
            "12 sync sync  double\n",
            "12 sync sync token=a-b\n",
            "12 sync sync token=a/b+c\n",
            "12 sync sync\n\n",
            "12 sync sync",
            "x sync sync\n",
            "12\n",
        ] {
            assert!(!line_is_permitted(line), "{line:?} was permitted");
        }
    }

    #[test]
    fn the_budget_holds_and_the_oldest_goes_first() {
        let scratch = Scratch::new();
        let budget = 1_000;
        let mut log = Log::open(&scratch.0, budget, RETENTION, at(1_000)).unwrap();
        for i in 0..200u64 {
            let event = Event::new(Subsystem::Sync, Word::new("tick"))
                .field(Word::new("n"), Value::Count(i));
            log.record(at(1_000), &event).unwrap();
            let on_disk: u64 = [CURRENT, PREVIOUS]
                .iter()
                .filter_map(|n| fs::metadata(scratch.0.join(n)).ok())
                .map(|m| m.len())
                .sum();
            assert!(
                on_disk <= budget,
                "{on_disk} bytes over a {budget}-byte budget"
            );
            assert_eq!(on_disk, log.live_bytes());
        }
        let text = contents(&scratch.0);
        assert!(text.contains(" n=199\n"), "the newest line was evicted");
        assert!(
            !text.contains(" n=0\n"),
            "the oldest line survived past the budget"
        );
        // What survives is a contiguous tail: eviction never punches a hole in the middle.
        let kept: Vec<u64> = text
            .lines()
            .map(|l| l.rsplit('=').next().unwrap().parse().unwrap())
            .collect();
        assert!(kept.windows(2).all(|w| w[1] == w[0] + 1), "{kept:?}");
    }

    #[test]
    fn no_line_outlives_the_retention_period() {
        let scratch = Scratch::new();
        let retention = Duration::from_secs(8 * DAY);
        let start = 100 * DAY;
        let mut log = Log::open(&scratch.0, BUDGET_BYTES, retention, at(start)).unwrap();
        let event = |n| {
            Event::new(Subsystem::Sync, Word::new("tick")).field(Word::new("n"), Value::Count(n))
        };
        // One line a day for a month. At every write, nothing older than the period remains.
        for day in 0..30u64 {
            let now = start + day * DAY;
            log.record(at(now), &event(day)).unwrap();
            for line in contents(&scratch.0).lines() {
                let t: u64 = line.split(' ').next().unwrap().parse().unwrap();
                assert!(
                    now - t <= retention.as_secs(),
                    "a line {} days old survived an {}-day retention",
                    (now - t) / DAY,
                    retention.as_secs() / DAY
                );
            }
        }
        // And pruning alone, with nothing to record, empties a log left untouched long enough.
        log.prune(at(start + 60 * DAY)).unwrap();
        assert_eq!(contents(&scratch.0), "");
        assert_eq!(log.live_bytes(), 0);
    }

    #[test]
    fn a_reopened_log_resumes_its_segments_and_their_ages() {
        let scratch = Scratch::new();
        let retention = Duration::from_secs(8 * DAY);
        {
            let mut log = Log::open(&scratch.0, BUDGET_BYTES, retention, at(10 * DAY)).unwrap();
            log.record(at(10 * DAY), &Event::pressure(1)).unwrap();
        }
        let log = Log::open(&scratch.0, BUDGET_BYTES, retention, at(11 * DAY)).unwrap();
        assert!(log.live_bytes() > 0);
        assert_eq!(log.current_started, Some(10 * DAY));
        // Five days later the current segment is past half the period, and rotates on open.
        let log = Log::open(&scratch.0, BUDGET_BYTES, retention, at(15 * DAY)).unwrap();
        assert_eq!(log.current_len, 0);
        assert!(scratch.0.join(PREVIOUS).exists());
    }

    #[test]
    fn a_line_is_attributed_to_the_logging_row() {
        // The Logging row exists for this buffer; a write that allocated under whichever tag
        // was current would charge another subsystem for it.
        let scratch = Scratch::new();
        let mut log = Log::open(&scratch.0, BUDGET_BYTES, RETENTION, at(1)).unwrap();
        let before = sift_alloc::current();
        log.record(at(1), &Event::launch(true)).unwrap();
        assert_eq!(
            sift_alloc::current(),
            before,
            "the tag leaked out of the write"
        );
        assert_eq!(log.budget(), BUDGET_BYTES);
        assert_eq!(log.directory(), scratch.0.as_path());
    }

    #[test]
    #[should_panic(expected = "MAX_FIELDS")]
    fn an_event_refuses_a_seventh_field() {
        let mut e = Event::new(Subsystem::Sync, Word::new("x"));
        for _ in 0..=MAX_FIELDS {
            e = e.field(Word::new("n"), Value::Count(1));
        }
    }
}
