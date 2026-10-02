//! Authorization. Everything here is decided from Discord role IDs that come
//! from configuration, never from names or user-supplied data.

use crate::config::RoleConfig;
use crate::error::{Error, Result};
use crate::members::model::ClanRank;
use serenity::model::id::RoleId;

/// Clan authority level derived from Discord roles. Ordered lowest to highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Guest,
    Nix,
    Styx,
    Charon,
    Kerberos,
    Warden,
}

impl Tier {
    /// The clan rank a tier corresponds to (`None` for non-members).
    pub fn clan_rank(self) -> Option<ClanRank> {
        match self {
            Tier::Guest => None,
            Tier::Nix => Some(ClanRank::Nix),
            Tier::Styx => Some(ClanRank::Styx),
            Tier::Charon => Some(ClanRank::Charon),
            Tier::Kerberos => Some(ClanRank::Kerberos),
            Tier::Warden => Some(ClanRank::Warden),
        }
    }
}

/// Fail with a user-safe `Forbidden` error unless `tier` may do `perm`.
pub fn require(tier: Tier, perm: Permission, charon_can_decide: bool) -> Result<()> {
    if allowed(tier, perm, charon_can_decide) {
        Ok(())
    } else {
        Err(Error::forbidden("You are not authorized to do that."))
    }
}

impl RoleConfig {
    /// Highest tier held by a set of roles.
    pub fn tier_of(&self, roles: &[RoleId]) -> Tier {
        let has = |r: RoleId| roles.contains(&r);
        if has(self.warden) {
            Tier::Warden
        } else if has(self.kerberos) {
            Tier::Kerberos
        } else if has(self.charon) {
            Tier::Charon
        } else if has(self.styx) {
            Tier::Styx
        } else if has(self.nix) {
            Tier::Nix
        } else {
            Tier::Guest
        }
    }

    pub fn role_for(&self, rank: crate::members::model::ClanRank) -> RoleId {
        use crate::members::model::ClanRank::*;
        match rank {
            Nix => self.nix,
            Styx => self.styx,
            Charon => self.charon,
            Kerberos => self.kerberos,
            Warden => self.warden,
        }
    }

    pub fn all_rank_roles(&self) -> [RoleId; 5] {
        [self.nix, self.styx, self.charon, self.kerberos, self.warden]
    }
}

impl From<crate::members::model::ClanRank> for Tier {
    fn from(r: crate::members::model::ClanRank) -> Self {
        use crate::members::model::ClanRank::*;
        match r {
            Nix => Tier::Nix,
            Styx => Tier::Styx,
            Charon => Tier::Charon,
            Kerberos => Tier::Kerberos,
            Warden => Tier::Warden,
        }
    }
}

/// What a caller is trying to do. Each variant maps to a minimum tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Any clan member (Nix or above).
    Member,
    /// Read/review applications and view member records (Charon and above).
    Review,
    /// Final accept/reject decisions (Warden, Kerberos; Charon only if configured).
    Decide,
    /// Registry changes, moderation, roster and health (Warden, Kerberos).
    Staff,
}

pub fn allowed(tier: Tier, perm: Permission, charon_can_decide: bool) -> bool {
    match perm {
        Permission::Member => tier >= Tier::Nix,
        Permission::Review => tier >= Tier::Charon,
        Permission::Decide => tier >= Tier::Kerberos || (charon_can_decide && tier >= Tier::Charon),
        Permission::Staff => tier >= Tier::Kerberos,
    }
}

/// May `actor` take a disciplinary action (kick/ban/timeout/warn/remove)
/// against `target`? The actor must be staff and strictly outrank the target,
/// and nobody can act on themselves.
pub fn can_act_on(actor: Tier, target: Tier, same_user: bool) -> bool {
    !same_user && actor >= Tier::Kerberos && actor > target
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles() -> RoleConfig {
        RoleConfig {
            warden: RoleId::new(10),
            kerberos: RoleId::new(11),
            charon: RoleId::new(12),
            styx: RoleId::new(13),
            nix: RoleId::new(14),
        }
    }

    #[test]
    fn tier_is_highest_role_held() {
        let r = roles();
        assert_eq!(r.tier_of(&[]), Tier::Guest);
        assert_eq!(r.tier_of(&[RoleId::new(999)]), Tier::Guest);
        assert_eq!(r.tier_of(&[RoleId::new(14)]), Tier::Nix);
        assert_eq!(r.tier_of(&[RoleId::new(13), RoleId::new(14)]), Tier::Styx);
        assert_eq!(
            r.tier_of(&[RoleId::new(14), RoleId::new(11), RoleId::new(12)]),
            Tier::Kerberos
        );
        assert_eq!(r.tier_of(&[RoleId::new(10), RoleId::new(14)]), Tier::Warden);
    }

    #[test]
    fn decide_is_kerberos_plus_unless_configured() {
        assert!(allowed(Tier::Warden, Permission::Decide, false));
        assert!(allowed(Tier::Kerberos, Permission::Decide, false));
        assert!(!allowed(Tier::Charon, Permission::Decide, false));
        assert!(allowed(Tier::Charon, Permission::Decide, true));
        assert!(!allowed(Tier::Styx, Permission::Decide, true));
    }

    #[test]
    fn review_and_staff_levels() {
        assert!(allowed(Tier::Charon, Permission::Review, false));
        assert!(!allowed(Tier::Styx, Permission::Review, false));
        assert!(!allowed(Tier::Charon, Permission::Staff, true));
        assert!(allowed(Tier::Kerberos, Permission::Staff, false));
        assert!(!allowed(Tier::Guest, Permission::Member, false));
        assert!(allowed(Tier::Nix, Permission::Member, false));
    }

    #[test]
    fn hierarchy_for_discipline() {
        assert!(can_act_on(Tier::Kerberos, Tier::Styx, false));
        assert!(can_act_on(Tier::Warden, Tier::Kerberos, false));
        assert!(!can_act_on(Tier::Kerberos, Tier::Kerberos, false));
        assert!(!can_act_on(Tier::Kerberos, Tier::Warden, false));
        assert!(!can_act_on(Tier::Warden, Tier::Warden, false));
        assert!(!can_act_on(Tier::Warden, Tier::Guest, true));
        assert!(!can_act_on(Tier::Charon, Tier::Guest, false));
        assert!(can_act_on(Tier::Kerberos, Tier::Guest, false));
    }
}
