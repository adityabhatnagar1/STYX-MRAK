//! Feed source adapters. The rest of the bot only sees the `FeedSource` trait.
//!
//! `XFeedSource` reads the public timeline of an X (Twitter) account through
//! the documented X API v2:
//!   GET /2/users/by/username/{username}
//!   GET /2/users/{id}/tweets?since_id=...&exclude=retweets,replies&tweet.fields=created_at
//! This needs an X API bearer token (`FEED_X_BEARER_TOKEN`). It has NOT been
//! exercised against the live API from the build environment; see
//! HANDOFF.md "Known limitations".

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum FeedError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("source returned HTTP {0}")]
    Status(u16),
    #[error("could not parse source response: {0}")]
    Parse(String),
    #[error("feed source not available: {0}")]
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedItem {
    /// Opaque, source-unique ID. For X this is a numeric snowflake string.
    pub id: String,
    pub text: String,
    pub url: String,
    pub created_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait FeedSource: Send + Sync {
    /// Stable identifier used as the dedup namespace and cursor key.
    fn name(&self) -> &str;

    /// Items newer than `since_id` (or the latest few when `None`). Order is
    /// not guaranteed; the service sorts.
    async fn fetch_new_items(&self, since_id: Option<&str>) -> Result<Vec<FeedItem>, FeedError>;
}

/// Compare two numeric-string IDs without parsing (works for any length).
pub fn cmp_ids(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[derive(Deserialize)]
struct UserLookup {
    data: Option<UserData>,
}
#[derive(Deserialize)]
struct UserData {
    id: String,
}
#[derive(Deserialize)]
struct Timeline {
    data: Option<Vec<Tweet>>,
}
#[derive(Deserialize)]
struct Tweet {
    id: String,
    text: String,
    created_at: Option<DateTime<Utc>>,
}

pub fn parse_user_id(body: &str) -> Result<String, FeedError> {
    let v: UserLookup = serde_json::from_str(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    v.data
        .map(|d| d.id)
        .ok_or_else(|| FeedError::Unavailable("account not found".into()))
}

/// Parse an X API v2 timeline response. A response with no `data` (no new
/// posts) is an empty list, not an error.
pub fn parse_timeline(body: &str, username: &str) -> Result<Vec<FeedItem>, FeedError> {
    let t: Timeline = serde_json::from_str(body).map_err(|e| FeedError::Parse(e.to_string()))?;
    Ok(t.data
        .unwrap_or_default()
        .into_iter()
        .map(|tw| FeedItem {
            url: format!("https://x.com/{username}/status/{}", tw.id),
            id: tw.id,
            text: tw.text,
            created_at: tw.created_at,
        })
        .collect())
}

pub struct XFeedSource {
    client: reqwest::Client,
    base: String,
    bearer: String,
    username: String,
    name: String,
    user_id: Mutex<Option<String>>,
}

impl XFeedSource {
    pub fn new(base: &str, bearer: &str, username: &str) -> Result<Self, FeedError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("styx-mrak/0.1")
            .build()?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            bearer: bearer.to_string(),
            username: username.trim_start_matches('@').to_string(),
            name: format!("x:{}", username.trim_start_matches('@').to_lowercase()),
            user_id: Mutex::new(None),
        })
    }

    async fn get(&self, url: &str, query: &[(&str, String)]) -> Result<String, FeedError> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.bearer)
            .query(query)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(FeedError::Status(status.as_u16()));
        }
        Ok(resp.text().await?)
    }

    async fn resolve_user_id(&self) -> Result<String, FeedError> {
        let mut guard = self.user_id.lock().await;
        if let Some(id) = guard.as_ref() {
            return Ok(id.clone());
        }
        let url = format!("{}/users/by/username/{}", self.base, self.username);
        let body = self.get(&url, &[]).await?;
        let id = parse_user_id(&body)?;
        *guard = Some(id.clone());
        Ok(id)
    }
}

#[async_trait]
impl FeedSource for XFeedSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn fetch_new_items(&self, since_id: Option<&str>) -> Result<Vec<FeedItem>, FeedError> {
        let id = self.resolve_user_id().await?;
        let url = format!("{}/users/{}/tweets", self.base, id);
        let mut q = vec![
            ("max_results", "20".to_string()),
            ("exclude", "retweets,replies".to_string()),
            ("tweet.fields", "created_at".to_string()),
        ];
        if let Some(s) = since_id {
            q.push(("since_id", s.to_string()));
        }
        let body = self.get(&url, &q).await?;
        parse_timeline(&body, &self.username)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn id_ordering_is_numeric() {
        assert_eq!(cmp_ids("9", "10"), Ordering::Less);
        assert_eq!(cmp_ids("1958924702103347360", "1958924702103347361"), Ordering::Less);
        assert_eq!(cmp_ids("0010", "10"), Ordering::Equal);
        assert_eq!(cmp_ids("200", "199"), Ordering::Greater);
    }

    #[test]
    fn parses_timeline_fixture() {
        let body = r##"{
          "data": [
            {"id":"1958924702103347360","text":"#RIVALS UPDATE 14! Emotes, Battleground Revamp","created_at":"2025-08-22T16:10:00.000Z"},
            {"id":"1958000000000000001","text":"Treasure Quest sale"}
          ],
          "meta": {"newest_id":"1958924702103347360","result_count":2}
        }"##;
        let items = parse_timeline(body, "NosniyGames").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].url, "https://x.com/NosniyGames/status/1958924702103347360");
        assert!(items[0].created_at.is_some());
        assert!(items[1].created_at.is_none());
    }

    #[tokio::test]
    async fn http_client_builds_with_tls() {
        // Guards against rustls crypto-provider conflicts that only show up at runtime.
        let src = XFeedSource::new("https://api.x.com/2/", "token", "@NosniyGames").unwrap();
        assert_eq!(src.name(), "x:nosniygames");
    }

    #[test]
    fn empty_timeline_is_not_an_error() {
        let items = parse_timeline(r#"{"meta":{"result_count":0}}"#, "x").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(matches!(parse_timeline("<html>", "x"), Err(FeedError::Parse(_))));
        assert!(matches!(
            parse_user_id(r#"{"errors":[{"title":"Not Found"}]}"#),
            Err(FeedError::Unavailable(_))
        ));
        assert_eq!(parse_user_id(r#"{"data":{"id":"123","username":"a"}}"#).unwrap(), "123");
    }
}
