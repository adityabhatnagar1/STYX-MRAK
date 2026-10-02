//! Discord-side effects and the confirmable-action executor.
//!
//! Pattern for every state change: (1) authorise, (2) database transaction,
//! (3) best-effort Discord effects (roles, DMs, roster, log). A failure in (3)
//! is logged and reported to the staff member; it never rolls back (2) and
//! never panics.

use crate::applications::model::Application;
use crate::applications::service as app_svc;
use crate::db::repositories::{self as repo, AuditEvent};
use crate::discord::embeds;
use crate::discord::permissions::{can_act_on, require, Permission, Tier};
use crate::discord::responder::Responder;
use crate::error::{Error, Result};
use crate::feeds::service::Announcer;
use crate::feeds::source::FeedItem;
use crate::members::model::{ClanRank, MemberStatus};
use crate::members::{roster, service as mem_svc};
use crate::moderation::service as mod_svc;
use crate::state::AppState;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serenity::all::{
    ChannelId, CreateActionRow, CreateEmbed, CreateInteractionResponseMessage, CreateMessage, EditMember, EditMessage,
    GetMessages, MessageId, Timestamp, UserId,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Actor {
    pub id: UserId,
    pub tier: Tier,
}

impl Actor {
    pub fn id_i64(&self) -> i64 {
        uid(self.id)
    }
}

pub fn uid(u: UserId) -> i64 {
    u.get() as i64
}

pub fn user_id(v: i64) -> UserId {
    UserId::new(v as u64)
}

pub fn is_not_found(e: &serenity::Error) -> bool {
    matches!(e, serenity::Error::Http(h)
        if matches!(h, serenity::http::HttpError::UnsuccessfulRequest(r) if r.status_code.as_u16() == 404))
}

// ---------------------------------------------------------------------------
// Confirmable actions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingAction {
    AcceptApplication {
        id: i64,
    },
    RejectApplication {
        id: i64,
        reason: Option<String>,
    },
    ChangeRank {
        user_id: i64,
        from: ClanRank,
        to: ClanRank,
        promote: bool,
    },
    RemoveMember {
        user_id: i64,
    },
    Warn {
        user_id: i64,
        reason: String,
    },
    Kick {
        user_id: i64,
        reason: String,
    },
    Ban {
        user_id: i64,
        reason: String,
    },
    Purge {
        channel_id: i64,
        amount: u8,
    },
}

impl PendingAction {
    pub fn required(&self) -> Permission {
        match self {
            Self::AcceptApplication { .. } | Self::RejectApplication { .. } => Permission::Decide,
            _ => Permission::Staff,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::AcceptApplication { id } => {
                format!("Accept application #{id}?\n\nThe applicant becomes Nix (trial).")
            }
            Self::RejectApplication { id, .. } => format!("Reject application #{id}?"),
            Self::ChangeRank {
                user_id,
                from,
                to,
                promote,
            } => format!(
                "{} <@{user_id}>?\n\n{} → {}",
                if *promote { "Promote" } else { "Demote" },
                from.label(),
                to.label()
            ),
            Self::RemoveMember { user_id } => {
                format!("Remove <@{user_id}> from the STYX registry?\n\nTheir clan roles will be stripped.")
            }
            Self::Warn { user_id, reason } => format!("Warn <@{user_id}>?\n\nReason: {}", embeds::clip(reason, 300)),
            Self::Kick { user_id, reason } => format!("Kick <@{user_id}>?\n\nReason: {}", embeds::clip(reason, 300)),
            Self::Ban { user_id, reason } => format!("Ban <@{user_id}>?\n\nReason: {}", embeds::clip(reason, 300)),
            Self::Purge { channel_id, amount } => format!("Delete the last {amount} messages in <#{channel_id}>?"),
        }
    }

    pub fn confirm_label(&self) -> &'static str {
        match self {
            Self::RejectApplication { .. } => "CONFIRM REJECTION",
            Self::AcceptApplication { .. } => "CONFIRM ACCEPT",
            Self::Kick { .. } => "CONFIRM KICK",
            Self::Ban { .. } => "CONFIRM BAN",
            Self::Purge { .. } => "CONFIRM PURGE",
            Self::RemoveMember { .. } => "CONFIRM REMOVAL",
            Self::Warn { .. } => "CONFIRM WARNING",
            Self::ChangeRank { .. } => "CONFIRM",
        }
    }

    pub fn danger(&self) -> bool {
        !matches!(
            self,
            Self::AcceptApplication { .. } | Self::ChangeRank { promote: true, .. }
        )
    }
}

/// Store the action and show an ephemeral confirm/cancel prompt.
pub async fn ask_confirm(st: &Arc<AppState>, resp: &dyn Responder, actor: &Actor, action: PendingAction) -> Result<()> {
    require(actor.tier, action.required(), st.cfg.charon_can_decide)?;
    let payload = serde_json::to_string(&action)?;
    let id = repo::create_pending_action(&st.db, actor.id_i64(), &payload, Utc::now(), Duration::minutes(10)).await?;
    resp.reply(
        st.http(),
        CreateInteractionResponseMessage::new()
            .embed(embeds::confirm_prompt(&action.describe()))
            .components(embeds::confirm_buttons(id, action.confirm_label(), action.danger()))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Run a confirmed action. Authorisation is checked again here, against the
/// clicker's *current* roles.
pub async fn execute(st: &Arc<AppState>, actor: &Actor, action: PendingAction) -> Result<String> {
    require(actor.tier, action.required(), st.cfg.charon_can_decide)?;
    let now = Utc::now();
    match action {
        PendingAction::AcceptApplication { id } => accept_application(st, actor, id).await,
        PendingAction::RejectApplication { id, reason } => reject_application(st, actor, id, reason).await,
        PendingAction::ChangeRank {
            user_id, to, promote, ..
        } => change_rank(st, actor, user_id, to, promote).await,
        PendingAction::RemoveMember { user_id } => remove_member(st, actor, user_id).await,
        PendingAction::Warn { user_id, reason } => {
            guard_target(st, actor, user_id).await?;
            repo::record_audit(
                &st.db,
                Some(actor.id_i64()),
                Some(user_id),
                AuditEvent::Warn,
                json!({ "reason": reason }),
                now,
            )
            .await?;
            let total = repo::count_warnings(&st.db, user_id).await?;
            send_log(
                st,
                embeds::moderation_log("WARN", user_id as u64, None, &reason, Some(actor.id.get())),
            )
            .await;
            Ok(format!("Warning recorded for <@{user_id}>. Total warnings: {total}."))
        }
        PendingAction::Kick { user_id, reason } => {
            guard_target(st, actor, user_id).await?;
            st.cfg
                .guild_id
                .kick_with_reason(st.http(), user_id_of(user_id), &audit_reason(&reason))
                .await?;
            repo::record_audit(
                &st.db,
                Some(actor.id_i64()),
                Some(user_id),
                AuditEvent::Kick,
                json!({ "reason": reason }),
                now,
            )
            .await?;
            mark_removed_if_member(st, actor, user_id).await;
            send_log(
                st,
                embeds::moderation_log("KICK", user_id as u64, None, &reason, Some(actor.id.get())),
            )
            .await;
            Ok(format!("Kicked <@{user_id}>."))
        }
        PendingAction::Ban { user_id, reason } => {
            guard_target(st, actor, user_id).await?;
            st.cfg
                .guild_id
                .ban_with_reason(st.http(), user_id_of(user_id), 0, &audit_reason(&reason))
                .await?;
            repo::record_audit(
                &st.db,
                Some(actor.id_i64()),
                Some(user_id),
                AuditEvent::Ban,
                json!({ "reason": reason }),
                now,
            )
            .await?;
            mark_removed_if_member(st, actor, user_id).await;
            send_log(
                st,
                embeds::moderation_log("BAN", user_id as u64, None, &reason, Some(actor.id.get())),
            )
            .await;
            Ok(format!("Banned <@{user_id}>."))
        }
        PendingAction::Purge { channel_id, amount } => purge(st, actor, channel_id, amount).await,
    }
}

fn user_id_of(v: i64) -> UserId {
    user_id(v)
}

fn audit_reason(reason: &str) -> String {
    embeds::clip(&format!("STYX bot: {reason}"), 500)
}

// ---------------------------------------------------------------------------
// Application decisions
// ---------------------------------------------------------------------------

async fn accept_application(st: &Arc<AppState>, actor: &Actor, id: i64) -> Result<String> {
    let out = app_svc::accept(&st.db, id, actor.id_i64(), Utc::now()).await?;
    let applicant = user_id(out.application.discord_user_id);
    tracing::info!(application_id = id, "application accepted");
    let mut notes: Vec<String> = Vec::new();

    if let Err(e) = sync_rank_roles(st, applicant, Some(ClanRank::Nix)).await {
        tracing::error!(error = %e, application_id = id, "failed to assign Nix role");
        notes.push(format!(
            "Could not assign the Nix role ({}). Assign it manually.",
            e.user_message()
        ));
    }
    if !dm_user(st, applicant, CreateMessage::new().embed(embeds::dm_accepted())).await {
        tracing::warn!(application_id = id, "failed to DM applicant");
        notes.push("The applicant has DMs closed. Let them know directly.".into());
    }
    update_review_card(st, &out.application).await;
    send_log(
        st,
        embeds::log_line(
            "APPLICATION ACCEPTED",
            format!(
                "Application #{id}\nApplicant: <@{applicant}>\nAccepted by: <@{}>",
                actor.id
            ),
        ),
    )
    .await;
    if let Err(e) = refresh_roster(st).await {
        tracing::error!(error = %e, "failed to update roster");
        notes.push("Roster refresh failed. Run `/roster refresh`.".into());
    }
    Ok(summarize(
        format!("Application #{id} accepted. <@{applicant}> is now Nix."),
        notes,
    ))
}

async fn reject_application(st: &Arc<AppState>, actor: &Actor, id: i64, reason: Option<String>) -> Result<String> {
    let app = app_svc::reject(&st.db, id, actor.id_i64(), reason.as_deref(), Utc::now()).await?;
    let applicant = user_id(app.discord_user_id);
    tracing::info!(application_id = id, "application rejected");
    let mut notes = Vec::new();
    // The reason is a private staff note and is deliberately not sent.
    if !dm_user(st, applicant, CreateMessage::new().embed(embeds::dm_rejected())).await {
        tracing::warn!(application_id = id, "failed to DM applicant");
        notes.push("The applicant has DMs closed.".into());
    }
    update_review_card(st, &app).await;
    send_log(
        st,
        embeds::log_line(
            "APPLICATION REJECTED",
            format!(
                "Application #{id}\nApplicant: <@{applicant}>\nRejected by: <@{}>",
                actor.id
            ),
        ),
    )
    .await;
    Ok(summarize(
        format!("Application #{id} rejected. The record is kept."),
        notes,
    ))
}

fn summarize(head: String, notes: Vec<String>) -> String {
    if notes.is_empty() {
        head
    } else {
        format!("{head}\n\n⚠ {}", notes.join("\n⚠ "))
    }
}

pub async fn post_review_card(st: &Arc<AppState>, app: &Application) -> Result<()> {
    let staff = st.cfg.channels.staff;
    let msg = staff
        .send_message(
            st.http(),
            CreateMessage::new()
                .embed(embeds::application_review(app))
                .components(embeds::review_buttons(app.id, true)),
        )
        .await?;
    repo::set_review_message(&st.db, app.id, staff.get() as i64, msg.id.get() as i64).await?;
    Ok(())
}

pub async fn update_review_card(st: &Arc<AppState>, app: &Application) {
    let (Some(c), Some(m)) = (app.review_channel_id, app.review_message_id) else {
        return;
    };
    let edit = EditMessage::new()
        .embeds(vec![embeds::application_review(app)])
        .components(embeds::review_buttons(app.id, app.status.is_open()));
    if let Err(e) = ChannelId::new(c as u64)
        .edit_message(st.http(), MessageId::new(m as u64), edit)
        .await
    {
        tracing::warn!(error = %e, application_id = app.id, "failed to update review card");
    }
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

async fn change_rank(st: &Arc<AppState>, actor: &Actor, target: i64, to: ClanRank, promote: bool) -> Result<String> {
    let change = mem_svc::change_rank(&st.db, actor.id_i64(), actor.tier, target, to, promote, Utc::now()).await?;
    let user = user_id(target);
    tracing::info!(from = %change.before.clan_rank, to = %to, "member rank changed");
    let mut notes = Vec::new();
    if let Err(e) = sync_rank_roles(st, user, Some(to)).await {
        tracing::error!(error = %e, "failed to sync rank roles");
        notes.push(format!(
            "Role sync failed ({}). Fix the roles manually.",
            e.user_message()
        ));
    }
    if promote
        && !dm_user(
            st,
            user,
            CreateMessage::new().embed(embeds::dm_promoted(change.before.clan_rank, to)),
        )
        .await
    {
        notes.push("The member has DMs closed.".into());
    }
    send_log(
        st,
        embeds::log_line(
            if promote { "PROMOTION" } else { "DEMOTION" },
            format!(
                "Member: <@{target}>\n{} → {}\nBy: <@{}>",
                change.before.clan_rank.label(),
                to.label(),
                actor.id
            ),
        ),
    )
    .await;
    if let Err(e) = refresh_roster(st).await {
        tracing::error!(error = %e, "failed to update roster");
        notes.push("Roster refresh failed. Run `/roster refresh`.".into());
    }
    Ok(summarize(
        format!("<@{target}>: {} → {}.", change.before.clan_rank.label(), to.label()),
        notes,
    ))
}

async fn remove_member(st: &Arc<AppState>, actor: &Actor, target: i64) -> Result<String> {
    mem_svc::remove(&st.db, actor.id_i64(), actor.tier, target, Utc::now()).await?;
    let mut notes = Vec::new();
    if let Err(e) = sync_rank_roles(st, user_id(target), None).await {
        notes.push(format!(
            "Could not strip roles ({}). Remove them manually.",
            e.user_message()
        ));
    }
    send_log(
        st,
        embeds::log_line("MEMBER REMOVED", format!("Member: <@{target}>\nBy: <@{}>", actor.id)),
    )
    .await;
    if let Err(e) = refresh_roster(st).await {
        tracing::error!(error = %e, "failed to update roster");
        notes.push("Roster refresh failed. Run `/roster refresh`.".into());
    }
    Ok(summarize(format!("<@{target}> removed from the registry."), notes))
}

async fn mark_removed_if_member(st: &Arc<AppState>, actor: &Actor, target: i64) {
    match repo::get_member(&st.db, target).await {
        Ok(Some(m)) if m.status.is_current() => {
            let now = Utc::now();
            if repo::set_member_status(&st.db, target, MemberStatus::Removed, now)
                .await
                .is_ok()
            {
                let _ = repo::record_audit(
                    &st.db,
                    Some(actor.id_i64()),
                    Some(target),
                    AuditEvent::MemberRemoved,
                    json!({ "via": "kick/ban" }),
                    now,
                )
                .await;
                if let Err(e) = refresh_roster(st).await {
                    tracing::error!(error = %e, "failed to update roster");
                }
            }
        }
        _ => {}
    }
}

/// Make the member's rank roles match `target` (`None` strips them all).
/// The Warden role is never touched by the bot.
pub async fn sync_rank_roles(st: &Arc<AppState>, user: UserId, target: Option<ClanRank>) -> Result<()> {
    let http = st.http();
    let guild = st.cfg.guild_id;
    let member = guild.member(http, user).await?;
    for rank in ClanRank::ALL {
        if rank == ClanRank::Warden {
            continue;
        }
        let rid = st.cfg.roles.role_for(rank);
        let want = Some(rank) == target;
        let has = member.roles.contains(&rid);
        if want && !has {
            http.add_member_role(guild, user, rid, Some("STYX bot: rank sync"))
                .await?;
        } else if !want && has {
            http.remove_member_role(guild, user, rid, Some("STYX bot: rank sync"))
                .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Moderation helpers
// ---------------------------------------------------------------------------

/// The tier of a user as Discord sees them right now. Users who are not in
/// the server are `Guest`.
pub async fn tier_of_user(st: &Arc<AppState>, user: UserId) -> Result<Tier> {
    match st.cfg.guild_id.member(st.http(), user).await {
        Ok(m) => Ok(st.cfg.roles.tier_of(&m.roles)),
        Err(e) if is_not_found(&e) => Ok(Tier::Guest),
        Err(e) => Err(e.into()),
    }
}

/// Hierarchy check shared by every disciplinary action.
pub async fn guard_target(st: &Arc<AppState>, actor: &Actor, target: i64) -> Result<()> {
    let tier = tier_of_user(st, user_id(target)).await?;
    if can_act_on(actor.tier, tier, actor.id_i64() == target) {
        Ok(())
    } else {
        Err(Error::forbidden("You cannot take action against that user."))
    }
}

pub async fn timeout_member(
    st: &Arc<AppState>,
    actor: &Actor,
    target: i64,
    duration: Duration,
    reason: &str,
) -> Result<String> {
    require(actor.tier, Permission::Staff, st.cfg.charon_can_decide)?;
    guard_target(st, actor, target).await?;
    let until = Utc::now() + duration;
    let stamp = Timestamp::from_unix_timestamp(until.timestamp())
        .map_err(|_| Error::validation("That duration is out of range."))?;
    st.cfg
        .guild_id
        .edit_member(
            st.http(),
            user_id(target),
            EditMember::new()
                .disable_communication_until_datetime(stamp)
                .audit_log_reason(&audit_reason(reason)),
        )
        .await?;
    let d = mod_svc::format_duration(duration);
    repo::record_audit(
        &st.db,
        Some(actor.id_i64()),
        Some(target),
        AuditEvent::Timeout,
        json!({ "duration": d, "reason": reason }),
        Utc::now(),
    )
    .await?;
    send_log(
        st,
        embeds::moderation_log("TIMEOUT", target as u64, Some(&d), reason, Some(actor.id.get())),
    )
    .await;
    Ok(format!("<@{target}> timed out for {d}."))
}

async fn purge(st: &Arc<AppState>, actor: &Actor, channel_id: i64, amount: u8) -> Result<String> {
    let channel = ChannelId::new(channel_id as u64);
    let msgs = channel.messages(st.http(), GetMessages::new().limit(amount)).await?;
    // Discord refuses to bulk-delete messages older than 14 days.
    let cutoff = Utc::now().timestamp() - 13 * 24 * 3600;
    let ids: Vec<MessageId> = msgs
        .iter()
        .filter(|m| m.timestamp.unix_timestamp() > cutoff)
        .map(|m| m.id)
        .collect();
    if ids.is_empty() {
        return Ok("Nothing to purge. Messages older than 14 days cannot be bulk-deleted.".into());
    }
    channel.delete_messages(st.http(), &ids).await?;
    repo::record_audit(
        &st.db,
        Some(actor.id_i64()),
        None,
        AuditEvent::Purge,
        json!({ "channel_id": channel_id, "count": ids.len() }),
        Utc::now(),
    )
    .await?;
    send_log(
        st,
        embeds::log_line(
            "MODERATION",
            format!(
                "Action: PURGE\nChannel: <#{channel_id}>\nMessages: {}\nModerator: <@{}>",
                ids.len(),
                actor.id
            ),
        ),
    )
    .await;
    Ok(format!("Deleted {} messages.", ids.len()))
}

// ---------------------------------------------------------------------------
// Messaging primitives
// ---------------------------------------------------------------------------

/// Post to the log channel. Never fails the caller.
pub async fn send_log(st: &Arc<AppState>, embed: CreateEmbed) {
    if let Err(e) = st
        .cfg
        .channels
        .logs
        .send_message(st.http(), CreateMessage::new().embed(embed))
        .await
    {
        tracing::warn!(error = %e, "failed to write to the log channel");
    }
}

/// DM a user. Returns `false` (and never errors) if DMs are closed.
pub async fn dm_user(st: &Arc<AppState>, user: UserId, msg: CreateMessage) -> bool {
    let channel = match user.create_dm_channel(st.http()).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "could not open DM channel");
            return false;
        }
    };
    match channel.id.send_message(st.http(), msg).await {
        Ok(_) => true,
        Err(e) => {
            tracing::debug!(error = %e, "DM delivery failed");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical messages (roster, apply panel)
// ---------------------------------------------------------------------------

pub fn parse_ref(s: &str) -> Option<(u64, u64)> {
    let (c, m) = s.split_once(':')?;
    Some((c.parse().ok()?, m.parse().ok()?))
}

/// Edit the stored canonical message, or create it if it is missing. Falls
/// back to creating a new message only when Discord says the old one is gone
/// (404); any other error is returned so a transient failure can never create
/// a duplicate.
async fn upsert_canonical_message(
    st: &Arc<AppState>,
    key: &str,
    channel: ChannelId,
    embed: CreateEmbed,
    components: Vec<CreateActionRow>,
) -> Result<MessageId> {
    let http = st.http();
    let stored = repo::get_config(&st.db, key).await?;
    let existing = stored
        .as_deref()
        .and_then(parse_ref)
        .filter(|(c, _)| *c == channel.get());
    if let Some((_, mid)) = existing {
        let edit = EditMessage::new()
            .embeds(vec![embed.clone()])
            .components(components.clone());
        match channel.edit_message(http, MessageId::new(mid), edit).await {
            Ok(m) => return Ok(m.id),
            Err(e) if is_not_found(&e) => tracing::info!(key, "canonical message is gone; recreating"),
            Err(e) => return Err(e.into()),
        }
    }
    let m = channel
        .send_message(http, CreateMessage::new().embed(embed).components(components))
        .await?;
    repo::set_config(&st.db, key, &format!("{}:{}", channel.get(), m.id.get()), Utc::now()).await?;
    Ok(m.id)
}

/// Regenerate the single canonical roster message from the registry.
pub async fn refresh_roster(st: &Arc<AppState>) -> Result<()> {
    let _guard = st.roster_lock.lock().await;
    let members = repo::list_current_members(&st.db).await?;
    let view = roster::build_roster(&members);
    let now = Utc::now();
    upsert_canonical_message(
        st,
        "roster.message",
        st.cfg.channels.roster,
        embeds::roster(&view, now),
        vec![],
    )
    .await?;
    repo::set_config(&st.db, "roster.updated_at", &now.to_rfc3339(), now).await?;
    repo::record_audit(
        &st.db,
        None,
        None,
        AuditEvent::RosterUpdated,
        json!({ "members": members.len() }),
        now,
    )
    .await?;
    if let Ok(mut g) = st.last_roster_update.lock() {
        *g = Some(now);
    }
    tracing::info!(members = members.len(), "roster updated");
    Ok(())
}

/// Post or refresh the permanent apply panel in the apply channel.
pub async fn ensure_apply_panel(st: &Arc<AppState>) -> Result<()> {
    upsert_canonical_message(
        st,
        "panel.apply",
        st.cfg.channels.apply,
        embeds::apply_panel(),
        embeds::apply_panel_buttons(),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Feed announcer
// ---------------------------------------------------------------------------

pub struct DiscordAnnouncer {
    pub state: Arc<AppState>,
}

#[async_trait]
impl Announcer for DiscordAnnouncer {
    async fn announce(&self, item: &FeedItem, source_label: &str) -> std::result::Result<i64, String> {
        let msg = CreateMessage::new().embed(embeds::intel_drop(item, source_label));
        self.state
            .cfg
            .channels
            .announcements
            .send_message(self.state.http(), msg)
            .await
            .map(|m| m.id.get() as i64)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_actions_round_trip_through_json() {
        let actions = vec![
            PendingAction::AcceptApplication { id: 3 },
            PendingAction::RejectApplication {
                id: 4,
                reason: Some("x".into()),
            },
            PendingAction::ChangeRank {
                user_id: 9,
                from: ClanRank::Nix,
                to: ClanRank::Styx,
                promote: true,
            },
            PendingAction::Purge {
                channel_id: 5,
                amount: 20,
            },
        ];
        for a in actions {
            let s = serde_json::to_string(&a).unwrap();
            assert_eq!(serde_json::from_str::<PendingAction>(&s).unwrap(), a);
        }
    }

    #[test]
    fn required_permissions() {
        assert_eq!(
            PendingAction::AcceptApplication { id: 1 }.required(),
            Permission::Decide
        );
        assert_eq!(
            PendingAction::RejectApplication { id: 1, reason: None }.required(),
            Permission::Decide
        );
        assert_eq!(
            PendingAction::Ban {
                user_id: 1,
                reason: "x".into()
            }
            .required(),
            Permission::Staff
        );
        assert_eq!(
            PendingAction::Purge {
                channel_id: 1,
                amount: 5
            }
            .required(),
            Permission::Staff
        );
    }

    #[test]
    fn ref_parsing() {
        assert_eq!(parse_ref("12:34"), Some((12, 34)));
        assert_eq!(parse_ref("12"), None);
        assert_eq!(parse_ref("a:b"), None);
    }

    #[test]
    fn rejection_prompt_matches_spec() {
        let a = PendingAction::RejectApplication { id: 7, reason: None };
        assert_eq!(a.confirm_label(), "CONFIRM REJECTION");
        assert!(a.danger());
    }
}
