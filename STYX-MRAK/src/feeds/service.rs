//! Feed polling: filter, deduplicate, announce, remember the cursor.
//!
//! Delivery semantics: at-most-once per source post. A post is claimed in the
//! database *before* it is announced. If the announcement fails the row is
//! marked `failed` and retried on later polls (up to `MAX_ATTEMPTS`). If the
//! process dies between claim and announce, that post is not retried (a rare
//! missed post is preferred over a duplicate announcement).

use super::source::{cmp_ids, FeedItem, FeedSource};
use crate::config::FeedConfig;
use crate::db::repositories as repo;
use crate::error::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};

pub const MAX_ATTEMPTS: i64 = 5;
const MAX_BACKOFF_SECS: u64 = 3600;

/// Posts a feed item to Discord. Implemented by the Discord layer; mocked in tests.
#[async_trait]
pub trait Announcer: Send + Sync {
    /// Returns the Discord message ID on success.
    async fn announce(&self, item: &FeedItem, source_label: &str) -> std::result::Result<i64, String>;
}

#[derive(Clone, Debug, Default)]
pub struct FeedStatus {
    pub enabled: bool,
    pub last_check: Option<DateTime<Utc>>,
    pub last_ok: bool,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PollOutcome {
    pub fetched: usize,
    pub announced: usize,
    pub filtered: usize,
    pub baseline: usize,
    pub failed: usize,
}

/// An item is relevant if it mentions an include keyword and no exclude keyword.
pub fn is_relevant(text: &str, include: &[String], exclude: &[String]) -> bool {
    let t = text.to_lowercase();
    include.iter().any(|k| !k.is_empty() && t.contains(k.as_str()))
        && !exclude.iter().any(|k| !k.is_empty() && t.contains(k.as_str()))
}

/// Exponential backoff capped at one hour: `interval * 2^failures`.
pub fn backoff_delay(interval_secs: u64, failures: u32) -> Duration {
    let factor = 1u64 << failures.min(16);
    Duration::from_secs(
        interval_secs
            .saturating_mul(factor)
            .min(MAX_BACKOFF_SECS)
            .max(interval_secs),
    )
}

fn cursor_key(source: &str) -> String {
    format!("feed.cursor.{source}")
}

/// One poll cycle. Errors from the source bubble up so the worker can back off.
pub async fn poll_once(
    pool: &SqlitePool,
    source: &dyn FeedSource,
    announcer: &dyn Announcer,
    cfg: &FeedConfig,
    now: DateTime<Utc>,
) -> Result<PollOutcome> {
    let name = source.name().to_string();
    let key = cursor_key(&name);
    let stored = repo::get_config(pool, &key).await?;
    // "0" is the sentinel written when the first poll found nothing.
    let since = stored.as_deref().filter(|c| *c != "0");
    let baseline_mode = stored.is_none() && !cfg.announce_backlog;

    let mut items = source.fetch_new_items(since).await?;
    items.sort_by(|a, b| cmp_ids(&a.id, &b.id));
    items.dedup_by(|a, b| a.id == b.id);

    let mut out = PollOutcome {
        fetched: items.len(),
        ..Default::default()
    };
    let mut advance_to: Option<String> = None;

    for item in &items {
        // Never go backwards past what we already processed.
        if let Some(s) = since {
            if cmp_ids(&item.id, s) != std::cmp::Ordering::Greater {
                continue;
            }
        }
        if baseline_mode {
            repo::record_skipped_feed_item(pool, &name, &item.id, "baseline", &item.text, now).await?;
            out.baseline += 1;
        } else if !is_relevant(&item.text, &cfg.include_keywords, &cfg.exclude_keywords) {
            repo::record_skipped_feed_item(pool, &name, &item.id, "filtered", &item.text, now).await?;
            out.filtered += 1;
        } else if repo::claim_feed_item(pool, &name, &item.id, &item.text, now, MAX_ATTEMPTS).await? {
            match announcer.announce(item, &cfg.source_label).await {
                Ok(message_id) => {
                    repo::mark_feed_announced(pool, &name, &item.id, message_id, now).await?;
                    out.announced += 1;
                }
                Err(e) => {
                    tracing::warn!(source = %name, post = %item.id, error = %e, "feed announce failed; will retry");
                    repo::mark_feed_failed(pool, &name, &item.id).await?;
                    out.failed += 1;
                    // Stop here so the cursor stays before this item and order is preserved.
                    break;
                }
            }
        }
        // else: already handled earlier (dedup) — just advance.
        advance_to = Some(item.id.clone());
    }

    let new_cursor = match (advance_to, baseline_mode) {
        (Some(id), _) => Some(id),
        (None, true) => Some("0".to_string()),
        (None, false) => None,
    };
    if let Some(c) = new_cursor {
        repo::set_config(pool, &key, &c, now).await?;
    }
    Ok(out)
}

/// Background worker. Polls on `interval`, backs off exponentially on
/// failure, and exits promptly when `shutdown` flips to `true`.
pub async fn run_worker(
    pool: SqlitePool,
    source: Arc<dyn FeedSource>,
    announcer: Arc<dyn Announcer>,
    cfg: FeedConfig,
    status: Arc<Mutex<FeedStatus>>,
    mut shutdown: watch::Receiver<bool>,
) {
    tracing::info!(
        source = source.name(),
        interval = cfg.interval_seconds,
        "feed worker started"
    );
    let mut failures: u32 = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let now = Utc::now();
        let result = poll_once(&pool, source.as_ref(), announcer.as_ref(), &cfg, now).await;
        {
            let mut st = status.lock().await;
            st.last_check = Some(now);
            match &result {
                Ok(o) => {
                    failures = 0;
                    st.last_ok = true;
                    st.consecutive_failures = 0;
                    st.last_error = None;
                    if o.announced + o.failed > 0 {
                        tracing::info!(announced = o.announced, failed = o.failed, "feed poll");
                    }
                    if o.failed > 0 {
                        st.last_ok = false;
                        st.last_error = Some("announcement failed; retrying".into());
                    }
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    st.last_ok = false;
                    st.consecutive_failures = failures;
                    st.last_error = Some(e.to_string());
                    tracing::warn!(error = %e, failures, "feed poll failed");
                }
            }
        }
        let delay = if result.is_err() {
            backoff_delay(cfg.interval_seconds, failures)
        } else {
            Duration::from_secs(cfg.interval_seconds)
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.changed() => {}
        }
    }
    tracing::info!("feed worker stopped");
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::feeds::source::FeedError;
    use std::sync::Mutex as StdMutex;

    pub struct MockSource {
        pub items: StdMutex<Vec<FeedItem>>,
        pub fail: StdMutex<bool>,
    }
    impl MockSource {
        pub fn new(items: Vec<FeedItem>) -> Self {
            Self {
                items: StdMutex::new(items),
                fail: StdMutex::new(false),
            }
        }
    }
    #[async_trait]
    impl FeedSource for MockSource {
        fn name(&self) -> &str {
            "mock"
        }
        async fn fetch_new_items(&self, since: Option<&str>) -> std::result::Result<Vec<FeedItem>, FeedError> {
            if *self.fail.lock().unwrap() {
                return Err(FeedError::Status(503));
            }
            Ok(self
                .items
                .lock()
                .unwrap()
                .iter()
                .filter(|i| since.is_none_or(|s| cmp_ids(&i.id, s) == std::cmp::Ordering::Greater))
                .cloned()
                .collect())
        }
    }

    #[derive(Default)]
    pub struct MockAnnouncer {
        pub posted: StdMutex<Vec<String>>,
        pub fail: StdMutex<bool>,
    }
    #[async_trait]
    impl Announcer for MockAnnouncer {
        async fn announce(&self, item: &FeedItem, _label: &str) -> std::result::Result<i64, String> {
            if *self.fail.lock().unwrap() {
                return Err("discord down".into());
            }
            self.posted.lock().unwrap().push(item.id.clone());
            Ok(item.id.parse::<i64>().unwrap_or(1))
        }
    }

    pub fn item(id: &str, text: &str) -> FeedItem {
        FeedItem {
            id: id.into(),
            text: text.into(),
            url: format!("https://x.com/a/status/{id}"),
            created_at: None,
        }
    }

    pub fn cfg(announce_backlog: bool) -> FeedConfig {
        FeedConfig {
            enabled: true,
            interval_seconds: 300,
            x_bearer_token: Some("t".into()),
            x_username: "NosniyGames".into(),
            x_api_base: "https://api.x.com/2".into(),
            include_keywords: vec!["rivals".into()],
            exclude_keywords: vec!["treasure quest".into()],
            source_label: "Nosniy Games".into(),
            announce_backlog,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::db::test_pool;

    #[test]
    fn relevance_filter() {
        let inc = vec!["rivals".to_string()];
        let exc = vec!["treasure quest".to_string()];
        assert!(is_relevant("#RIVALS UPDATE 14!", &inc, &exc));
        assert!(!is_relevant("Treasure Quest sale", &inc, &exc));
        assert!(!is_relevant("Treasure Quest x Rivals crossover", &inc, &exc));
        assert!(!is_relevant("Happy birthday studio", &inc, &exc));
        assert!(!is_relevant("anything", &[], &[]));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_delay(300, 0), Duration::from_secs(300));
        assert_eq!(backoff_delay(300, 1), Duration::from_secs(600));
        assert_eq!(backoff_delay(300, 2), Duration::from_secs(1200));
        assert_eq!(backoff_delay(300, 10), Duration::from_secs(3600));
        assert_eq!(backoff_delay(300, u32::MAX), Duration::from_secs(3600));
        assert_eq!(backoff_delay(7200, 3), Duration::from_secs(7200));
    }

    #[tokio::test]
    async fn first_run_baselines_without_announcing() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![item("100", "RIVALS update"), item("101", "RIVALS patch")]);
        let ann = MockAnnouncer::default();
        let o = poll_once(&pool, &src, &ann, &cfg(false), Utc::now()).await.unwrap();
        assert_eq!(o.baseline, 2);
        assert!(ann.posted.lock().unwrap().is_empty());
        // New post after baseline is announced.
        src.items.lock().unwrap().push(item("102", "RIVALS Season 2"));
        let o = poll_once(&pool, &src, &ann, &cfg(false), Utc::now()).await.unwrap();
        assert_eq!(o.announced, 1);
        assert_eq!(*ann.posted.lock().unwrap(), vec!["102"]);
    }

    #[tokio::test]
    async fn empty_first_poll_still_baselines() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![]);
        let ann = MockAnnouncer::default();
        poll_once(&pool, &src, &ann, &cfg(false), Utc::now()).await.unwrap();
        src.items.lock().unwrap().push(item("5", "RIVALS news"));
        let o = poll_once(&pool, &src, &ann, &cfg(false), Utc::now()).await.unwrap();
        assert_eq!(o.announced, 1, "a post after an empty baseline must be announced");
    }

    #[tokio::test]
    async fn deduplicates_across_polls_and_restarts() {
        let pool = test_pool().await;
        let ann = MockAnnouncer::default();
        let items = vec![item("1", "RIVALS a"), item("2", "RIVALS b")];
        let src = MockSource::new(items.clone());
        poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
        assert_eq!(ann.posted.lock().unwrap().len(), 2);
        // Same source returns the same items again (cursor reset simulated by a source that ignores since).
        struct Stubborn(Vec<FeedItem>);
        #[async_trait]
        impl FeedSource for Stubborn {
            fn name(&self) -> &str {
                "mock"
            }
            async fn fetch_new_items(
                &self,
                _: Option<&str>,
            ) -> std::result::Result<Vec<FeedItem>, crate::feeds::source::FeedError> {
                Ok(self.0.clone())
            }
        }
        let o = poll_once(&pool, &Stubborn(items.clone()), &ann, &cfg(true), Utc::now())
            .await
            .unwrap();
        assert_eq!(o.announced, 0);
        assert_eq!(ann.posted.lock().unwrap().len(), 2, "no duplicate announcements");
        // Wipe the cursor (as if config were lost): the feed_items table still prevents duplicates.
        sqlx::query("DELETE FROM config").execute(&pool).await.unwrap();
        let o = poll_once(&pool, &Stubborn(items), &ann, &cfg(true), Utc::now())
            .await
            .unwrap();
        assert_eq!(o.announced, 0);
        assert_eq!(ann.posted.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn irrelevant_posts_are_filtered_and_remembered() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![item("1", "Treasure Quest update"), item("2", "RIVALS update")]);
        let ann = MockAnnouncer::default();
        let o = poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
        assert_eq!((o.filtered, o.announced), (1, 1));
        assert_eq!(
            repo::feed_item_status(&pool, "mock", "1").await.unwrap().as_deref(),
            Some("filtered")
        );
        assert_eq!(
            repo::feed_item_status(&pool, "mock", "2").await.unwrap().as_deref(),
            Some("announced")
        );
    }

    #[tokio::test]
    async fn failed_announcement_is_retried_then_given_up() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![item("1", "RIVALS a")]);
        let ann = MockAnnouncer::default();
        *ann.fail.lock().unwrap() = true;
        for _ in 0..MAX_ATTEMPTS {
            let o = poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
            assert_eq!(o.failed, 1);
        }
        // Attempts exhausted: it is no longer handed out and the cursor moves on.
        let o = poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
        assert_eq!((o.failed, o.announced), (0, 0));
        assert_eq!(
            repo::feed_item_status(&pool, "mock", "1").await.unwrap().as_deref(),
            Some("failed")
        );
    }

    #[tokio::test]
    async fn recovery_after_temporary_failure() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![item("1", "RIVALS a"), item("2", "RIVALS b")]);
        let ann = MockAnnouncer::default();
        *ann.fail.lock().unwrap() = true;
        let o = poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
        assert_eq!(o.failed, 1);
        *ann.fail.lock().unwrap() = false;
        let o = poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
        assert_eq!(o.announced, 2);
        // Order preserved.
        assert_eq!(*ann.posted.lock().unwrap(), vec!["1", "2"]);
    }

    #[tokio::test]
    async fn source_failure_is_an_error_and_changes_nothing() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![item("1", "RIVALS a")]);
        *src.fail.lock().unwrap() = true;
        let ann = MockAnnouncer::default();
        assert!(poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.is_err());
        assert!(ann.posted.lock().unwrap().is_empty());
        *src.fail.lock().unwrap() = false;
        assert_eq!(
            poll_once(&pool, &src, &ann, &cfg(true), Utc::now())
                .await
                .unwrap()
                .announced,
            1
        );
    }

    #[tokio::test]
    async fn out_of_order_items_are_announced_oldest_first() {
        let pool = test_pool().await;
        let src = MockSource::new(vec![
            item("30", "RIVALS c"),
            item("9", "RIVALS a"),
            item("100", "RIVALS d"),
        ]);
        let ann = MockAnnouncer::default();
        poll_once(&pool, &src, &ann, &cfg(true), Utc::now()).await.unwrap();
        assert_eq!(*ann.posted.lock().unwrap(), vec!["9", "30", "100"]);
    }
}
