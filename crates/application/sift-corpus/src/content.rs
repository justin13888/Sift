//! What each synthetic message says — deterministically, from a seed.
//!
//! "The shape matters more than the content", but content is not free of shape: subject and
//! sender lengths decide the envelope's bytes, threading decides the thread table, and the
//! script a subject is written in decides whether the index word-segments it or trigrams it
//! (D-81). So the content is plain, but it is shaped like mail.

use sift_foundation::limits::L16_SNIPPET_CHARS;
use sift_provider::adapter::{Envelope, RemoteFolderId, RemoteMessageId};

/// SplitMix64. Small, fast, and — the property that matters — the same sequence on every
/// machine, so a seed names a corpus rather than a run.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`. `n` of zero yields zero.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// True with probability `percent` in a hundred.
    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a>(&mut self, from: &'a [&'a str]) -> &'a str {
        from[usize::try_from(self.below(from.len() as u64)).unwrap_or(0)]
    }
}

/// A thread a later message may reply into.
#[derive(Debug, Clone)]
struct OpenThread {
    remote: String,
    subject: String,
    last_message_id: String,
}

/// The per-account generator: the account's own address, its sender pool, and the threads it
/// has started so far.
#[derive(Debug)]
pub(crate) struct Writer {
    rng: Rng,
    account: usize,
    address: String,
    anchor_millis: u64,
    next_message: u64,
    next_thread: u64,
    /// The most recent threads, as a ring. A reply joins one of these, which is how real
    /// conversations cluster: nobody replies uniformly across ten years of mail.
    threads: Vec<OpenThread>,
    ring: usize,
}

/// How far back the corpus reaches.
const SPAN_MILLIS: u64 = 8 * 365 * 24 * 60 * 60 * 1000;
/// Share of messages that reply into an existing thread.
const REPLY_PERCENT: u64 = 35;
/// Share of subjects in a script D-81 trigrams, or with diacritics the index folds.
const NON_LATIN_PERCENT: u64 = 5;
const THREAD_RING: usize = 2048;

impl Writer {
    pub(crate) fn new(seed: u64, account: usize, address: &str, anchor_millis: u64) -> Self {
        // Mixed per account so two accounts built from one seed are not the same mailbox.
        let mut mix = Rng::new(seed ^ (account as u64).wrapping_mul(0xA24B_AED4_963E_E407));
        Self {
            rng: Rng::new(mix.next_u64()),
            account,
            address: address.to_owned(),
            anchor_millis,
            next_message: 0,
            next_thread: 0,
            threads: Vec::with_capacity(THREAD_RING),
            ring: 0,
        }
    }

    /// The next message, in `folder`.
    pub(crate) fn envelope(&mut self, folder: &RemoteFolderId, sent: bool) -> Envelope {
        let n = self.next_message;
        self.next_message += 1;
        let message_id = format!("<{n}.a{}@corpus.sift.test>", self.account);

        let reply_to = if !self.threads.is_empty() && self.rng.chance(REPLY_PERCENT) {
            let i = usize::try_from(self.rng.below(self.threads.len() as u64)).unwrap_or(0);
            Some(i)
        } else {
            None
        };
        let (thread, subject, references) = match reply_to {
            Some(i) => {
                let t = &mut self.threads[i];
                let references = vec![t.last_message_id.clone()];
                t.last_message_id.clone_from(&message_id);
                (t.remote.clone(), format!("Re: {}", t.subject), references)
            }
            None => {
                let remote = format!("t{}", self.next_thread);
                self.next_thread += 1;
                let subject = self.subject();
                let open = OpenThread {
                    remote: remote.clone(),
                    subject: subject.clone(),
                    last_message_id: message_id.clone(),
                };
                if self.threads.len() < THREAD_RING {
                    self.threads.push(open);
                } else {
                    self.threads[self.ring] = open;
                    self.ring = (self.ring + 1) % THREAD_RING;
                }
                (remote, subject, Vec::new())
            }
        };

        let correspondent = self.correspondent();
        let (from, to) = if sent {
            (self.address.clone(), vec![correspondent])
        } else {
            let mut to = vec![self.address.clone()];
            if self.rng.chance(20) {
                to.push(self.correspondent());
            }
            (correspondent, to)
        };

        // Skewed toward the recent: a square of a uniform draw puts most mail in the last
        // few years and a long tail behind it, which is what an inbox sorted by D-55's key
        // actually pages through.
        let u = self.rng.below(1 << 20);
        let age = SPAN_MILLIS / (1 << 20) * u / (1 << 20) * u;
        let received = self.anchor_millis.saturating_sub(age);
        let origination = received.saturating_sub(self.rng.below(120_000));

        let size = match self.rng.below(100) {
            0..=69 => 2_000 + self.rng.below(30_000),
            70..=94 => 30_000 + self.rng.below(300_000),
            _ => 300_000 + self.rng.below(10_000_000),
        };
        let tags = if self.rng.chance(3) {
            vec![self.rng.pick(TAGS).to_owned()]
        } else {
            Vec::new()
        };

        Envelope {
            id: RemoteMessageId(format!("m{n}")),
            thread_id: Some(thread),
            internet_message_id: Some(message_id),
            references,
            subject: Some(subject),
            from: Some(from),
            to,
            received_at_millis: received,
            origination_date_millis: Some(origination),
            snippet: Some(self.snippet()),
            folders: vec![folder.clone()],
            tags,
            // Most old mail is read; the unread share is what an inbox badge counts.
            read: sent || !self.rng.chance(12),
            flagged: self.rng.chance(2),
            size_estimate: size,
        }
    }

    fn correspondent(&mut self) -> String {
        let first = self.rng.pick(FIRST);
        let last = self.rng.pick(LAST);
        let domain = self.rng.below(400);
        format!(
            "{first} {last} <{}.{}@d{domain}.example>",
            first.to_lowercase(),
            last.to_lowercase()
        )
    }

    fn subject(&mut self) -> String {
        if self.rng.chance(NON_LATIN_PERCENT) {
            return self.rng.pick(NON_LATIN).to_owned();
        }
        let words = 3 + self.rng.below(7);
        let mut s = String::new();
        for i in 0..words {
            if i > 0 {
                s.push(' ');
            }
            let w = self.rng.pick(WORDS);
            if i == 0 {
                let mut c = w.chars();
                if let Some(f) = c.next() {
                    s.extend(f.to_uppercase());
                    s.push_str(c.as_str());
                }
            } else {
                s.push_str(w);
            }
        }
        if self.rng.chance(10) {
            s.push_str(&format!(" #{}", self.rng.below(100_000)));
        }
        s
    }

    fn snippet(&mut self) -> String {
        let limit = usize::try_from(L16_SNIPPET_CHARS).unwrap_or(280);
        let target = 80 + usize::try_from(self.rng.below(200)).unwrap_or(0);
        let mut s = String::new();
        while s.chars().count() < target.min(limit) {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(self.rng.pick(WORDS));
        }
        // L-16 truncates rather than rejects, and so does this.
        s.chars().take(limit).collect()
    }
}

const FIRST: &[&str] = &[
    "Ada", "Bea", "Cyrus", "Dana", "Emil", "Farah", "Gus", "Hana", "Ivo", "Jun", "Kira", "Lars",
    "Mina", "Nils", "Olga", "Pita", "Quinn", "Rosa", "Sven", "Tomas", "Uma", "Vera", "Wes", "Xian",
    "Yara", "Zeno",
];

const LAST: &[&str] = &[
    "Abara", "Brandt", "Castillo", "Dubois", "Eriksen", "Fischer", "Gomez", "Haddad", "Ibsen",
    "Jansen", "Kowalski", "Larsen", "Moreau", "Nakamura", "Okafor", "Petrov", "Quist", "Rossi",
    "Sato", "Tanaka", "Ueda", "Varga", "Weber", "Yilmaz", "Zhou",
];

/// Plain English, and nothing a scan of this crate would take for a provider name.
const WORDS: &[&str] = &[
    "quarterly",
    "report",
    "meeting",
    "notes",
    "invoice",
    "schedule",
    "update",
    "draft",
    "review",
    "budget",
    "travel",
    "itinerary",
    "receipt",
    "order",
    "shipped",
    "delivery",
    "project",
    "plan",
    "roadmap",
    "question",
    "about",
    "the",
    "next",
    "week",
    "monday",
    "friday",
    "call",
    "agenda",
    "summary",
    "follow",
    "up",
    "on",
    "our",
    "discussion",
    "team",
    "lunch",
    "photos",
    "from",
    "weekend",
    "renewal",
    "subscription",
    "confirmation",
    "account",
    "statement",
    "reminder",
    "deadline",
    "proposal",
    "contract",
    "signed",
    "copy",
    "attached",
    "please",
    "find",
    "thanks",
    "again",
    "for",
    "your",
    "help",
    "with",
    "this",
    "release",
    "notes",
    "version",
    "build",
    "failed",
    "passed",
    "tickets",
    "concert",
    "dinner",
    "party",
    "garden",
    "kitchen",
    "repair",
    "estimate",
    "quote",
    "insurance",
    "policy",
    "claim",
    "newsletter",
    "digest",
    "issue",
    "volume",
    "welcome",
    "introduction",
    "hiring",
    "offer",
    "interview",
    "feedback",
    "survey",
    "results",
    "migration",
    "database",
    "server",
    "latency",
    "café",
    "naïve",
    "résumé",
    "coöperation",
    "façade",
];

/// Subjects in scripts D-81 must trigram, or that the index must fold and the display must
/// isolate — CJK, Hangul, Thai, Arabic, Hebrew, Cyrillic, Greek.
const NON_LATIN: &[&str] = &[
    "会議の議事録について",
    "来週のスケジュール確認",
    "季度报告和预算",
    "주간 회의 안내",
    "การประชุมประจำสัปดาห์",
    "تقرير الربع السنوي",
    "סיכום הפגישה",
    "Счёт за услуги связи",
    "Πρόγραμμα ταξιδιού",
    "Zusammenfassung der Besprechung",
];

const TAGS: &[&str] = &["important", "follow-up", "waiting", "personal", "work"];

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> RemoteFolderId {
        RemoteFolderId("INBOX".into())
    }

    #[test]
    fn a_seed_names_a_corpus() {
        let mut a = Writer::new(22, 0, "me@corpus.test", 1_700_000_000_000);
        let mut b = Writer::new(22, 0, "me@corpus.test", 1_700_000_000_000);
        for _ in 0..500 {
            assert_eq!(a.envelope(&folder(), false), b.envelope(&folder(), false));
        }
    }

    #[test]
    fn two_accounts_from_one_seed_are_different_mailboxes() {
        let mut a = Writer::new(22, 0, "me@corpus.test", 1_700_000_000_000);
        let mut b = Writer::new(22, 1, "me@corpus.test", 1_700_000_000_000);
        assert_ne!(
            a.envelope(&folder(), false).subject,
            b.envelope(&folder(), false).subject
        );
    }

    #[test]
    fn snippets_respect_l16() {
        let mut w = Writer::new(1, 0, "me@corpus.test", 1_700_000_000_000);
        for _ in 0..2_000 {
            let e = w.envelope(&folder(), false);
            let chars = e.snippet.expect("snippet").chars().count();
            assert!(chars as u64 <= L16_SNIPPET_CHARS, "{chars} chars");
        }
    }

    #[test]
    fn replies_join_threads_and_new_threads_start() {
        let mut w = Writer::new(3, 0, "me@corpus.test", 1_700_000_000_000);
        let envelopes: Vec<_> = (0..2_000).map(|_| w.envelope(&folder(), false)).collect();
        let threads: std::collections::BTreeSet<_> = envelopes
            .iter()
            .filter_map(|e| e.thread_id.clone())
            .collect();
        let replies = envelopes
            .iter()
            .filter(|e| !e.references.is_empty())
            .count();
        assert!(threads.len() < envelopes.len(), "nothing was threaded");
        assert!(replies > 400, "only {replies} replies in 2,000");
        assert!(
            envelopes
                .iter()
                .filter(|e| !e.references.is_empty())
                .all(|e| e.subject.as_deref().is_some_and(|s| s.starts_with("Re: "))),
            "a reply did not carry its thread's subject"
        );
    }

    #[test]
    fn received_times_stay_within_the_span_and_never_pass_the_anchor() {
        let anchor = 1_700_000_000_000;
        let mut w = Writer::new(9, 0, "me@corpus.test", anchor);
        for _ in 0..5_000 {
            let e = w.envelope(&folder(), false);
            assert!(e.received_at_millis <= anchor);
            assert!(e.received_at_millis >= anchor - SPAN_MILLIS);
        }
    }

    #[test]
    fn some_subjects_are_in_scripts_the_index_trigrams() {
        let mut w = Writer::new(5, 0, "me@corpus.test", 1_700_000_000_000);
        let trigrammed = (0..4_000)
            .map(|_| w.envelope(&folder(), false))
            .filter(|e| {
                sift_index::ingest::tokenize(e.subject.as_deref().unwrap_or(""))
                    .iter()
                    .any(|t| t.trigram)
            })
            .count();
        assert!(trigrammed > 0, "no subject exercised D-81's trigram path");
    }

    #[test]
    fn sent_mail_is_from_the_account() {
        let mut w = Writer::new(5, 0, "me@corpus.test", 1_700_000_000_000);
        let e = w.envelope(&RemoteFolderId("SENT".into()), true);
        assert_eq!(e.from.as_deref(), Some("me@corpus.test"));
        assert!(e.read);
    }
}
