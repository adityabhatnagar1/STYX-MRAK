//! All SQL lives here. Functions are generic over `SqliteExecutor` so they run
//! against either the pool or an open transaction (`&mut *tx`).
//!
//! Runtime-checked queries are used on purpose: no build-time database needed.

use crate::applications::model::{Application, ApplicationStatus, NewApplication};
use crate::error::{Error, Result};
use crate::members::model::{ClanRank, Member, MemberStatus};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::SqliteExecutor;

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditEvent {
    ApplicationSubmitted,
    ApplicationReviewed,
    ApplicationAccepted,
    ApplicationRejected,
    ApplicationInfoRequested,
    ApplicationInfoProvided,
    MemberCreated,
    MemberUpdated,
    MemberPromoted,
    MemberDemoted,
    MemberRemoved,
    SquadChanged,
    Warn,
    Timeout,
    Kick,
    Ban,
    Purge,
    AutomodAction,
    RosterUpdated,
    ConfigChanged,
}

impl AuditEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApplicationSubmitted => "APPLICATION_SUBMITTED",
            Self::ApplicationReviewed => "APPLICATION_REVIEWED",
            Self::ApplicationAccepted => "APPLICATION_ACCEPTED",
            Self::ApplicationRejected => "APPLICATION_REJECTED",
            Self::ApplicationInfoRequested => "APPLICATION_INFO_REQUESTED",
            Self::ApplicationInfoProvided => "APPLICATION_INFO_PROVIDED",
            Self::MemberCreated => "MEMBER_CREATED",
            Self::MemberUpdated => "MEMBER_UPDATED",
            Self::MemberPromoted => "MEMBER_PROMOTED",
            Self::MemberDemoted => "MEMBER_DEMOTED",
            Self::MemberRemoved => "MEMBER_REMOVED",
            Self::SquadChanged => "SQUAD_CHANGED",
            Self::Warn => "WARN",
            Self::Timeout => "TIMEOUT",
            Self::Kick => "KICK",
            Self::Ban => "BAN",
            Self::Purge => "PURGE",
            Self::AutomodAction => "AUTOMOD_ACTION",
            Self::RosterUpdated => "ROSTER_UPDATED",
            Self::ConfigChanged => "CONFIG_CHANGED",
        }
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AuditRow {
    pub event_id: i64,
    pub actor_discord_id: Option<i64>,
    pub target_discord_id: Option<i64>,
    pub event_type: String,
    pub metadata: String,
    pub timestamp: DateTime<Utc>,
}

pub async fn record_audit<'e>(
    ex: impl SqliteExecutor<'e>,
    actor: Option<i64>,
    target: Option<i64>,
    event: AuditEvent,
    metadata: Value,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_events (actor_discord_id, target_discord_id, event_type, metadata, timestamp)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(actor)
    .bind(target)
    .bind(event.as_str())
    .bind(metadata.to_string())
    .bind(now)
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn audit_for_target<'e>(ex: impl SqliteExecutor<'e>, target: i64, limit: i64) -> Result<Vec<AuditRow>> {
    Ok(sqlx::query_as::<_, AuditRow>(
        "SELECT * FROM audit_events WHERE target_discord_id = ? ORDER BY event_id DESC LIMIT ?",
    )
    .bind(target)
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

pub async fn count_audit_since<'e>(ex: impl SqliteExecutor<'e>, since: DateTime<Utc>) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE timestamp >= ?")
            .bind(since)
            .fetch_one(ex)
            .await?,
    )
}

pub async fn count_warnings<'e>(ex: impl SqliteExecutor<'e>, target: i64) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE target_discord_id = ? AND event_type = 'WARN'")
            .bind(target)
            .fetch_one(ex)
            .await?,
    )
}

// ---------------------------------------------------------------------------
// Applications
// ---------------------------------------------------------------------------

pub async fn insert_application<'e>(
    ex: impl SqliteExecutor<'e>,
    new: &NewApplication,
    now: DateTime<Utc>,
) -> Result<Application> {
    let res = sqlx::query_as::<_, Application>(
        "INSERT INTO applications
            (discord_user_id, discord_username, roblox_username, player_level, current_rank, elo,
             win_rate, preferred_loadout, playstyle, timezone, motivation, status, submitted_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)
         RETURNING *",
    )
    .bind(new.discord_user_id)
    .bind(&new.discord_username)
    .bind(&new.one.roblox_username)
    .bind(new.one.player_level)
    .bind(&new.one.current_rank)
    .bind(new.one.elo)
    .bind(new.one.win_rate)
    .bind(&new.two.preferred_loadout)
    .bind(&new.two.playstyle)
    .bind(&new.two.timezone)
    .bind(&new.two.motivation)
    .bind(now)
    .fetch_one(ex)
    .await;
    match res {
        Ok(a) => Ok(a),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(Error::conflict(
            "You already have an open application. Staff will get to it.",
        )),
        Err(e) => Err(e.into()),
    }
}

pub async fn get_application<'e>(ex: impl SqliteExecutor<'e>, id: i64) -> Result<Option<Application>> {
    Ok(
        sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE id = ?")
            .bind(id)
            .fetch_optional(ex)
            .await?,
    )
}

pub async fn open_application_for_user<'e>(ex: impl SqliteExecutor<'e>, user: i64) -> Result<Option<Application>> {
    Ok(sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE discord_user_id = ? AND status IN ('pending','under_review')
         ORDER BY id DESC LIMIT 1",
    )
    .bind(user)
    .fetch_optional(ex)
    .await?)
}

pub async fn list_open_applications<'e>(ex: impl SqliteExecutor<'e>, limit: i64) -> Result<Vec<Application>> {
    Ok(sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE status IN ('pending','under_review') ORDER BY id ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

pub async fn list_applications_by_status<'e>(
    ex: impl SqliteExecutor<'e>,
    status: ApplicationStatus,
    limit: i64,
) -> Result<Vec<Application>> {
    Ok(
        sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE status = ? ORDER BY id DESC LIMIT ?")
            .bind(status.as_str())
            .bind(limit)
            .fetch_all(ex)
            .await?,
    )
}

pub async fn set_review_message<'e>(ex: impl SqliteExecutor<'e>, id: i64, channel: i64, message: i64) -> Result<()> {
    sqlx::query("UPDATE applications SET review_channel_id = ?, review_message_id = ? WHERE id = ?")
        .bind(channel)
        .bind(message)
        .bind(id)
        .execute(ex)
        .await?;
    Ok(())
}

/// Atomically move an *open* application to `next`. Returns `None` if the
/// application is missing or no longer in a state that allows the move; the
/// status guard is in SQL so two reviewers cannot both win a race.
pub async fn transition_application<'e>(
    ex: impl SqliteExecutor<'e>,
    id: i64,
    next: ApplicationStatus,
    reviewer: i64,
    notes: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Option<Application>> {
    let from_clause = if next == ApplicationStatus::UnderReview {
        "status = 'pending'"
    } else {
        "status IN ('pending','under_review')"
    };
    let sql = format!(
        "UPDATE applications
            SET status = ?, reviewed_at = ?, reviewed_by = ?, review_notes = COALESCE(?, review_notes)
          WHERE id = ? AND {from_clause}
          RETURNING *"
    );
    Ok(sqlx::query_as::<_, Application>(&sql)
        .bind(next.as_str())
        .bind(now)
        .bind(reviewer)
        .bind(notes)
        .bind(id)
        .fetch_optional(ex)
        .await?)
}

pub async fn set_info_request<'e>(ex: impl SqliteExecutor<'e>, id: i64, text: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE applications SET info_request_text = ? WHERE id = ? AND status IN ('pending','under_review')",
    )
    .bind(text)
    .bind(id)
    .execute(ex)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn save_draft<'e>(ex: impl SqliteExecutor<'e>, user: i64, payload: &str, now: DateTime<Utc>) -> Result<()> {
    sqlx::query(
        "INSERT INTO application_drafts (discord_user_id, payload, created_at) VALUES (?, ?, ?)
         ON CONFLICT(discord_user_id) DO UPDATE SET payload = excluded.payload, created_at = excluded.created_at",
    )
    .bind(user)
    .bind(payload)
    .bind(now)
    .execute(ex)
    .await?;
    Ok(())
}

/// Drafts older than 30 minutes are treated as expired.
pub async fn load_draft<'e>(ex: impl SqliteExecutor<'e>, user: i64, now: DateTime<Utc>) -> Result<Option<String>> {
    let cutoff = now - Duration::minutes(30);
    Ok(
        sqlx::query_scalar("SELECT payload FROM application_drafts WHERE discord_user_id = ? AND created_at >= ?")
            .bind(user)
            .bind(cutoff)
            .fetch_optional(ex)
            .await?,
    )
}

pub async fn delete_draft<'e>(ex: impl SqliteExecutor<'e>, user: i64) -> Result<()> {
    sqlx::query("DELETE FROM application_drafts WHERE discord_user_id = ?")
        .bind(user)
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn application_counts<'e>(ex: impl SqliteExecutor<'e>) -> Result<Vec<(String, i64)>> {
    Ok(
        sqlx::query_as("SELECT status, COUNT(*) FROM applications GROUP BY status ORDER BY status")
            .fetch_all(ex)
            .await?,
    )
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

pub async fn get_member<'e>(ex: impl SqliteExecutor<'e>, discord_id: i64) -> Result<Option<Member>> {
    Ok(
        sqlx::query_as::<_, Member>("SELECT * FROM members WHERE discord_user_id = ?")
            .bind(discord_id)
            .fetch_optional(ex)
            .await?,
    )
}

/// Members currently part of the clan (trial, active, inactive).
pub async fn list_current_members<'e>(ex: impl SqliteExecutor<'e>) -> Result<Vec<Member>> {
    Ok(sqlx::query_as::<_, Member>(
        "SELECT * FROM members WHERE status IN ('trial','active','inactive') ORDER BY joined_at ASC, id ASC",
    )
    .fetch_all(ex)
    .await?)
}

pub async fn member_counts<'e>(ex: impl SqliteExecutor<'e>) -> Result<Vec<(String, i64)>> {
    Ok(
        sqlx::query_as("SELECT status, COUNT(*) FROM members GROUP BY status ORDER BY status")
            .fetch_all(ex)
            .await?,
    )
}

/// Create the member record for an accepted application, or reset a returning
/// member back to trial.
pub async fn upsert_member_from_application<'e>(
    ex: impl SqliteExecutor<'e>,
    app: &Application,
    now: DateTime<Utc>,
) -> Result<Member> {
    Ok(sqlx::query_as::<_, Member>(
        "INSERT INTO members
            (discord_user_id, discord_username, roblox_username, status, clan_rank, squad,
             player_level, current_rank, elo, win_rate, preferred_loadout, playstyle,
             joined_at, last_updated_at)
         VALUES (?, ?, ?, 'trial', 'nix', NULL, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(discord_user_id) DO UPDATE SET
            discord_username  = excluded.discord_username,
            roblox_username   = excluded.roblox_username,
            status            = 'trial',
            clan_rank         = 'nix',
            squad             = NULL,
            player_level      = excluded.player_level,
            current_rank      = excluded.current_rank,
            elo               = excluded.elo,
            win_rate          = excluded.win_rate,
            preferred_loadout = excluded.preferred_loadout,
            playstyle         = excluded.playstyle,
            joined_at         = CASE WHEN members.status IN ('retired','removed')
                                     THEN excluded.joined_at ELSE members.joined_at END,
            last_updated_at   = excluded.last_updated_at
         RETURNING *",
    )
    .bind(app.discord_user_id)
    .bind(&app.discord_username)
    .bind(&app.roblox_username)
    .bind(app.player_level)
    .bind(&app.current_rank)
    .bind(app.elo)
    .bind(app.win_rate)
    .bind(&app.preferred_loadout)
    .bind(&app.playstyle)
    .bind(now)
    .bind(now)
    .fetch_one(ex)
    .await?)
}

/// Register an existing Discord member (bootstrap / import) at `rank`.
pub async fn upsert_member_with_rank<'e>(
    ex: impl SqliteExecutor<'e>,
    discord_id: i64,
    username: &str,
    rank: ClanRank,
    status: MemberStatus,
    now: DateTime<Utc>,
) -> Result<Member> {
    Ok(sqlx::query_as::<_, Member>(
        "INSERT INTO members (discord_user_id, discord_username, status, clan_rank, joined_at, last_updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(discord_user_id) DO UPDATE SET
            discord_username = excluded.discord_username,
            status           = excluded.status,
            clan_rank        = excluded.clan_rank,
            last_updated_at  = excluded.last_updated_at
         RETURNING *",
    )
    .bind(discord_id)
    .bind(username)
    .bind(status.as_str())
    .bind(rank.as_str())
    .bind(now)
    .bind(now)
    .fetch_one(ex)
    .await?)
}

pub async fn set_member_rank<'e>(
    ex: impl SqliteExecutor<'e>,
    discord_id: i64,
    rank: ClanRank,
    status: MemberStatus,
    now: DateTime<Utc>,
) -> Result<Member> {
    sqlx::query_as::<_, Member>(
        "UPDATE members SET clan_rank = ?, status = ?, last_updated_at = ? WHERE discord_user_id = ? RETURNING *",
    )
    .bind(rank.as_str())
    .bind(status.as_str())
    .bind(now)
    .bind(discord_id)
    .fetch_optional(ex)
    .await?
    .ok_or_else(|| Error::not_found("That user is not in the registry."))
}

pub async fn set_member_squad<'e>(
    ex: impl SqliteExecutor<'e>,
    discord_id: i64,
    squad: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Member> {
    sqlx::query_as::<_, Member>(
        "UPDATE members SET squad = ?, last_updated_at = ? WHERE discord_user_id = ? RETURNING *",
    )
    .bind(squad)
    .bind(now)
    .bind(discord_id)
    .fetch_optional(ex)
    .await?
    .ok_or_else(|| Error::not_found("That user is not in the registry."))
}

pub async fn set_member_status<'e>(
    ex: impl SqliteExecutor<'e>,
    discord_id: i64,
    status: MemberStatus,
    now: DateTime<Utc>,
) -> Result<Member> {
    sqlx::query_as::<_, Member>(
        "UPDATE members SET status = ?, last_updated_at = ? WHERE discord_user_id = ? RETURNING *",
    )
    .bind(status.as_str())
    .bind(now)
    .bind(discord_id)
    .fetch_optional(ex)
    .await?
    .ok_or_else(|| Error::not_found("That user is not in the registry."))
}

pub async fn update_member_username<'e>(ex: impl SqliteExecutor<'e>, discord_id: i64, username: &str) -> Result<()> {
    sqlx::query("UPDATE members SET discord_username = ? WHERE discord_user_id = ? AND discord_username <> ?")
        .bind(username)
        .bind(discord_id)
        .bind(username)
        .execute(ex)
        .await?;
    Ok(())
}

pub struct StatsUpdate<'a> {
    pub roblox_username: &'a str,
    pub player_level: i64,
    pub current_rank: &'a str,
    pub elo: i64,
    pub win_rate: f64,
}

pub async fn update_member_stats<'e>(
    ex: impl SqliteExecutor<'e>,
    discord_id: i64,
    u: &StatsUpdate<'_>,
    now: DateTime<Utc>,
) -> Result<Member> {
    sqlx::query_as::<_, Member>(
        "UPDATE members SET roblox_username = ?, player_level = ?, current_rank = ?, elo = ?,
                win_rate = ?, last_updated_at = ?
         WHERE discord_user_id = ? RETURNING *",
    )
    .bind(u.roblox_username)
    .bind(u.player_level)
    .bind(u.current_rank)
    .bind(u.elo)
    .bind(u.win_rate)
    .bind(now)
    .bind(discord_id)
    .fetch_optional(ex)
    .await?
    .ok_or_else(|| Error::not_found("That user is not in the registry."))
}

pub async fn update_member_profile<'e>(
    ex: impl SqliteExecutor<'e>,
    discord_id: i64,
    loadout: Option<&str>,
    playstyle: Option<&str>,
    notes: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Member> {
    sqlx::query_as::<_, Member>(
        "UPDATE members SET preferred_loadout = ?, playstyle = ?, notes = ?, last_updated_at = ?
         WHERE discord_user_id = ? RETURNING *",
    )
    .bind(loadout)
    .bind(playstyle)
    .bind(notes)
    .bind(now)
    .bind(discord_id)
    .fetch_optional(ex)
    .await?
    .ok_or_else(|| Error::not_found("That user is not in the registry."))
}

// ---------------------------------------------------------------------------
// Config key/value (canonical message IDs, feed cursor, ...)
// ---------------------------------------------------------------------------

pub async fn get_config<'e>(ex: impl SqliteExecutor<'e>, key: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM config WHERE key = ?")
        .bind(key)
        .fetch_optional(ex)
        .await?)
}

pub async fn set_config<'e>(ex: impl SqliteExecutor<'e>, key: &str, value: &str, now: DateTime<Utc>) -> Result<()> {
    sqlx::query(
        "INSERT INTO config (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now)
    .execute(ex)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Feed items
// ---------------------------------------------------------------------------

/// Try to take ownership of announcing `source_id`. Returns `true` if this
/// caller should post it. A row that is already announced / filtered /
/// baseline / claimed is never handed out again; a `failed` row is retried
/// until `max_attempts`.
pub async fn claim_feed_item<'e, E>(
    ex: E,
    source: &str,
    source_id: &str,
    text: &str,
    now: DateTime<Utc>,
    max_attempts: i64,
) -> Result<bool>
where
    E: SqliteExecutor<'e> + Copy,
{
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO feed_items (source, source_id, status, attempts, text, first_seen_at)
         VALUES (?, ?, 'claimed', 1, ?, ?)",
    )
    .bind(source)
    .bind(source_id)
    .bind(text)
    .bind(now)
    .execute(ex)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(true);
    }
    let retried = sqlx::query(
        "UPDATE feed_items SET status = 'claimed', attempts = attempts + 1
         WHERE source = ? AND source_id = ? AND status = 'failed' AND attempts < ?",
    )
    .bind(source)
    .bind(source_id)
    .bind(max_attempts)
    .execute(ex)
    .await?;
    Ok(retried.rows_affected() == 1)
}

/// Record an item that is deliberately not announced (`filtered` / `baseline`).
pub async fn record_skipped_feed_item<'e>(
    ex: impl SqliteExecutor<'e>,
    source: &str,
    source_id: &str,
    status: &str,
    text: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    debug_assert!(status == "filtered" || status == "baseline");
    sqlx::query(
        "INSERT OR IGNORE INTO feed_items (source, source_id, status, attempts, text, first_seen_at)
         VALUES (?, ?, ?, 0, ?, ?)",
    )
    .bind(source)
    .bind(source_id)
    .bind(status)
    .bind(text)
    .bind(now)
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn mark_feed_announced<'e>(
    ex: impl SqliteExecutor<'e>,
    source: &str,
    source_id: &str,
    message_id: i64,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "UPDATE feed_items SET status = 'announced', discord_message_id = ?, announced_at = ?
         WHERE source = ? AND source_id = ?",
    )
    .bind(message_id)
    .bind(now)
    .bind(source)
    .bind(source_id)
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn mark_feed_failed<'e>(ex: impl SqliteExecutor<'e>, source: &str, source_id: &str) -> Result<()> {
    sqlx::query("UPDATE feed_items SET status = 'failed' WHERE source = ? AND source_id = ?")
        .bind(source)
        .bind(source_id)
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn feed_item_status<'e>(
    ex: impl SqliteExecutor<'e>,
    source: &str,
    source_id: &str,
) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT status FROM feed_items WHERE source = ? AND source_id = ?")
            .bind(source)
            .bind(source_id)
            .fetch_optional(ex)
            .await?,
    )
}

pub async fn count_feed_items<'e>(ex: impl SqliteExecutor<'e>, status: &str) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM feed_items WHERE status = ?")
        .bind(status)
        .fetch_one(ex)
        .await?)
}

// ---------------------------------------------------------------------------
// Pending (confirmable) actions
// ---------------------------------------------------------------------------

pub async fn create_pending_action<'e>(
    ex: impl SqliteExecutor<'e>,
    actor: i64,
    payload: &str,
    now: DateTime<Utc>,
    ttl: Duration,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO pending_actions (actor_discord_id, payload, created_at, expires_at)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(actor)
    .bind(payload)
    .bind(now)
    .bind(now + ttl)
    .fetch_one(ex)
    .await?)
}

/// Atomically consume a pending action. Only the actor who created it can
/// consume it, only once, and only before it expires.
pub async fn consume_pending_action<'e>(
    ex: impl SqliteExecutor<'e>,
    id: i64,
    actor: i64,
    now: DateTime<Utc>,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "UPDATE pending_actions SET consumed_at = ?
         WHERE id = ? AND actor_discord_id = ? AND consumed_at IS NULL AND expires_at > ?
         RETURNING payload",
    )
    .bind(now)
    .bind(id)
    .bind(actor)
    .bind(now)
    .fetch_optional(ex)
    .await?)
}

pub async fn purge_stale_pending_actions<'e>(ex: impl SqliteExecutor<'e>, now: DateTime<Utc>) -> Result<u64> {
    let cutoff = now - Duration::days(1);
    Ok(sqlx::query("DELETE FROM pending_actions WHERE expires_at < ?")
        .bind(cutoff)
        .execute(ex)
        .await?
        .rows_affected())
}
