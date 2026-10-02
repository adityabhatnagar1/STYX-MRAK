//! Registry operations. Each state change is one transaction that also writes
//! the audit event.

use super::model::{check_rank_change, status_for_rank, ClanRank, Member, MemberStatus};
use crate::db::repositories::{self as repo, AuditEvent, StatsUpdate};
use crate::discord::permissions::Tier;
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;

pub async fn require_current(pool: &SqlitePool, user: i64) -> Result<Member> {
    match repo::get_member(pool, user).await? {
        Some(m) if m.status.is_current() => Ok(m),
        Some(_) => Err(Error::not_found("That user is no longer an active clan member.")),
        None => Err(Error::not_found("That user is not in the registry.")),
    }
}

pub struct RankChange {
    pub before: Member,
    pub after: Member,
}

pub async fn change_rank(
    pool: &SqlitePool,
    actor_id: i64,
    actor_tier: Tier,
    target: i64,
    to: ClanRank,
    promote: bool,
    now: DateTime<Utc>,
) -> Result<RankChange> {
    let mut tx = pool.begin().await?;
    let before = match repo::get_member(&mut *tx, target).await? {
        Some(m) if m.status.is_current() => m,
        _ => return Err(Error::not_found("That user is not a current clan member.")),
    };
    check_rank_change(actor_tier, before.clan_rank, to, promote)?;
    let after = repo::set_member_rank(&mut *tx, target, to, status_for_rank(to), now).await?;
    repo::record_audit(
        &mut *tx,
        Some(actor_id),
        Some(target),
        if promote {
            AuditEvent::MemberPromoted
        } else {
            AuditEvent::MemberDemoted
        },
        json!({ "from": before.clan_rank.as_str(), "to": to.as_str() }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(RankChange { before, after })
}

pub async fn set_squad(
    pool: &SqlitePool,
    actor_id: i64,
    target: i64,
    squad: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Member> {
    let mut tx = pool.begin().await?;
    let before = require_current_tx(&mut tx, target).await?;
    let after = repo::set_member_squad(&mut *tx, target, squad, now).await?;
    repo::record_audit(
        &mut *tx,
        Some(actor_id),
        Some(target),
        AuditEvent::SquadChanged,
        json!({ "from": before.squad, "to": squad }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(after)
}

async fn require_current_tx(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, user: i64) -> Result<Member> {
    match repo::get_member(&mut **tx, user).await? {
        Some(m) if m.status.is_current() => Ok(m),
        _ => Err(Error::not_found("That user is not a current clan member.")),
    }
}

pub async fn update_stats(
    pool: &SqlitePool,
    actor_id: i64,
    target: i64,
    update: &StatsUpdate<'_>,
    now: DateTime<Utc>,
) -> Result<Member> {
    let mut tx = pool.begin().await?;
    require_current_tx(&mut tx, target).await?;
    let after = repo::update_member_stats(&mut *tx, target, update, now).await?;
    repo::record_audit(
        &mut *tx,
        Some(actor_id),
        Some(target),
        AuditEvent::MemberUpdated,
        json!({ "section": "stats" }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(after)
}

pub async fn update_profile(
    pool: &SqlitePool,
    actor_id: i64,
    target: i64,
    loadout: Option<&str>,
    playstyle: Option<&str>,
    notes: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Member> {
    let mut tx = pool.begin().await?;
    require_current_tx(&mut tx, target).await?;
    let after = repo::update_member_profile(&mut *tx, target, loadout, playstyle, notes, now).await?;
    repo::record_audit(
        &mut *tx,
        Some(actor_id),
        Some(target),
        AuditEvent::MemberUpdated,
        json!({ "section": "profile" }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(after)
}

/// Mark a member removed. The record is kept; only the status changes.
pub async fn remove(
    pool: &SqlitePool,
    actor_id: i64,
    actor_tier: Tier,
    target: i64,
    now: DateTime<Utc>,
) -> Result<Member> {
    let mut tx = pool.begin().await?;
    let before = require_current_tx(&mut tx, target).await?;
    if !crate::discord::permissions::can_act_on(actor_tier, Tier::from(before.clan_rank), actor_id == target) {
        return Err(Error::forbidden("You cannot remove that member."));
    }
    let after = repo::set_member_status(&mut *tx, target, MemberStatus::Removed, now).await?;
    repo::record_audit(
        &mut *tx,
        Some(actor_id),
        Some(target),
        AuditEvent::MemberRemoved,
        json!({ "rank": before.clan_rank.as_str() }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(after)
}

/// Register someone who is already in the Discord server (bootstrap/import).
/// The rank is derived by the caller from the target's actual Discord roles,
/// so this can only record what Discord already says.
pub async fn import_existing(
    pool: &SqlitePool,
    actor_id: i64,
    target: i64,
    username: &str,
    rank: ClanRank,
    now: DateTime<Utc>,
) -> Result<Member> {
    let mut tx = pool.begin().await?;
    let m = repo::upsert_member_with_rank(&mut *tx, target, username, rank, status_for_rank(rank), now).await?;
    repo::record_audit(
        &mut *tx,
        Some(actor_id),
        Some(target),
        AuditEvent::MemberCreated,
        json!({ "rank": rank.as_str(), "source": "import" }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::applications::service::{accept, sample_new_application, submit};
    use crate::db::test_pool;

    async fn nix_member(pool: &SqlitePool, id: i64) {
        let a = submit(pool, &sample_new_application(id), Utc::now()).await.unwrap();
        accept(pool, a.id, 900, Utc::now()).await.unwrap();
    }

    #[tokio::test]
    async fn promote_nix_to_styx() {
        let pool = test_pool().await;
        nix_member(&pool, 1).await;
        let r = change_rank(&pool, 900, Tier::Kerberos, 1, ClanRank::Styx, true, Utc::now())
            .await
            .unwrap();
        assert_eq!(r.before.clan_rank, ClanRank::Nix);
        assert_eq!(r.after.clan_rank, ClanRank::Styx);
        assert_eq!(r.after.status, MemberStatus::Active);
        let audit = repo::audit_for_target(&pool, 1, 10).await.unwrap();
        assert!(audit.iter().any(|a| a.event_type == "MEMBER_PROMOTED"));
    }

    #[tokio::test]
    async fn charon_cannot_promote() {
        let pool = test_pool().await;
        nix_member(&pool, 1).await;
        let r = change_rank(&pool, 900, Tier::Charon, 1, ClanRank::Styx, true, Utc::now()).await;
        assert!(matches!(r, Err(Error::Forbidden(_))));
        assert_eq!(
            repo::get_member(&pool, 1).await.unwrap().unwrap().clan_rank,
            ClanRank::Nix
        );
    }

    #[tokio::test]
    async fn demote_back_to_trial() {
        let pool = test_pool().await;
        nix_member(&pool, 1).await;
        change_rank(&pool, 900, Tier::Kerberos, 1, ClanRank::Styx, true, Utc::now())
            .await
            .unwrap();
        let r = change_rank(&pool, 900, Tier::Kerberos, 1, ClanRank::Nix, false, Utc::now())
            .await
            .unwrap();
        assert_eq!(r.after.status, MemberStatus::Trial);
    }

    #[tokio::test]
    async fn squad_and_remove() {
        let pool = test_pool().await;
        nix_member(&pool, 1).await;
        let m = set_squad(&pool, 900, 1, Some("Alpha"), Utc::now()).await.unwrap();
        assert_eq!(m.squad.as_deref(), Some("Alpha"));
        let m = set_squad(&pool, 900, 1, None, Utc::now()).await.unwrap();
        assert_eq!(m.squad, None);
        let removed = remove(&pool, 900, Tier::Kerberos, 1, Utc::now()).await.unwrap();
        assert_eq!(removed.status, MemberStatus::Removed);
        // Record is preserved but no longer current.
        assert!(repo::get_member(&pool, 1).await.unwrap().is_some());
        assert!(repo::list_current_members(&pool).await.unwrap().is_empty());
        assert!(set_squad(&pool, 900, 1, Some("x"), Utc::now()).await.is_err());
    }

    #[tokio::test]
    async fn cannot_remove_equal_or_higher_or_self() {
        let pool = test_pool().await;
        import_existing(&pool, 1, 10, "mod", ClanRank::Kerberos, Utc::now())
            .await
            .unwrap();
        import_existing(&pool, 1, 11, "mod2", ClanRank::Kerberos, Utc::now())
            .await
            .unwrap();
        assert!(remove(&pool, 10, Tier::Kerberos, 11, Utc::now()).await.is_err());
        assert!(remove(&pool, 10, Tier::Kerberos, 10, Utc::now()).await.is_err());
        assert!(remove(&pool, 1, Tier::Warden, 11, Utc::now()).await.is_ok());
    }

    #[tokio::test]
    async fn update_stats_and_profile() {
        let pool = test_pool().await;
        nix_member(&pool, 1).await;
        let m = update_stats(
            &pool,
            900,
            1,
            &StatsUpdate {
                roblox_username: "NewName",
                player_level: 400,
                current_rank: "Elite",
                elo: 2000,
                win_rate: 60.0,
            },
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(m.roblox_username.as_deref(), Some("NewName"));
        assert_eq!(m.elo, Some(2000));
        let m = update_profile(&pool, 900, 1, Some("Knife"), Some("Entry"), Some("solid"), Utc::now())
            .await
            .unwrap();
        assert_eq!(m.notes.as_deref(), Some("solid"));
    }
}
