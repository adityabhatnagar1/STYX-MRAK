//! Moderation logic that does not need Discord: duration parsing and the
//! automod rule engine. The engine only *evaluates* messages; deleting,
//! timing out and logging happen in `discord::actions`.
//!
//! Policy: a single ambiguous message is never punished. A violation deletes
//! that message; only repeated violations inside a window escalate to a timeout.

use crate::config::AutomodConfig;
use crate::error::{Error, Result};
use chrono::Duration as ChronoDuration;
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// Discord allows timeouts from 1 minute up to 28 days.
const MIN_TIMEOUT_SECS: i64 = 60;
const MAX_TIMEOUT_SECS: i64 = 28 * 24 * 3600;

/// Parse `10m`, `2h`, `1d`, `1w`. A unit is mandatory.
pub fn parse_duration(input: &str) -> Result<ChronoDuration> {
    let s = input.trim().to_ascii_lowercase();
    let bad = || Error::validation("Duration must look like `10m`, `2h`, `1d` or `1w` (1 minute to 28 days).");
    let unit = s.chars().last().ok_or_else(bad)?;
    let mult: i64 = match unit {
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return Err(bad()),
    };
    let digits = &s[..s.len() - 1];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || digits.len() > 6 {
        return Err(bad());
    }
    let n: i64 = digits.parse().map_err(|_| bad())?;
    let secs = n.checked_mul(mult).ok_or_else(bad)?;
    if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs) {
        return Err(bad());
    }
    Ok(ChronoDuration::seconds(secs))
}

pub fn format_duration(d: ChronoDuration) -> String {
    let secs = d.num_seconds();
    if secs % 604_800 == 0 && secs >= 604_800 {
        format!("{}w", secs / 604_800)
    } else if secs % 86_400 == 0 && secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs % 3600 == 0 && secs >= 3600 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}m", secs / 60)
    }
}

/// Validate a free-text moderation reason.
pub fn validate_reason(raw: &str) -> Result<String> {
    let s = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        return Err(Error::validation("A reason is required."));
    }
    if s.chars().count() > 400 {
        return Err(Error::validation("Reason is too long (max 400 characters)."));
    }
    Ok(s)
}

pub fn validate_purge_amount(n: i64) -> Result<u8> {
    if (1..=100).contains(&n) {
        Ok(n as u8)
    } else {
        Err(Error::validation("Purge amount must be between 1 and 100."))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    Invite,
    BannedWord(String),
    MentionSpam,
    Repeat,
    Flood,
}

impl Violation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Invite => "invite link",
            Self::BannedWord(_) => "banned word",
            Self::MentionSpam => "mention spam",
            Self::Repeat => "repeated message",
            Self::Flood => "message flood",
        }
    }
}

#[derive(Default)]
struct UserTrack {
    recent: VecDeque<(Instant, u64)>,
    strikes: VecDeque<Instant>,
}

pub struct Automod {
    cfg: AutomodConfig,
    invite_re: Regex,
    banned_re: Option<Regex>,
    users: HashMap<u64, UserTrack>,
}

fn hash_content(s: &str) -> u64 {
    let norm: String = s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let mut h = DefaultHasher::new();
    norm.hash(&mut h);
    h.finish()
}

impl Automod {
    pub fn new(cfg: AutomodConfig) -> Self {
        let invite_re =
            Regex::new(r"(?i)(?:discord\.gg|discord(?:app)?\.com/invite|dsc\.gg)/[a-z0-9-]+").expect("static regex");
        let words: Vec<String> = cfg
            .banned_words
            .iter()
            .filter(|w| !w.trim().is_empty())
            .map(|w| regex::escape(w.trim()))
            .collect();
        let banned_re = if words.is_empty() {
            None
        } else {
            Regex::new(&format!(
                r"(?i)(?:^|[^\p{{L}}\p{{N}}])({})(?:$|[^\p{{L}}\p{{N}}])",
                words.join("|")
            ))
            .ok()
        };
        Self {
            cfg,
            invite_re,
            banned_re,
            users: HashMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// Evaluate one message. `mentions` is the number of distinct user/role mentions.
    pub fn check(&mut self, user: u64, content: &str, mentions: usize, now: Instant) -> Option<Violation> {
        if !self.cfg.enabled {
            return None;
        }
        if self.users.len() > 2000 {
            self.prune(now);
        }
        // Track for flood/repeat first so every message counts.
        let flood_window = Duration::from_secs(self.cfg.flood_window_seconds);
        let repeat_window = Duration::from_secs(self.cfg.repeat_window_seconds);
        let keep = flood_window.max(repeat_window);
        let hash = hash_content(content);
        let track = self.users.entry(user).or_default();
        while track.recent.front().is_some_and(|(t, _)| now.duration_since(*t) > keep) {
            track.recent.pop_front();
        }
        track.recent.push_back((now, hash));
        let in_flood = track
            .recent
            .iter()
            .filter(|(t, _)| now.duration_since(*t) <= flood_window)
            .count();
        let repeats = track
            .recent
            .iter()
            .filter(|(t, h)| *h == hash && now.duration_since(*t) <= repeat_window)
            .count();

        if self.cfg.block_invites && self.invite_re.is_match(content) {
            return Some(Violation::Invite);
        }
        if let Some(re) = &self.banned_re {
            if let Some(c) = re.captures(content) {
                return Some(Violation::BannedWord(c[1].to_lowercase()));
            }
        }
        if mentions > self.cfg.max_mentions {
            return Some(Violation::MentionSpam);
        }
        if !content.trim().is_empty() && repeats >= self.cfg.repeat_threshold {
            return Some(Violation::Repeat);
        }
        if in_flood >= self.cfg.flood_messages {
            return Some(Violation::Flood);
        }
        None
    }

    /// Record a strike. Returns `true` when the user has reached the strike
    /// limit inside the window and should be timed out; strikes then reset.
    pub fn record_strike(&mut self, user: u64, now: Instant) -> bool {
        let window = Duration::from_secs(self.cfg.strike_window_seconds);
        let limit = self.cfg.strikes_before_timeout.max(1);
        let track = self.users.entry(user).or_default();
        while track.strikes.front().is_some_and(|t| now.duration_since(*t) > window) {
            track.strikes.pop_front();
        }
        track.strikes.push_back(now);
        if track.strikes.len() >= limit {
            track.strikes.clear();
            true
        } else {
            false
        }
    }

    pub fn timeout_minutes(&self) -> u64 {
        self.cfg.timeout_minutes
    }

    fn prune(&mut self, now: Instant) {
        let keep = Duration::from_secs(
            self.cfg
                .flood_window_seconds
                .max(self.cfg.repeat_window_seconds)
                .max(self.cfg.strike_window_seconds),
        );
        self.users.retain(|_, t| {
            t.recent.back().is_some_and(|(at, _)| now.duration_since(*at) <= keep)
                || t.strikes.back().is_some_and(|at| now.duration_since(*at) <= keep)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AutomodConfig {
        AutomodConfig {
            enabled: true,
            banned_words: vec!["badword".into(), "slur x".into()],
            block_invites: true,
            max_mentions: 5,
            flood_messages: 6,
            flood_window_seconds: 8,
            repeat_threshold: 3,
            repeat_window_seconds: 30,
            strikes_before_timeout: 3,
            strike_window_seconds: 600,
            timeout_minutes: 10,
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("10m").unwrap(), ChronoDuration::minutes(10));
        assert_eq!(parse_duration(" 2H ").unwrap(), ChronoDuration::hours(2));
        assert_eq!(parse_duration("1d").unwrap(), ChronoDuration::days(1));
        assert_eq!(parse_duration("4w").unwrap(), ChronoDuration::weeks(4));
        for bad in [
            "",
            "m",
            "10",
            "0m",
            "-5m",
            "5x",
            "1.5h",
            "5w1d",
            "29d",
            "5w",
            "99999999999m",
            "1 0m",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
        assert!(parse_duration("28d").is_ok());
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(ChronoDuration::minutes(10)), "10m");
        assert_eq!(format_duration(ChronoDuration::hours(2)), "2h");
        assert_eq!(format_duration(ChronoDuration::days(3)), "3d");
        assert_eq!(format_duration(ChronoDuration::weeks(2)), "2w");
        assert_eq!(format_duration(ChronoDuration::minutes(90)), "90m");
    }

    #[test]
    fn reasons_and_purge_bounds() {
        assert!(validate_reason("  spam   in #general ").is_ok());
        assert!(validate_reason("   ").is_err());
        assert!(validate_reason(&"x".repeat(401)).is_err());
        assert_eq!(validate_purge_amount(50).unwrap(), 50);
        assert!(validate_purge_amount(0).is_err());
        assert!(validate_purge_amount(101).is_err());
    }

    #[test]
    fn invite_and_banned_words() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        assert_eq!(a.check(1, "join discord.gg/abc123 now", 0, t), Some(Violation::Invite));
        assert_eq!(
            a.check(1, "https://discord.com/invite/xyz", 0, t),
            Some(Violation::Invite)
        );
        assert_eq!(
            a.check(1, "you BadWord!", 0, t),
            Some(Violation::BannedWord("badword".into()))
        );
        // Word boundaries: no false positive inside a longer word.
        assert_eq!(a.check(1, "unbadwordly text", 0, t), None);
        assert_eq!(a.check(1, "gg that was a clean round", 0, t), None);
    }

    #[test]
    fn mention_spam() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        assert_eq!(a.check(1, "hi", 5, t), None);
        assert_eq!(a.check(1, "hi 2", 6, t), Some(Violation::MentionSpam));
    }

    #[test]
    fn repeats_need_threshold_and_window() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        assert_eq!(a.check(1, "buy now", 0, t), None);
        assert_eq!(a.check(1, "BUY   now", 0, t + Duration::from_secs(1)), None);
        assert_eq!(
            a.check(1, "buy now", 0, t + Duration::from_secs(2)),
            Some(Violation::Repeat)
        );
        // Outside the window the old copies no longer count.
        let mut b = Automod::new(cfg());
        b.check(2, "same", 0, t);
        b.check(2, "same", 0, t + Duration::from_secs(1));
        assert_eq!(b.check(2, "same", 0, t + Duration::from_secs(100)), None);
    }

    #[test]
    fn flood_detection() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        let mut hit = None;
        for i in 0..6 {
            hit = a.check(1, &format!("msg {i}"), 0, t + Duration::from_millis(i * 100));
        }
        assert_eq!(hit, Some(Violation::Flood));
        // A normal pace never trips it.
        let mut b = Automod::new(cfg());
        for i in 0..20 {
            assert_eq!(b.check(2, &format!("m{i}"), 0, t + Duration::from_secs(i * 5)), None);
        }
    }

    #[test]
    fn users_are_tracked_independently() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        a.check(1, "x", 0, t);
        a.check(1, "x", 0, t);
        assert_eq!(a.check(2, "x", 0, t), None);
    }

    #[test]
    fn single_strike_never_times_out() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        assert!(!a.record_strike(1, t));
        assert!(!a.record_strike(1, t + Duration::from_secs(1)));
        assert!(a.record_strike(1, t + Duration::from_secs(2)));
        // Strikes reset after escalation.
        assert!(!a.record_strike(1, t + Duration::from_secs(3)));
    }

    #[test]
    fn old_strikes_expire() {
        let mut a = Automod::new(cfg());
        let t = Instant::now();
        a.record_strike(1, t);
        a.record_strike(1, t + Duration::from_secs(1));
        assert!(!a.record_strike(1, t + Duration::from_secs(1000)));
    }

    #[test]
    fn disabled_engine_flags_nothing() {
        let mut c = cfg();
        c.enabled = false;
        let mut a = Automod::new(c);
        assert_eq!(a.check(1, "discord.gg/abc", 50, Instant::now()), None);
    }
}
