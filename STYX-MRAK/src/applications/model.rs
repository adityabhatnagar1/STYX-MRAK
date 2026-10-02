//! Application data model, input validation and the status state machine.
//!
//! Everything in here is pure (no I/O) so it can be unit-tested aggressively.

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ApplicationStatus {
    Pending,
    UnderReview,
    Accepted,
    Rejected,
    Withdrawn,
}

impl ApplicationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::UnderReview => "under_review",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::UnderReview => "UNDER REVIEW",
            Self::Accepted => "ACCEPTED",
            Self::Rejected => "REJECTED",
            Self::Withdrawn => "WITHDRAWN",
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::UnderReview)
    }

    /// The application state machine. Terminal states have no exits.
    pub fn can_transition_to(self, next: Self) -> bool {
        use ApplicationStatus::*;
        matches!(
            (self, next),
            (Pending, UnderReview)
                | (Pending, Accepted)
                | (Pending, Rejected)
                | (Pending, Withdrawn)
                | (UnderReview, Accepted)
                | (UnderReview, Rejected)
                | (UnderReview, Withdrawn)
        )
    }

    pub fn ensure_transition(self, next: Self) -> Result<()> {
        if self.can_transition_to(next) {
            Ok(())
        } else {
            Err(Error::conflict(format!(
                "This application is already {} and cannot become {}.",
                self.label().to_lowercase(),
                next.label().to_lowercase()
            )))
        }
    }
}

impl fmt::Display for ApplicationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ApplicationStatus {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "pending" => Self::Pending,
            "under_review" | "under review" => Self::UnderReview,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            "withdrawn" => Self::Withdrawn,
            other => return Err(Error::validation(format!("Unknown status `{other}`."))),
        })
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct Application {
    pub id: i64,
    pub discord_user_id: i64,
    pub discord_username: String,
    pub roblox_username: String,
    pub player_level: i64,
    pub current_rank: String,
    pub elo: i64,
    pub win_rate: f64,
    pub preferred_loadout: String,
    pub playstyle: String,
    pub timezone: String,
    pub motivation: String,
    pub status: ApplicationStatus,
    pub submitted_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub reviewed_by: Option<i64>,
    pub review_notes: Option<String>,
    pub info_request_text: Option<String>,
    pub review_channel_id: Option<i64>,
    pub review_message_id: Option<i64>,
}

/// Validated step-1 answers, persisted as a draft between the two modals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepOne {
    pub roblox_username: String,
    pub player_level: i64,
    pub current_rank: String,
    pub elo: i64,
    pub win_rate: f64,
}

/// Validated step-2 answers.
#[derive(Clone, Debug, PartialEq)]
pub struct StepTwo {
    pub preferred_loadout: String,
    pub playstyle: String,
    pub timezone: String,
    pub motivation: String,
}

/// A complete, validated application ready to insert.
#[derive(Clone, Debug, PartialEq)]
pub struct NewApplication {
    pub discord_user_id: i64,
    pub discord_username: String,
    pub one: StepOne,
    pub two: StepTwo,
}

// ---------------------------------------------------------------------------
// Field validation
// ---------------------------------------------------------------------------

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn reject_credentials(field: &str, s: &str) -> Result<()> {
    let l = s.to_lowercase();
    let markers = [
        "roblosecurity",
        "do-not-share-this",
        "password",
        "2fa code",
        "authenticator code",
    ];
    if markers.iter().any(|m| l.contains(m)) {
        return Err(Error::validation(format!(
            "{field}: never share passwords, cookies or codes. STYX will never ask for them."
        )));
    }
    Ok(())
}

pub fn validate_roblox_username(raw: &str) -> Result<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(Error::validation("Roblox username is required."));
    }
    if s.chars().count() > 20 {
        return Err(Error::validation("Roblox username is too long (max 20 characters)."));
    }
    if s.chars().count() < 3 {
        return Err(Error::validation("Roblox username is too short (min 3 characters)."));
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::validation(
            "Roblox username may only contain letters, numbers and underscores.",
        ));
    }
    if s.starts_with('_') || s.ends_with('_') {
        return Err(Error::validation(
            "Roblox username cannot start or end with an underscore.",
        ));
    }
    Ok(s.to_string())
}

fn parse_whole_number(field: &str, raw: &str) -> Result<i64> {
    let s: String = raw
        .trim()
        .chars()
        .filter(|c| *c != ',' && *c != '_' && !c.is_whitespace())
        .collect();
    if s.is_empty() {
        return Err(Error::validation(format!("{field} is required.")));
    }
    s.parse::<i64>()
        .map_err(|_| Error::validation(format!("{field} must be a whole number.")))
}

pub fn parse_level(raw: &str) -> Result<i64> {
    let n = parse_whole_number("Level", raw)?;
    if !(1..=10_000).contains(&n) {
        return Err(Error::validation("Level must be between 1 and 10000."));
    }
    Ok(n)
}

pub fn parse_elo(raw: &str) -> Result<i64> {
    let n = parse_whole_number("ELO", raw)?;
    if !(0..=10_000).contains(&n) {
        return Err(Error::validation("ELO must be between 0 and 10000."));
    }
    Ok(n)
}

/// Accepts `75%`, `75`, `75.5%`, `0.75`. Returns a percentage in `0.0..=100.0`.
///
/// Without a `%`, a value with a decimal point that is `<= 1.0` is treated as
/// a fraction (`0.75` -> 75%). A bare `1` is 1%, not 100%.
pub fn parse_win_rate(raw: &str) -> Result<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(Error::validation("Win rate is required."));
    }
    let (num, has_pct) = match s.strip_suffix('%') {
        Some(n) => (n.trim(), true),
        None => (s, false),
    };
    let v: f64 = num
        .parse()
        .map_err(|_| Error::validation("Win rate must look like 75%, 75 or 0.75."))?;
    if !v.is_finite() {
        return Err(Error::validation("Win rate must be a real number."));
    }
    let pct = if !has_pct && num.contains('.') && v <= 1.0 {
        v * 100.0
    } else {
        v
    };
    if !(0.0..=100.0).contains(&pct) {
        return Err(Error::validation("Win rate must be between 0% and 100%."));
    }
    Ok((pct * 100.0).round() / 100.0)
}

/// Ranks are self-reported and the game's rank names change, so they are
/// validated for shape only, not against a fixed list.
pub fn validate_rank(raw: &str) -> Result<String> {
    let s = clean(raw);
    if s.is_empty() {
        return Err(Error::validation("Rank is required."));
    }
    if s.chars().count() > 32 {
        return Err(Error::validation("Rank is too long (max 32 characters)."));
    }
    if !s.chars().next().is_some_and(char::is_alphabetic)
        || !s
            .chars()
            .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '+')
    {
        return Err(Error::validation("Rank must be plain text like `Diamond 2`."));
    }
    Ok(s)
}

fn text_field(field: &str, raw: &str, min: usize, max: usize) -> Result<String> {
    let s = clean(raw);
    let n = s.chars().count();
    if n == 0 {
        return Err(Error::validation(format!("{field} is required.")));
    }
    if n < min {
        return Err(Error::validation(format!(
            "{field} is too short (min {min} characters)."
        )));
    }
    if n > max {
        return Err(Error::validation(format!(
            "{field} is too long (max {max} characters)."
        )));
    }
    if s.chars().any(char::is_control) {
        return Err(Error::validation(format!("{field} contains invalid characters.")));
    }
    reject_credentials(field, &s)?;
    Ok(s)
}

pub fn validate_step_one(roblox: &str, level: &str, rank: &str, elo: &str, win_rate: &str) -> Result<StepOne> {
    Ok(StepOne {
        roblox_username: validate_roblox_username(roblox)?,
        player_level: parse_level(level)?,
        current_rank: validate_rank(rank)?,
        elo: parse_elo(elo)?,
        win_rate: parse_win_rate(win_rate)?,
    })
}

pub fn validate_step_two(loadout: &str, playstyle: &str, timezone: &str, motivation: &str) -> Result<StepTwo> {
    Ok(StepTwo {
        preferred_loadout: text_field("Loadout", loadout, 2, 100)?,
        playstyle: text_field("Playstyle", playstyle, 2, 100)?,
        timezone: text_field("Timezone", timezone, 1, 32)?,
        motivation: text_field("Motivation", motivation, 10, 1000)?,
    })
}

/// Free-text reply / question / reason (info requests, rejection reasons).
pub fn validate_free_text(field: &str, raw: &str, max: usize) -> Result<String> {
    text_field(field, raw, 2, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ApplicationStatus::*;

    #[test]
    fn win_rate_formats() {
        assert_eq!(parse_win_rate("75%").unwrap(), 75.0);
        assert_eq!(parse_win_rate("75").unwrap(), 75.0);
        assert_eq!(parse_win_rate("0.75").unwrap(), 75.0);
        assert_eq!(parse_win_rate(" 71.5 % ").unwrap(), 71.5);
        assert_eq!(parse_win_rate("1").unwrap(), 1.0);
        assert_eq!(parse_win_rate("0").unwrap(), 0.0);
        assert_eq!(parse_win_rate("100%").unwrap(), 100.0);
    }

    #[test]
    fn win_rate_rejects_garbage() {
        for bad in ["101%", "-5", "abc", "", "NaN", "inf", "150", "%", "7 5%"] {
            assert!(parse_win_rate(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn elo_and_level() {
        assert_eq!(parse_elo("1800").unwrap(), 1800);
        assert_eq!(parse_elo("1,840").unwrap(), 1840);
        assert!(parse_elo("abc").is_err());
        assert!(parse_elo("-1").is_err());
        assert!(parse_elo("99999").is_err());
        assert!(parse_elo("").is_err());
        assert_eq!(parse_level("312").unwrap(), 312);
        assert!(parse_level("-1").is_err());
        assert!(parse_level("0").is_err());
        assert!(parse_level("3.5").is_err());
        assert!(parse_level("99999999999999999999").is_err());
    }

    #[test]
    fn usernames() {
        assert_eq!(validate_roblox_username("  Cool_Guy1 ").unwrap(), "Cool_Guy1");
        for bad in [
            "",
            "  ",
            "ab",
            "_lead",
            "trail_",
            "has space",
            "emoji😀name",
            &"a".repeat(21),
            "<@123>",
        ] {
            assert!(validate_roblox_username(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn ranks() {
        assert_eq!(validate_rank("  Diamond   2 ").unwrap(), "Diamond 2");
        assert!(validate_rank("").is_err());
        assert!(validate_rank("2 Diamond").is_err());
        assert!(validate_rank(&"x".repeat(33)).is_err());
        assert!(validate_rank("Gold\n@everyone").is_err());
    }

    #[test]
    fn step_two_rules() {
        let ok = validate_step_two(
            "Shotgun / Uzi / Knife / Flash",
            "Aggressive entry",
            "IST",
            "I want to play with a real squad.",
        );
        assert!(ok.is_ok());
        assert!(validate_step_two("", "x", "IST", "long enough motivation").is_err());
        assert!(validate_step_two("Shotgun", "Aggro", "IST", "short").is_err());
        assert!(validate_step_two("Shotgun", "Aggro", "IST", &"x".repeat(1001)).is_err());
    }

    #[test]
    fn credentials_are_refused() {
        assert!(validate_step_two("Shotgun", "Aggro", "IST", "my password is hunter2 lol").is_err());
        assert!(validate_step_two("Shotgun", "Aggro", "IST", "_|WARNING:-DO-NOT-SHARE-THIS.--abc").is_err());
    }

    #[test]
    fn state_machine() {
        assert!(Pending.can_transition_to(UnderReview));
        assert!(Pending.can_transition_to(Accepted));
        assert!(UnderReview.can_transition_to(Rejected));
        assert!(UnderReview.can_transition_to(Withdrawn));
        for terminal in [Accepted, Rejected, Withdrawn] {
            for next in [Pending, UnderReview, Accepted, Rejected, Withdrawn] {
                assert!(!terminal.can_transition_to(next), "{terminal} -> {next}");
            }
        }
        assert!(!UnderReview.can_transition_to(Pending));
        assert!(Accepted.ensure_transition(Rejected).is_err());
    }

    #[test]
    fn status_round_trip() {
        for s in [Pending, UnderReview, Accepted, Rejected, Withdrawn] {
            assert_eq!(s.as_str().parse::<ApplicationStatus>().unwrap(), s);
        }
        assert!("nope".parse::<ApplicationStatus>().is_err());
    }
}
