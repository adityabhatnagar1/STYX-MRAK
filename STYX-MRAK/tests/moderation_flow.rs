//! Moderation: audit trail, hierarchy rules and automod escalation. The
//! Discord REST calls (kick/ban/timeout/delete) need a live guild and are not
//! exercised here.

use chrono::Utc;
use serde_json::json;
use std::time::{Duration, Instant};
use styx_mrak::config::AutomodConfig;
use styx_mrak::db::{
    self,
    repositories::{self as repo, AuditEvent},
};
use styx_mrak::discord::permissions::{allowed, can_act_on, Permission, Tier};
use styx_mrak::moderation::service::{parse_duration, Automod, Violation};

fn automod() -> Automod {
    Automod::new(AutomodConfig {
        enabled: true,
        banned_words: vec!["badword".into()],
        block_invites: true,
        max_mentions: 5,
        flood_messages: 6,
        flood_window_seconds: 8,
        repeat_threshold: 3,
        repeat_window_seconds: 30,
        strikes_before_timeout: 3,
        strike_window_seconds: 600,
        timeout_minutes: 10,
    })
}

#[tokio::test]
async fn every_moderation_action_is_audited() {
    let pool = db::connect_memory().await.unwrap();
    for (ev, name) in [
        (AuditEvent::Warn, "WARN"),
        (AuditEvent::Timeout, "TIMEOUT"),
        (AuditEvent::Kick, "KICK"),
        (AuditEvent::Ban, "BAN"),
        (AuditEvent::Purge, "PURGE"),
    ] {
        repo::record_audit(&pool, Some(1), Some(2), ev, json!({ "reason": "test" }), Utc::now())
            .await
            .unwrap();
        assert_eq!(ev.as_str(), name);
    }
    let rows = repo::audit_for_target(&pool, 2, 20).await.unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(repo::count_warnings(&pool, 2).await.unwrap(), 1);
    assert!(rows.iter().all(|r| r.actor_discord_id == Some(1)));
}

#[test]
fn only_staff_may_moderate_and_only_downwards() {
    for t in [Tier::Guest, Tier::Nix, Tier::Styx, Tier::Charon] {
        assert!(!allowed(t, Permission::Staff, true), "{t:?}");
    }
    assert!(can_act_on(Tier::Kerberos, Tier::Charon, false));
    assert!(!can_act_on(Tier::Kerberos, Tier::Warden, false));
    assert!(!can_act_on(Tier::Kerberos, Tier::Kerberos, false));
    assert!(!can_act_on(Tier::Warden, Tier::Warden, true));
}

#[test]
fn timeout_durations() {
    assert_eq!(parse_duration("10m").unwrap().num_minutes(), 10);
    assert!(parse_duration("0m").is_err());
    assert!(parse_duration("30d").is_err());
}

#[test]
fn automod_deletes_first_then_escalates_only_on_repeats() {
    let mut a = automod();
    let t = Instant::now();
    // One invite link: flagged, but one strike never times anyone out.
    assert_eq!(a.check(1, "discord.gg/spam", 0, t), Some(Violation::Invite));
    assert!(!a.record_strike(1, t));
    // Normal chat is never flagged.
    assert_eq!(a.check(1, "good game everyone", 0, t + Duration::from_secs(60)), None);
    // Third strike inside the window escalates.
    assert_eq!(
        a.check(1, "discord.gg/spam2", 0, t + Duration::from_secs(61)),
        Some(Violation::Invite)
    );
    assert!(!a.record_strike(1, t + Duration::from_secs(61)));
    assert_eq!(
        a.check(1, "discord.gg/spam3", 0, t + Duration::from_secs(62)),
        Some(Violation::Invite)
    );
    assert!(a.record_strike(1, t + Duration::from_secs(62)));
}
