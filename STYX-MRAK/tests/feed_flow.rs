//! Feed pipeline: filter -> deduplicate -> announce, driven through the public API.

use async_trait::async_trait;
use chrono::Utc;
use std::sync::Mutex;
use styx_mrak::config::FeedConfig;
use styx_mrak::db::{self, repositories as repo};
use styx_mrak::feeds::service::{backoff_delay, is_relevant, poll_once, Announcer};
use styx_mrak::feeds::source::{parse_timeline, FeedError, FeedItem, FeedSource};

struct Fixed(Mutex<Vec<FeedItem>>);
#[async_trait]
impl FeedSource for Fixed {
    fn name(&self) -> &str {
        "x:nosniygames"
    }
    async fn fetch_new_items(&self, _since: Option<&str>) -> Result<Vec<FeedItem>, FeedError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[derive(Default)]
struct Rec(Mutex<Vec<(String, String)>>);
#[async_trait]
impl Announcer for Rec {
    async fn announce(&self, i: &FeedItem, label: &str) -> Result<i64, String> {
        self.0.lock().unwrap().push((i.id.clone(), label.to_string()));
        Ok(42)
    }
}

fn cfg() -> FeedConfig {
    FeedConfig {
        enabled: true,
        interval_seconds: 300,
        x_bearer_token: Some("t".into()),
        x_username: "NosniyGames".into(),
        x_api_base: "https://api.x.com/2".into(),
        include_keywords: vec!["rivals".into()],
        exclude_keywords: vec!["treasure quest".into(), "super golf".into()],
        source_label: "Nosniy Games".into(),
        announce_backlog: true,
    }
}

#[tokio::test]
async fn new_source_item_is_filtered_deduplicated_and_announced() {
    let pool = db::connect_memory().await.unwrap();
    // A realistic X API v2 payload (shape per the public API docs).
    let body = r##"{"data":[
        {"id":"1958924702103347360","text":"#RIVALS UPDATE 14! Emotes, Battleground Revamp, Map Contracts"},
        {"id":"1958924702103347361","text":"Treasure Quest is 50% off this weekend"},
        {"id":"1958924702103347362","text":"Happy holidays from the studio!"}
    ],"meta":{"result_count":3}}"##;
    let items = parse_timeline(body, "NosniyGames").unwrap();
    let src = Fixed(Mutex::new(items));
    let rec = Rec::default();

    let first = poll_once(&pool, &src, &rec, &cfg(), Utc::now()).await.unwrap();
    assert_eq!((first.announced, first.filtered), (1, 2));
    // Same payload again (e.g. after a restart or an API repeat): nothing new.
    let second = poll_once(&pool, &src, &rec, &cfg(), Utc::now()).await.unwrap();
    assert_eq!((second.announced, second.filtered), (0, 0));

    let posted = rec.0.lock().unwrap().clone();
    assert_eq!(
        posted,
        vec![("1958924702103347360".to_string(), "Nosniy Games".to_string())]
    );
    assert_eq!(repo::count_feed_items(&pool, "announced").await.unwrap(), 1);
    assert_eq!(repo::count_feed_items(&pool, "filtered").await.unwrap(), 2);
}

#[test]
fn relevance_and_backoff_rules() {
    let c = cfg();
    assert!(is_relevant(
        "RIVALS season 2 trailer",
        &c.include_keywords,
        &c.exclude_keywords
    ));
    assert!(!is_relevant(
        "Super Golf update",
        &c.include_keywords,
        &c.exclude_keywords
    ));
    assert!(backoff_delay(300, 3) > backoff_delay(300, 1));
    assert_eq!(backoff_delay(300, 30).as_secs(), 3600);
}
