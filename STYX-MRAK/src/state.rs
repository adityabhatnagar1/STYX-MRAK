//! Shared application state handed to every handler.

use crate::config::Config;
use crate::feeds::service::FeedStatus;
use crate::moderation::service::Automod;
use chrono::{DateTime, Utc};
use serenity::http::Http;
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Instant;
use tokio::sync::Mutex;

pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: SqlitePool,
    http: OnceLock<Arc<Http>>,
    pub started_at: Instant,
    pub feed_status: Arc<Mutex<FeedStatus>>,
    pub automod: StdMutex<Automod>,
    /// Serialises roster refreshes so two triggers cannot race and double-post.
    pub roster_lock: Mutex<()>,
    pub last_roster_update: StdMutex<Option<DateTime<Utc>>>,
}

impl AppState {
    pub fn new(cfg: Config, db: SqlitePool) -> Arc<Self> {
        let automod = Automod::new(cfg.automod.clone());
        let feed_status = FeedStatus {
            enabled: cfg.feed.enabled,
            ..Default::default()
        };
        Arc::new(Self {
            cfg: Arc::new(cfg),
            db,
            http: OnceLock::new(),
            started_at: Instant::now(),
            feed_status: Arc::new(Mutex::new(feed_status)),
            automod: StdMutex::new(automod),
            roster_lock: Mutex::new(()),
            last_roster_update: StdMutex::new(None),
        })
    }

    /// Called once, right after the Discord client is built and before it starts.
    pub fn set_http(&self, http: Arc<Http>) {
        let _ = self.http.set(http);
    }

    /// The Discord REST client. `set_http` runs before any handler can fire,
    /// so this is an invariant of startup rather than a runtime failure mode.
    pub fn http(&self) -> &Arc<Http> {
        self.http
            .get()
            .expect("http client is initialised before the bot starts")
    }
}
