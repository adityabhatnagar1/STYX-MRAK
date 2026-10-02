//! Member registry model, clan ranks and promotion rules (pure logic).

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::fmt;
use std::str::FromStr;

/// Clan rank. Declaration order is hierarchy order (lowest first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ClanRank {
    Nix,
    Styx,
    Charon,
    Kerberos,
    Warden,
}

impl ClanRank {
    pub const ALL: [ClanRank; 5] = [Self::Nix, Self::Styx, Self::Charon, Self::Kerberos, Self::Warden];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nix => "nix",
            Self::Styx => "styx",
            Self::Charon => "charon",
            Self::Kerberos => "kerberos",
            Self::Warden => "warden",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nix => "NIX",
            Self::Styx => "STYX",
            Self::Charon => "CHARON",
            Self::Kerberos => "KERBEROS",
            Self::Warden => "WARDEN",
        }
    }
}

impl fmt::Display for ClanRank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for ClanRank {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "nix" => Self::Nix,
            "styx" => Self::Styx,
            "charon" => Self::Charon,
            "kerberos" => Self::Kerberos,
            "warden" => Self::Warden,
            other => return Err(Error::validation(format!("Unknown rank `{other}`."))),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Trial,
    Active,
    Inactive,
    Retired,
    Removed,
}

impl MemberStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trial => "trial",
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Retired => "retired",
            Self::Removed => "removed",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Trial => "TRIAL",
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Retired => "RETIRED",
            Self::Removed => "REMOVED",
        }
    }
    /// Currently part of the clan (holds a rank role).
    pub fn is_current(self) -> bool {
        matches!(self, Self::Trial | Self::Active | Self::Inactive)
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct Member {
    pub id: i64,
    pub discord_user_id: i64,
    pub discord_username: String,
    pub roblox_username: Option<String>,
    pub status: MemberStatus,
    pub clan_rank: ClanRank,
    pub squad: Option<String>,
    pub player_level: Option<i64>,
    pub current_rank: Option<String>,
    pub elo: Option<i64>,
    pub win_rate: Option<f64>,
    pub preferred_loadout: Option<String>,
    pub playstyle: Option<String>,
    pub joined_at: DateTime<Utc>,
    pub last_updated_at: DateTime<Utc>,
    pub notes: Option<String>,
}

/// The membership status implied by a clan rank: Nix are on trial.
pub fn status_for_rank(rank: ClanRank) -> MemberStatus {
    if rank == ClanRank::Nix {
        MemberStatus::Trial
    } else {
        MemberStatus::Active
    }
}

/// Highest rank an actor may grant/revoke. Kerberos manage Nix/Styx/Charon;
/// only the Warden may touch Kerberos. Nobody grants Warden through the bot.
fn actor_ceiling(actor: crate::discord::permissions::Tier) -> Option<ClanRank> {
    use crate::discord::permissions::Tier;
    match actor {
        Tier::Warden => Some(ClanRank::Kerberos),
        Tier::Kerberos => Some(ClanRank::Charon),
        _ => None,
    }
}

/// Validate a promotion (`promote == true`) or demotion. Pure rule check;
/// callers pass the actor's tier computed from configured role IDs.
pub fn check_rank_change(
    actor: crate::discord::permissions::Tier,
    from: ClanRank,
    to: ClanRank,
    promote: bool,
) -> Result<()> {
    let Some(ceiling) = actor_ceiling(actor) else {
        return Err(Error::forbidden("Only Warden and Kerberos can change ranks."));
    };
    if from == ClanRank::Warden || to == ClanRank::Warden {
        return Err(Error::forbidden("The Warden rank cannot be changed with the bot."));
    }
    if promote && to <= from {
        return Err(Error::validation(format!(
            "{} is not above {}. Use /member demote to lower a rank.",
            to.label(),
            from.label()
        )));
    }
    if !promote && to >= from {
        return Err(Error::validation(format!(
            "{} is not below {}. Use /member promote to raise a rank.",
            to.label(),
            from.label()
        )));
    }
    if from > ceiling || to > ceiling {
        return Err(Error::forbidden("Only the Warden can grant or remove Kerberos."));
    }
    Ok(())
}

/// `none`, `-` or empty clears the squad.
pub fn validate_squad(raw: &str) -> Result<Option<String>> {
    let s = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() || s.eq_ignore_ascii_case("none") || s == "-" {
        return Ok(None);
    }
    if s.chars().count() > 24 {
        return Err(Error::validation("Squad name is too long (max 24 characters)."));
    }
    if !s
        .chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
    {
        return Err(Error::validation(
            "Squad names may only use letters, numbers, spaces, - and _.",
        ));
    }
    Ok(Some(s))
}

/// Section of `/member update`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateSection {
    Stats,
    Profile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::permissions::Tier;

    #[test]
    fn rank_ordering() {
        assert!(ClanRank::Nix < ClanRank::Styx);
        assert!(ClanRank::Styx < ClanRank::Charon);
        assert!(ClanRank::Charon < ClanRank::Kerberos);
        assert!(ClanRank::Kerberos < ClanRank::Warden);
    }

    #[test]
    fn nix_to_styx_by_kerberos_and_warden() {
        for actor in [Tier::Kerberos, Tier::Warden] {
            assert!(check_rank_change(actor, ClanRank::Nix, ClanRank::Styx, true).is_ok());
        }
    }

    #[test]
    fn unauthorised_actors_cannot_change_ranks() {
        for actor in [Tier::Guest, Tier::Nix, Tier::Styx, Tier::Charon] {
            assert!(check_rank_change(actor, ClanRank::Nix, ClanRank::Styx, true).is_err());
        }
    }

    #[test]
    fn direction_is_enforced() {
        assert!(check_rank_change(Tier::Kerberos, ClanRank::Styx, ClanRank::Styx, true).is_err());
        assert!(check_rank_change(Tier::Kerberos, ClanRank::Styx, ClanRank::Nix, true).is_err());
        assert!(check_rank_change(Tier::Kerberos, ClanRank::Nix, ClanRank::Styx, false).is_err());
        assert!(check_rank_change(Tier::Kerberos, ClanRank::Styx, ClanRank::Nix, false).is_ok());
    }

    #[test]
    fn kerberos_and_warden_ranks_are_protected() {
        assert!(check_rank_change(Tier::Kerberos, ClanRank::Charon, ClanRank::Kerberos, true).is_err());
        assert!(check_rank_change(Tier::Warden, ClanRank::Charon, ClanRank::Kerberos, true).is_ok());
        assert!(check_rank_change(Tier::Kerberos, ClanRank::Kerberos, ClanRank::Styx, false).is_err());
        assert!(check_rank_change(Tier::Warden, ClanRank::Kerberos, ClanRank::Styx, false).is_ok());
        assert!(check_rank_change(Tier::Warden, ClanRank::Charon, ClanRank::Warden, true).is_err());
        assert!(check_rank_change(Tier::Warden, ClanRank::Warden, ClanRank::Styx, false).is_err());
    }

    #[test]
    fn squads() {
        assert_eq!(validate_squad("Alpha").unwrap(), Some("Alpha".into()));
        assert_eq!(validate_squad("  none ").unwrap(), None);
        assert_eq!(validate_squad("-").unwrap(), None);
        assert!(validate_squad("<@123>").is_err());
        assert!(validate_squad(&"a".repeat(25)).is_err());
    }

    #[test]
    fn trial_is_nix() {
        assert_eq!(status_for_rank(ClanRank::Nix), MemberStatus::Trial);
        assert_eq!(status_for_rank(ClanRank::Styx), MemberStatus::Active);
    }
}
