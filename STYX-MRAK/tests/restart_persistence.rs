//! Simulates stopping and starting the bot: everything that persistent UI and
//! the roster depend on must survive a close/reopen of the SQLite file.

mod common;

use async_trait::async_trait;
use chrono::Utc;
use std::sync::Mutex;
use styx_mrak::applications::service as apps;
use styx_mrak::config::FeedConfig;
use styx_mrak::db::{self, repositories as repo};
use styx_mrak::feeds::service::{poll_once, Announcer};
use styx_mrak::feeds::source::{FeedError, FeedItem, FeedSource};

struct Src(Vec<FeedItem>);
#[async_trait]
impl FeedSource for Src {
    fn name(&self) -> &str {
        "x:test"
    }
    async fn fetch_new_items(&self, since: Option<&str>) -> Result<Vec<FeedItem>, FeedError> {
        Ok(self
            .0
            .iter()
            .filter(|i| since.is_none_or(|s| styx_mrak::feeds::source::cmp_ids(&i.id, s).is_gt()))
            .cloned()
            .collect())
    }
}

#[derive(Default)]
struct Ann(Mutex<Vec<String>>);
#[async_trait]
impl Announcer for Ann {
    async fn announce(&self, item: &FeedItem, _l: &str) -> Result<i64, String> {
        self.0.lock().unwrap().push(item.id.clone());
        Ok(1)
    }
}

fn item(id: &str, text: &str) -> FeedItem {
    FeedItem {
        id: id.into(),
        text: text.into(),
        url: String::new(),
        created_at: None,
    }
}

fn feed_cfg() -> FeedConfig {
    FeedConfig {
        enabled: true,
        interval_seconds: 300,
        x_bearer_token: Some("t".into()),
        x_username: "NosniyGames".into(),
        x_api_base: "https://api.x.com/2".into(),
        include_keywords: vec!["rivals".into()],
        exclude_keywords: vec![],
        source_label: "Nosniy Games".into(),
        announce_backlog: true,
    }
}

#[tokio::test]
async fn data_ui_state_and_feed_cursor_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("styx.db").display());
    let ann = Ann::default();
    let src = Src(vec![item("10", "RIVALS update"), item("11", "RIVALS patch notes")]);

    // --- first run ---
    let pool = db::connect(&url).await.unwrap();
    let app = apps::submit(&pool, &common::application(1), Utc::now()).await.unwrap();
    apps::accept(&pool, app.id, 9, Utc::now()).await.unwrap();
    repo::set_config(&pool, "roster.message", "555:666", Utc::now())
        .await
        .unwrap();
    repo::set_config(&pool, "panel.apply", "777:888", Utc::now())
        .await
        .unwrap();
    let pending = repo::create_pending_action(&pool, 9, "{}", Utc::now(), chrono::Duration::minutes(10))
        .await
        .unwrap();
    poll_once(&pool, &src, &ann, &feed_cfg(), Utc::now()).await.unwrap();
    assert_eq!(ann.0.lock().unwrap().len(), 2);
    pool.close().await;

    // --- second run (fresh pool, migrations run again) ---
    let pool = db::connect(&url).await.unwrap();
    assert_eq!(repo::list_current_members(&pool).await.unwrap().len(), 1);
    assert!(repo::get_application(&pool, app.id).await.unwrap().is_some());
    // Canonical roster message and apply panel IDs are still known: no duplicate posts.
    assert_eq!(
        repo::get_config(&pool, "roster.message").await.unwrap().as_deref(),
        Some("555:666")
    );
    assert_eq!(
        repo::get_config(&pool, "panel.apply").await.unwrap().as_deref(),
        Some("777:888")
    );
    // An unconfirmed action can still be confirmed after the restart.
    assert!(repo::consume_pending_action(&pool, pending, 9, Utc::now())
        .await
        .unwrap()
        .is_some());
    // The feed does not re-announce what was already posted.
    let o = poll_once(&pool, &src, &ann, &feed_cfg(), Utc::now()).await.unwrap();
    assert_eq!(o.announced, 0);
    assert_eq!(ann.0.lock().unwrap().len(), 2);
    // New posts after the restart are announced exactly once.
    let src2 = Src(vec![
        item("10", "RIVALS update"),
        item("11", "RIVALS patch notes"),
        item("12", "RIVALS season 2"),
    ]);
    poll_once(&pool, &src2, &ann, &feed_cfg(), Utc::now()).await.unwrap();
    poll_once(&pool, &src2, &ann, &feed_cfg(), Utc::now()).await.unwrap();
    assert_eq!(*ann.0.lock().unwrap(), vec!["10", "11", "12"]);
    pool.close().await;
}

#[tokio::test]
async fn migrations_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("nested/dir/styx.db").display());
    for _ in 0..3 {
        let pool = db::connect(&url).await.unwrap();
        assert!(db::ping(&pool).await);
        pool.close().await;
    }
}
