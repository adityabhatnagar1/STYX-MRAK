//! End-to-end database path of the application lifecycle (everything except
//! the Discord REST calls, which need a live guild).

mod common;

use chrono::Utc;
use styx_mrak::applications::model::{
    parse_elo, parse_level, parse_win_rate, validate_roblox_username, validate_step_one, ApplicationStatus,
};
use styx_mrak::applications::service as apps;
use styx_mrak::db::{self, repositories as repo};
use styx_mrak::discord::permissions::Tier;
use styx_mrak::error::Error;
use styx_mrak::members::model::{ClanRank, MemberStatus};
use styx_mrak::members::{roster, service as members};

#[tokio::test]
async fn submit_accept_roster_promote() {
    let pool = db::connect_memory().await.unwrap();

    // Submit -> pending, one open application per user.
    let app = apps::submit(&pool, &common::application(1), Utc::now()).await.unwrap();
    assert_eq!(app.status, ApplicationStatus::Pending);
    assert!(matches!(
        apps::submit(&pool, &common::application(1), Utc::now()).await,
        Err(Error::Conflict(_))
    ));

    // Staff review then accept -> Nix/trial member, audited, attributed.
    apps::mark_under_review(&pool, app.id, 100, Utc::now()).await.unwrap();
    let out = apps::accept(&pool, app.id, 100, Utc::now()).await.unwrap();
    assert_eq!(out.application.reviewed_by, Some(100));
    assert_eq!(out.member.clan_rank, ClanRank::Nix);
    assert_eq!(out.member.status, MemberStatus::Trial);

    // Roster shows exactly one trial member.
    let view = roster::build_roster(&repo::list_current_members(&pool).await.unwrap());
    assert_eq!((view.active, view.trial), (0, 1));
    assert_eq!(view.sections.len(), 1);
    assert_eq!(view.sections[0].rank, ClanRank::Nix);

    // Promote Nix -> Styx (by Kerberos): DB, audit and roster all follow.
    let change = members::change_rank(&pool, 100, Tier::Kerberos, 1, ClanRank::Styx, true, Utc::now())
        .await
        .unwrap();
    assert_eq!(change.after.status, MemberStatus::Active);
    let view = roster::build_roster(&repo::list_current_members(&pool).await.unwrap());
    assert_eq!((view.active, view.trial), (1, 0));
    assert_eq!(view.sections[0].rank, ClanRank::Styx);

    let kinds: Vec<String> = repo::audit_for_target(&pool, 1, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.event_type)
        .collect();
    for k in [
        "APPLICATION_SUBMITTED",
        "APPLICATION_REVIEWED",
        "APPLICATION_ACCEPTED",
        "MEMBER_CREATED",
        "MEMBER_PROMOTED",
    ] {
        assert!(kinds.iter().any(|x| x == k), "missing audit event {k}: {kinds:?}");
    }
}

#[tokio::test]
async fn reject_path_keeps_the_record_and_audits() {
    let pool = db::connect_memory().await.unwrap();
    let app = apps::submit(&pool, &common::application(2), Utc::now()).await.unwrap();
    let rejected = apps::reject(&pool, app.id, 100, Some("trial not a fit"), Utc::now())
        .await
        .unwrap();
    assert_eq!(rejected.status, ApplicationStatus::Rejected);
    assert_eq!(rejected.reviewed_by, Some(100));
    assert!(rejected.reviewed_at.is_some());
    // No member was created and the application is retained.
    assert!(repo::get_member(&pool, 2).await.unwrap().is_none());
    assert!(repo::get_application(&pool, app.id).await.unwrap().is_some());
    let audit = repo::audit_for_target(&pool, 2, 10).await.unwrap();
    assert!(audit.iter().any(|a| a.event_type == "APPLICATION_REJECTED"));
    // A decided application cannot be decided again.
    assert!(apps::accept(&pool, app.id, 100, Utc::now()).await.is_err());
}

#[tokio::test]
async fn confirmations_are_single_use_owner_bound_and_expire() {
    let pool = db::connect_memory().await.unwrap();
    let now = Utc::now();
    let id = repo::create_pending_action(
        &pool,
        7,
        r#"{"kind":"accept_application","id":1}"#,
        now,
        chrono::Duration::minutes(10),
    )
    .await
    .unwrap();
    // Someone else cannot consume it.
    assert!(repo::consume_pending_action(&pool, id, 8, now).await.unwrap().is_none());
    // The owner can, once.
    assert!(repo::consume_pending_action(&pool, id, 7, now).await.unwrap().is_some());
    assert!(repo::consume_pending_action(&pool, id, 7, now).await.unwrap().is_none());
    // Expired confirmations are refused.
    let id2 = repo::create_pending_action(&pool, 7, "{}", now, chrono::Duration::minutes(10))
        .await
        .unwrap();
    let later = now + chrono::Duration::minutes(11);
    assert!(repo::consume_pending_action(&pool, id2, 7, later)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn concurrent_decisions_have_one_winner() {
    let pool = db::connect_memory().await.unwrap();
    let app = apps::submit(&pool, &common::application(3), Utc::now()).await.unwrap();
    let (a, b) = tokio::join!(
        apps::accept(&pool, app.id, 1, Utc::now()),
        apps::reject(&pool, app.id, 2, None, Utc::now())
    );
    assert!(a.is_ok() ^ b.is_ok(), "exactly one decision may succeed");
}

#[test]
fn spec_validation_examples() {
    // win rate: 75% / 0.75 / 101%
    assert_eq!(parse_win_rate("75%").unwrap(), 75.0);
    assert_eq!(parse_win_rate("0.75").unwrap(), 75.0);
    assert!(parse_win_rate("101%").is_err());
    // elo: 1800 / abc
    assert_eq!(parse_elo("1800").unwrap(), 1800);
    assert!(parse_elo("abc").is_err());
    // level: -1
    assert!(parse_level("-1").is_err());
    // empty and oversized usernames
    assert!(validate_roblox_username("").is_err());
    assert!(validate_roblox_username(&"a".repeat(64)).is_err());
    // whole step-1 validation
    assert!(validate_step_one("Cool_Guy1", "312", "Diamond", "1840", "71%").is_ok());
    assert!(validate_step_one("Cool_Guy1", "312", "Diamond", "abc", "71%").is_err());
}
