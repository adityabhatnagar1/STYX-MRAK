//! Application business logic. No Discord calls here: every state change is a
//! single database transaction, so a failure can never leave a half-accepted
//! applicant. Discord side effects (roles, DMs, roster) run afterwards.

use super::model::{Application, ApplicationStatus, NewApplication};
use crate::db::repositories as repo;
use crate::db::repositories::AuditEvent;
use crate::error::{Error, Result};
use crate::members::model::Member;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;

pub struct Accepted {
    pub application: Application,
    pub member: Member,
}

/// Store a validated application. Fails if the applicant is already a current
/// member or already has an open application (also enforced by a unique index).
pub async fn submit(pool: &SqlitePool, new: &NewApplication, now: DateTime<Utc>) -> Result<Application> {
    let mut tx = pool.begin().await?;
    if let Some(m) = repo::get_member(&mut *tx, new.discord_user_id).await? {
        if m.status.is_current() {
            return Err(Error::conflict("You are already part of STYX."));
        }
    }
    let app = repo::insert_application(&mut *tx, new, now).await?;
    repo::record_audit(
        &mut *tx,
        Some(new.discord_user_id),
        Some(new.discord_user_id),
        AuditEvent::ApplicationSubmitted,
        json!({ "application_id": app.id }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(app)
}

async fn load(pool: &SqlitePool, id: i64) -> Result<Application> {
    repo::get_application(pool, id)
        .await?
        .ok_or_else(|| Error::not_found(format!("Application #{id} does not exist.")))
}

/// Claim an application for review (`pending` -> `under_review`).
pub async fn mark_under_review(pool: &SqlitePool, id: i64, reviewer: i64, now: DateTime<Utc>) -> Result<Application> {
    let mut tx = pool.begin().await?;
    let current = repo::get_application(&mut *tx, id)
        .await?
        .ok_or_else(|| Error::not_found(format!("Application #{id} does not exist.")))?;
    current.status.ensure_transition(ApplicationStatus::UnderReview)?;
    let app = repo::transition_application(&mut *tx, id, ApplicationStatus::UnderReview, reviewer, None, now)
        .await?
        .ok_or_else(|| Error::conflict("Someone else already moved this application."))?;
    repo::record_audit(
        &mut *tx,
        Some(reviewer),
        Some(app.discord_user_id),
        AuditEvent::ApplicationReviewed,
        json!({ "application_id": id }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(app)
}

/// Accept: update application, create/reset member as Nix/trial, audit. One transaction.
pub async fn accept(pool: &SqlitePool, id: i64, reviewer: i64, now: DateTime<Utc>) -> Result<Accepted> {
    let mut tx = pool.begin().await?;
    let current = repo::get_application(&mut *tx, id)
        .await?
        .ok_or_else(|| Error::not_found(format!("Application #{id} does not exist.")))?;
    current.status.ensure_transition(ApplicationStatus::Accepted)?;

    let existing = repo::get_member(&mut *tx, current.discord_user_id).await?;
    if existing.as_ref().is_some_and(|m| m.status.is_current()) {
        return Err(Error::conflict(
            "That user is already a current member. Reject or withdraw the application instead.",
        ));
    }

    let app = repo::transition_application(&mut *tx, id, ApplicationStatus::Accepted, reviewer, None, now)
        .await?
        .ok_or_else(|| Error::conflict("Someone else already decided this application."))?;
    let member = repo::upsert_member_from_application(&mut *tx, &app, now).await?;
    repo::record_audit(
        &mut *tx,
        Some(reviewer),
        Some(app.discord_user_id),
        AuditEvent::ApplicationAccepted,
        json!({ "application_id": id }),
        now,
    )
    .await?;
    repo::record_audit(
        &mut *tx,
        Some(reviewer),
        Some(app.discord_user_id),
        if existing.is_some() {
            AuditEvent::MemberUpdated
        } else {
            AuditEvent::MemberCreated
        },
        json!({ "rank": "nix", "status": "trial", "application_id": id }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Accepted {
        application: app,
        member,
    })
}

/// Reject. The application row is kept forever; the reason is stored as a
/// private review note and is never sent to the applicant.
pub async fn reject(
    pool: &SqlitePool,
    id: i64,
    reviewer: i64,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Application> {
    let mut tx = pool.begin().await?;
    let current = repo::get_application(&mut *tx, id)
        .await?
        .ok_or_else(|| Error::not_found(format!("Application #{id} does not exist.")))?;
    current.status.ensure_transition(ApplicationStatus::Rejected)?;
    let app = repo::transition_application(&mut *tx, id, ApplicationStatus::Rejected, reviewer, reason, now)
        .await?
        .ok_or_else(|| Error::conflict("Someone else already decided this application."))?;
    repo::record_audit(
        &mut *tx,
        Some(reviewer),
        Some(app.discord_user_id),
        AuditEvent::ApplicationRejected,
        json!({ "application_id": id, "reason": reason }),
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(app)
}

pub async fn withdraw(pool: &SqlitePool, id: i64, user: i64, now: DateTime<Utc>) -> Result<Application> {
    let mut tx = pool.begin().await?;
    let current = repo::get_application(&mut *tx, id)
        .await?
        .ok_or_else(|| Error::not_found(format!("Application #{id} does not exist.")))?;
    if current.discord_user_id != user {
        return Err(Error::forbidden("That is not your application."));
    }
    current.status.ensure_transition(ApplicationStatus::Withdrawn)?;
    let app = repo::transition_application(&mut *tx, id, ApplicationStatus::Withdrawn, user, None, now)
        .await?
        .ok_or_else(|| Error::conflict("This application is no longer open."))?;
    tx.commit().await?;
    Ok(app)
}

/// Record that staff asked the applicant for more information.
pub async fn request_info(
    pool: &SqlitePool,
    id: i64,
    reviewer: i64,
    text: &str,
    now: DateTime<Utc>,
) -> Result<Application> {
    let mut tx = pool.begin().await?;
    let current = repo::get_application(&mut *tx, id)
        .await?
        .ok_or_else(|| Error::not_found(format!("Application #{id} does not exist.")))?;
    if !current.status.is_open() {
        return Err(Error::conflict("This application is already closed."));
    }
    repo::set_info_request(&mut *tx, id, text).await?;
    repo::record_audit(
        &mut *tx,
        Some(reviewer),
        Some(current.discord_user_id),
        AuditEvent::ApplicationInfoRequested,
        json!({ "application_id": id }),
        now,
    )
    .await?;
    tx.commit().await?;
    load(pool, id).await
}

/// Record the applicant's answer to an info request.
pub async fn record_info_reply(
    pool: &SqlitePool,
    id: i64,
    user: i64,
    reply: &str,
    now: DateTime<Utc>,
) -> Result<Application> {
    let app = load(pool, id).await?;
    if app.discord_user_id != user {
        return Err(Error::forbidden("That is not your application."));
    }
    if !app.status.is_open() {
        return Err(Error::conflict("This application is already closed."));
    }
    repo::record_audit(
        pool,
        Some(user),
        Some(user),
        AuditEvent::ApplicationInfoProvided,
        json!({ "application_id": id, "reply": reply }),
        now,
    )
    .await?;
    Ok(app)
}

/// Load for display. Public wrapper used by handlers.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Application> {
    load(pool, id).await
}

#[cfg(test)]
pub fn sample_new_application(user: i64) -> NewApplication {
    use super::model::{StepOne, StepTwo};
    NewApplication {
        discord_user_id: user,
        discord_username: format!("user{user}"),
        one: StepOne {
            roblox_username: "Cool_Guy1".into(),
            player_level: 312,
            current_rank: "Diamond".into(),
            elo: 1840,
            win_rate: 71.0,
        },
        two: StepTwo {
            preferred_loadout: "Shotgun / Uzi / Knife / Flash".into(),
            playstyle: "Aggressive entry".into(),
            timezone: "IST".into(),
            motivation: "I want to play with a real squad.".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;
    use crate::members::model::{ClanRank, MemberStatus};

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[tokio::test]
    async fn submit_then_duplicate_is_rejected() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        assert_eq!(a.status, ApplicationStatus::Pending);
        let dup = submit(&pool, &sample_new_application(1), now()).await;
        assert!(matches!(dup, Err(Error::Conflict(_))));
    }

    #[tokio::test]
    async fn accept_creates_trial_nix_member_and_audits() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        let out = accept(&pool, a.id, 99, now()).await.unwrap();
        assert_eq!(out.application.status, ApplicationStatus::Accepted);
        assert_eq!(out.application.reviewed_by, Some(99));
        assert!(out.application.reviewed_at.is_some());
        assert_eq!(out.member.clan_rank, ClanRank::Nix);
        assert_eq!(out.member.status, MemberStatus::Trial);
        assert_eq!(out.member.roblox_username.as_deref(), Some("Cool_Guy1"));
        let audit = repo::audit_for_target(&pool, 1, 10).await.unwrap();
        let kinds: Vec<_> = audit.iter().map(|r| r.event_type.as_str()).collect();
        assert!(kinds.contains(&"APPLICATION_ACCEPTED"));
        assert!(kinds.contains(&"MEMBER_CREATED"));
        assert!(kinds.contains(&"APPLICATION_SUBMITTED"));
    }

    #[tokio::test]
    async fn double_decision_is_a_conflict() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        accept(&pool, a.id, 99, now()).await.unwrap();
        assert!(matches!(accept(&pool, a.id, 99, now()).await, Err(Error::Conflict(_))));
        assert!(matches!(
            reject(&pool, a.id, 99, None, now()).await,
            Err(Error::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn reject_keeps_history_and_reapply_is_allowed() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        let r = reject(&pool, a.id, 99, Some("needs more time"), now()).await.unwrap();
        assert_eq!(r.status, ApplicationStatus::Rejected);
        assert_eq!(r.review_notes.as_deref(), Some("needs more time"));
        // Record is still there.
        assert!(repo::get_application(&pool, a.id).await.unwrap().is_some());
        // The applicant may apply again later.
        let again = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        assert_ne!(again.id, a.id);
    }

    #[tokio::test]
    async fn current_members_cannot_apply() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        accept(&pool, a.id, 99, now()).await.unwrap();
        let r = submit(&pool, &sample_new_application(1), now()).await;
        assert!(matches!(r, Err(Error::Conflict(_))));
    }

    #[tokio::test]
    async fn review_then_decide() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        let r = mark_under_review(&pool, a.id, 5, now()).await.unwrap();
        assert_eq!(r.status, ApplicationStatus::UnderReview);
        assert!(mark_under_review(&pool, a.id, 5, now()).await.is_err());
        assert!(accept(&pool, a.id, 5, now()).await.is_ok());
    }

    #[tokio::test]
    async fn missing_application_is_not_found() {
        let pool = test_pool().await;
        assert!(matches!(accept(&pool, 404, 1, now()).await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn info_request_only_while_open() {
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        let updated = request_info(&pool, a.id, 5, "What is your peak ELO?", now())
            .await
            .unwrap();
        assert_eq!(updated.info_request_text.as_deref(), Some("What is your peak ELO?"));
        reject(&pool, a.id, 5, None, now()).await.unwrap();
        assert!(request_info(&pool, a.id, 5, "again?", now()).await.is_err());
    }

    #[tokio::test]
    async fn failed_accept_rolls_back() {
        // A returning, still-current member makes accept fail; the application must stay open.
        let pool = test_pool().await;
        let a = submit(&pool, &sample_new_application(1), now()).await.unwrap();
        repo::upsert_member_with_rank(&pool, 1, "x", ClanRank::Styx, MemberStatus::Active, now())
            .await
            .unwrap();
        assert!(accept(&pool, a.id, 5, now()).await.is_err());
        let after = repo::get_application(&pool, a.id).await.unwrap().unwrap();
        assert_eq!(after.status, ApplicationStatus::Pending);
    }
}
