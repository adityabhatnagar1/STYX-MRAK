//! Environment-based configuration with an optional TOML file for tunables.
//!
//! Secrets and Discord IDs come from the environment (`.env`). Tunables that
//! are awkward as env vars (word lists, thresholds) live in `config.toml`.

use crate::error::{Error, Result};
use serde::Deserialize;
use serenity::model::id::{ChannelId, GuildId, RoleId};
use std::fmt;

#[derive(Clone, Copy, Debug)]
pub struct RoleConfig {
    pub warden: RoleId,
    pub kerberos: RoleId,
    pub charon: RoleId,
    pub styx: RoleId,
    pub nix: RoleId,
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelConfig {
    pub apply: ChannelId,
    pub roster: ChannelId,
    pub announcements: ChannelId,
    pub staff: ChannelId,
    pub logs: ChannelId,
}

#[derive(Clone, Debug)]
pub struct FeedConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub x_bearer_token: Option<String>,
    pub x_username: String,
    pub x_api_base: String,
    pub include_keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    pub source_label: String,
    /// Announce the existing backlog on the very first run. Off by default so
    /// a fresh install does not spam #announcements.
    pub announce_backlog: bool,
}

#[derive(Clone, Debug)]
pub struct AutomodConfig {
    pub enabled: bool,
    pub banned_words: Vec<String>,
    pub block_invites: bool,
    pub max_mentions: usize,
    pub flood_messages: usize,
    pub flood_window_seconds: u64,
    pub repeat_threshold: usize,
    pub repeat_window_seconds: u64,
    pub strikes_before_timeout: usize,
    pub strike_window_seconds: u64,
    pub timeout_minutes: u64,
}

#[derive(Clone)]
pub struct Config {
    pub discord_token: String,
    pub guild_id: GuildId,
    pub roles: RoleConfig,
    pub channels: ChannelConfig,
    pub database_url: String,
    /// When true, Charon may take final accept/reject decisions. Default false.
    pub charon_can_decide: bool,
    pub feed: FeedConfig,
    pub automod: AutomodConfig,
}

// Manual Debug so the token can never reach a log line.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("discord_token", &"<redacted>")
            .field("guild_id", &self.guild_id)
            .field("roles", &self.roles)
            .field("channels", &self.channels)
            .field("database_url", &"<redacted>")
            .field("charon_can_decide", &self.charon_can_decide)
            .field("feed", &self.feed.enabled)
            .field("automod", &self.automod.enabled)
            .finish()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub automod: FileAutomod,
    pub feed: FileFeed,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FileAutomod {
    pub banned_words: Option<Vec<String>>,
    pub block_invites: Option<bool>,
    pub max_mentions: Option<usize>,
    pub flood_messages: Option<usize>,
    pub flood_window_seconds: Option<u64>,
    pub repeat_threshold: Option<usize>,
    pub repeat_window_seconds: Option<u64>,
    pub strikes_before_timeout: Option<usize>,
    pub strike_window_seconds: Option<u64>,
    pub timeout_minutes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct FileFeed {
    pub include_keywords: Option<Vec<String>>,
    pub exclude_keywords: Option<Vec<String>>,
    pub source_label: Option<String>,
    pub x_api_base: Option<String>,
    pub announce_backlog: Option<bool>,
}

impl Config {
    /// Load from the process environment plus `CONFIG_FILE` (default
    /// `config.toml`, optional).
    pub fn load() -> Result<Self> {
        let path = std::env::var("CONFIG_FILE").unwrap_or_else(|_| "config.toml".into());
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<FileConfig>(&text).map_err(|e| Error::Config(format!("{path}: {e}")))?,
            Err(_) => FileConfig::default(),
        };
        Self::from_lookup(|k| std::env::var(k).ok(), file)
    }

    /// Testable constructor: `lookup` stands in for the environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>, file: FileConfig) -> Result<Self> {
        let get = |k: &str| lookup(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let req = |k: &str| get(k).ok_or_else(|| Error::Config(format!("{k} is required")));
        let id = |k: &str| -> Result<u64> {
            let raw = req(k)?;
            let n: u64 = raw
                .parse()
                .map_err(|_| Error::Config(format!("{k} must be a numeric Discord ID")))?;
            if n == 0 {
                return Err(Error::Config(format!("{k} must not be 0")));
            }
            Ok(n)
        };
        let flag = |k: &str, default: bool| -> Result<bool> {
            match get(k).as_deref().map(str::to_ascii_lowercase).as_deref() {
                None => Ok(default),
                Some("1" | "true" | "yes" | "on") => Ok(true),
                Some("0" | "false" | "no" | "off") => Ok(false),
                Some(_) => Err(Error::Config(format!("{k} must be true or false"))),
            }
        };

        let roles = RoleConfig {
            warden: RoleId::new(id("ROLE_WARDEN")?),
            kerberos: RoleId::new(id("ROLE_KERBEROS")?),
            charon: RoleId::new(id("ROLE_CHARON")?),
            styx: RoleId::new(id("ROLE_STYX")?),
            nix: RoleId::new(id("ROLE_NIX")?),
        };
        let ids = [roles.warden, roles.kerberos, roles.charon, roles.styx, roles.nix];
        for (i, a) in ids.iter().enumerate() {
            if ids[i + 1..].contains(a) {
                return Err(Error::Config("ROLE_* IDs must all be different".into()));
            }
        }
        let channels = ChannelConfig {
            apply: ChannelId::new(id("CHANNEL_APPLY")?),
            roster: ChannelId::new(id("CHANNEL_ROSTER")?),
            announcements: ChannelId::new(id("CHANNEL_ANNOUNCEMENTS")?),
            staff: ChannelId::new(id("CHANNEL_STAFF")?),
            logs: ChannelId::new(id("CHANNEL_LOGS")?),
        };

        let interval = match get("FEED_INTERVAL_SECONDS") {
            None => 300,
            Some(v) => v
                .parse::<u64>()
                .map_err(|_| Error::Config("FEED_INTERVAL_SECONDS must be a number".into()))?,
        };
        if interval < 30 {
            return Err(Error::Config("FEED_INTERVAL_SECONDS must be at least 30".into()));
        }
        let lower = |v: Vec<String>| v.into_iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
        let feed = FeedConfig {
            enabled: flag("FEED_ENABLED", false)?,
            interval_seconds: interval,
            x_bearer_token: get("FEED_X_BEARER_TOKEN"),
            x_username: get("FEED_X_USERNAME").unwrap_or_else(|| "NosniyGames".into()),
            x_api_base: file.feed.x_api_base.unwrap_or_else(|| "https://api.x.com/2".into()),
            include_keywords: lower(file.feed.include_keywords.unwrap_or_else(|| vec!["rivals".into()])),
            exclude_keywords: lower(file.feed.exclude_keywords.unwrap_or_else(|| {
                vec![
                    "treasure quest".into(),
                    "super golf".into(),
                    "noodle arms".into(),
                    "black hole simulator".into(),
                    "halloween simulator".into(),
                ]
            })),
            source_label: file.feed.source_label.unwrap_or_else(|| "Nosniy Games".into()),
            announce_backlog: file.feed.announce_backlog.unwrap_or(false),
        };
        if feed.enabled && feed.x_bearer_token.is_none() {
            return Err(Error::Config("FEED_ENABLED=true requires FEED_X_BEARER_TOKEN".into()));
        }

        let a = file.automod;
        let automod = AutomodConfig {
            enabled: flag("AUTOMOD_ENABLED", false)?,
            banned_words: lower(a.banned_words.unwrap_or_default()),
            block_invites: a.block_invites.unwrap_or(true),
            max_mentions: a.max_mentions.unwrap_or(5),
            flood_messages: a.flood_messages.unwrap_or(6),
            flood_window_seconds: a.flood_window_seconds.unwrap_or(8),
            repeat_threshold: a.repeat_threshold.unwrap_or(3),
            repeat_window_seconds: a.repeat_window_seconds.unwrap_or(30),
            strikes_before_timeout: a.strikes_before_timeout.unwrap_or(3),
            strike_window_seconds: a.strike_window_seconds.unwrap_or(600),
            timeout_minutes: a.timeout_minutes.unwrap_or(10),
        };

        Ok(Self {
            discord_token: req("DISCORD_TOKEN")?,
            guild_id: GuildId::new(id("GUILD_ID")?),
            roles,
            channels,
            database_url: get("DATABASE_URL").unwrap_or_else(|| "sqlite://data/styx.db".into()),
            charon_can_decide: flag("CHARON_CAN_DECIDE", false)?,
            feed,
            automod,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;

    pub fn base_env() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("DISCORD_TOKEN", "test-token"),
            ("GUILD_ID", "1"),
            ("ROLE_WARDEN", "10"),
            ("ROLE_KERBEROS", "11"),
            ("ROLE_CHARON", "12"),
            ("ROLE_STYX", "13"),
            ("ROLE_NIX", "14"),
            ("CHANNEL_APPLY", "20"),
            ("CHANNEL_ROSTER", "21"),
            ("CHANNEL_ANNOUNCEMENTS", "22"),
            ("CHANNEL_STAFF", "23"),
            ("CHANNEL_LOGS", "24"),
        ])
    }

    pub fn test_config() -> Config {
        let env = base_env();
        Config::from_lookup(|k| env.get(k).map(|v| v.to_string()), FileConfig::default()).unwrap()
    }

    #[test]
    fn loads_valid_config_with_defaults() {
        let c = test_config();
        assert_eq!(c.roles.warden, RoleId::new(10));
        assert_eq!(c.database_url, "sqlite://data/styx.db");
        assert!(!c.feed.enabled);
        assert!(!c.charon_can_decide);
        assert_eq!(c.feed.interval_seconds, 300);
    }

    #[test]
    fn missing_or_bad_ids_are_rejected() {
        let mut env = base_env();
        env.remove("ROLE_NIX");
        assert!(Config::from_lookup(|k| env.get(k).map(|v| v.to_string()), FileConfig::default()).is_err());
        let mut env = base_env();
        env.insert("CHANNEL_LOGS", "abc");
        assert!(Config::from_lookup(|k| env.get(k).map(|v| v.to_string()), FileConfig::default()).is_err());
        let mut env = base_env();
        env.insert("ROLE_NIX", "10");
        assert!(Config::from_lookup(|k| env.get(k).map(|v| v.to_string()), FileConfig::default()).is_err());
    }

    #[test]
    fn feed_requires_token_when_enabled() {
        let mut env = base_env();
        env.insert("FEED_ENABLED", "true");
        assert!(Config::from_lookup(|k| env.get(k).map(|v| v.to_string()), FileConfig::default()).is_err());
        env.insert("FEED_X_BEARER_TOKEN", "abc");
        assert!(Config::from_lookup(|k| env.get(k).map(|v| v.to_string()), FileConfig::default()).is_ok());
    }

    #[test]
    fn debug_output_never_contains_token() {
        let c = test_config();
        assert!(!format!("{c:?}").contains("test-token"));
    }
}
