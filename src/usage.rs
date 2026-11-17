//! Launch history, used to rank apps you actually use higher.
//!
//! Two signals:
//! * **Frecency** per app: launch count weighted by how recently it was used.
//!   Lifts frequently used apps, up to about one match tier.
//! * **Learned queries**: which app you picked after typing what. Typing the same
//!   text (or the start of it) again puts that app on top.
//!
//! Stored as a small tab-separated file in `%LOCALAPPDATA%\WinLaunch\usage.tsv`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: u64 = 24 * 60 * 60;
const MAX_QUERIES: usize = 500;
const MAX_FRECENCY_BOOST: f64 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    count: u32,
    last: u64, // unix seconds
}

#[derive(Clone, Default)]
pub struct Usage {
    apps: HashMap<String, Entry>,
    /// (lowercased query, target) -> how often that target was picked for it
    queries: HashMap<(String, String), Entry>,
}

impl Usage {
    /// Recency weight: recent launches count more, old ones fade but never vanish.
    fn recency(now: u64, last: u64) -> f64 {
        match now.saturating_sub(last) / DAY {
            0 => 1.0,
            1..=6 => 0.7,
            7..=29 => 0.4,
            _ => 0.2,
        }
    }

    /// Launch count weighted by recency; 0 for apps never launched.
    fn frecency(e: &Entry, now: u64) -> f64 {
        e.count as f64 * Self::recency(now, e.last)
    }

    /// Launched apps, most used first (frecency, then most recent launch).
    pub fn most_used(&self, now: u64) -> Vec<&str> {
        let mut apps: Vec<(&String, &Entry)> = self.apps.iter().collect();
        apps.sort_by(|a, b| {
            Self::frecency(b.1, now)
                .total_cmp(&Self::frecency(a.1, now))
                .then(b.1.last.cmp(&a.1.last))
                .then(a.0.cmp(b.0))
        });
        apps.into_iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Ranking bonus for `target` while the user has typed `query` (lowercased).
    pub fn boost(&self, query: &str, target: &str, now: u64) -> i32 {
        let mut boost = 0.0;
        if let Some(e) = self.apps.get(target) {
            boost += (50.0 * (1.0 + Self::frecency(e, now)).ln()).min(MAX_FRECENCY_BOOST);
        }
        if !query.is_empty() {
            // Best learned query that starts with what is typed so far:
            // picked after "chr" also counts while only "c" or "ch" is typed.
            let learned = self
                .queries
                .iter()
                .filter(|((q, t), _)| t == target && q.starts_with(query))
                .map(|((q, _), e)| {
                    let exact = if q == query { 100.0 } else { 0.0 };
                    let base = 250.0 + exact + 50.0 * (e.count.min(4) as f64);
                    base * Self::recency(now, e.last).max(0.4)
                })
                .fold(0.0, f64::max);
            boost += learned;
        }
        boost.round() as i32
    }

    pub fn record(&mut self, query: &str, target: &str, now: u64) {
        let bump = |e: &mut Entry| {
            e.count = e.count.saturating_add(1);
            e.last = now;
        };
        bump(self.apps.entry(target.to_string()).or_insert(Entry {
            count: 0,
            last: now,
        }));
        if !query.is_empty() {
            let key = (query.to_string(), target.to_string());
            bump(self.queries.entry(key).or_insert(Entry {
                count: 0,
                last: now,
            }));
            if self.queries.len() > MAX_QUERIES {
                // forget the least recently used learned query
                if let Some(oldest) = self
                    .queries
                    .iter()
                    .min_by_key(|(_, e)| e.last)
                    .map(|(k, _)| k.clone())
                {
                    self.queries.remove(&oldest);
                }
            }
        }
    }

    fn to_tsv(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        let mut out = String::from("# WinLaunch usage v1\n");
        for (t, e) in &self.apps {
            out += &format!("A\t{}\t{}\t{}\n", clean(t), e.count, e.last);
        }
        for ((q, t), e) in &self.queries {
            out += &format!("Q\t{}\t{}\t{}\t{}\n", clean(q), clean(t), e.count, e.last);
        }
        out
    }

    fn from_tsv(text: &str) -> Self {
        let mut usage = Usage::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let entry = |c: &str, l: &str| -> Option<Entry> {
                Some(Entry {
                    count: c.parse().ok()?,
                    last: l.parse().ok()?,
                })
            };
            match f.as_slice() {
                ["A", t, c, l] => {
                    if let Some(e) = entry(c, l) {
                        usage.apps.insert(t.to_string(), e);
                    }
                }
                ["Q", q, t, c, l] => {
                    if let Some(e) = entry(c, l) {
                        usage.queries.insert((q.to_string(), t.to_string()), e);
                    }
                }
                _ => {} // comments, unknown or broken lines
            }
        }
        usage
    }
}

static USAGE: Mutex<Option<Arc<Usage>>> = Mutex::new(None);

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    Some(crate::local_data_dir()?.join("usage.tsv"))
}

/// Current history (empty until `load` has run).
pub fn snapshot() -> Arc<Usage> {
    USAGE.lock().unwrap().clone().unwrap_or_default()
}

/// Reads the history file once; called from the background index thread.
pub fn load() {
    let mut slot = USAGE.lock().unwrap();
    if slot.is_some() {
        return;
    }
    let text = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    *slot = Some(Arc::new(Usage::from_tsv(&text)));
}

/// Held while the history is written, so two saves never share the temporary file.
static SAVING: Mutex<()> = Mutex::new(());

/// Remembers a launch and saves the history in the background.
pub fn record(query: &str, target: &str) {
    load();
    {
        let mut slot = USAGE.lock().unwrap();
        let mut usage = slot.as_deref().cloned().unwrap_or_default();
        usage.record(query, target, now());
        *slot = Some(Arc::new(usage));
    }
    std::thread::spawn(|| {
        let _saving = SAVING.lock().unwrap();
        // The newest state when it is our turn: a save that waited for another one
        // never writes an older state over a newer one.
        let usage = snapshot();
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            // write + rename, so a crash never leaves a half-written file
            let tmp = p.with_extension("tsv.tmp");
            if std::fs::write(&tmp, usage.to_tsv()).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn unused_app_gets_no_boost() {
        assert_eq!(Usage::default().boost("no", "OneNote", NOW), 0);
    }

    #[test]
    fn frequent_recent_use_beats_rare_old_use() {
        let mut u = Usage::default();
        for _ in 0..10 {
            u.record("", "Frequent", NOW);
        }
        u.record("", "Rare", NOW - 60 * DAY);
        assert!(u.boost("", "Frequent", NOW) > u.boost("", "Rare", NOW));
        assert!(u.boost("", "Frequent", NOW) <= MAX_FRECENCY_BOOST as i32);
    }

    #[test]
    fn learned_query_also_matches_shorter_prefix() {
        let mut u = Usage::default();
        u.record("chr", "Chrome", NOW);
        let typed_c = u.boost("c", "Chrome", NOW);
        let typed_chr = u.boost("chr", "Chrome", NOW);
        assert!(
            typed_c > 250,
            "prefix of a learned query should boost strongly"
        );
        assert!(typed_chr > typed_c, "exact learned query should boost most");
        assert!(
            u.boost("cx", "Chrome", NOW) < 200,
            "different query: frecency only"
        );
        assert_eq!(u.boost("c", "Calculator", NOW), 0);
    }

    #[test]
    fn learned_query_lifts_app_above_better_text_match() {
        // "note": Notepad++ is a prefix match (900), OneNote a word match (800).
        let mut u = Usage::default();
        u.record("note", "OneNote", NOW);
        assert!(800 + u.boost("note", "OneNote", NOW) > 900 + u.boost("note", "Notepad++", NOW));
    }

    #[test]
    fn most_used_orders_by_frecency_then_recency() {
        let mut u = Usage::default();
        assert!(u.most_used(NOW).is_empty());
        for _ in 0..5 {
            u.record("", "OldFavourite", NOW - 60 * DAY); // 5 x 0.2 = 1.0
        }
        for _ in 0..3 {
            u.record("", "DailyTool", NOW); // 3 x 1.0 = 3.0
        }
        u.record("", "OnceToday", NOW); // 1.0, but more recent than OldFavourite
        assert_eq!(
            u.most_used(NOW),
            vec!["DailyTool", "OnceToday", "OldFavourite"]
        );
    }

    #[test]
    fn round_trips_through_file_format() {
        let mut u = Usage::default();
        u.record("vs", "Microsoft.VisualStudioCode", NOW);
        u.record("", "C:\\Tools\\app.exe", NOW - DAY);
        let back = Usage::from_tsv(&u.to_tsv());
        assert_eq!(back.apps, u.apps);
        assert_eq!(back.queries, u.queries);
    }

    #[test]
    fn broken_lines_are_ignored() {
        let u = Usage::from_tsv("garbage\nA\tx\tnot-a-number\t1\nA\tok\t2\t3\n");
        assert_eq!(u.apps.len(), 1);
        assert_eq!(u.apps["ok"], Entry { count: 2, last: 3 });
    }

    #[test]
    fn query_history_is_capped() {
        let mut u = Usage::default();
        for i in 0..(MAX_QUERIES as u64 + 20) {
            u.record(&format!("q{i}"), "App", NOW + i);
        }
        assert_eq!(u.queries.len(), MAX_QUERIES);
        assert!(!u
            .queries
            .contains_key(&("q0".to_string(), "App".to_string())));
    }
}
