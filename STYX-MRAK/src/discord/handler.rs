//! Gateway event handler: interactions, startup, and the (optional) automod.

use crate::db::repositories::{self as repo, AuditEvent};
use crate::discord::actions::{self, Actor};
use crate::discord::permissions::Tier;
use crate::discord::{commands, embeds, interactions};
use crate::state::AppState;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::json;
use serenity::all::{Context, EditMember, EventHandler, Interaction, Message, Ready, RoleId, Timestamp, User};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub struct Handler {
    pub state: Arc<AppState>,
    ready_once: AtomicBool,
}

impl Handler {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            ready_once: AtomicBool::new(false),
        }
    }

    fn actor(&self, user: &User, roles: &[RoleId]) -> Actor {
        Actor {
            id: user.id,
            tier: self.state.cfg.roles.tier_of(roles),
        }
    }
}

/// Buttons/modals an applicant may use from a DM (no guild context).
fn allowed_outside_guild(custom_id: &str) -> bool {
    custom_id.starts_with("app:reply:") || custom_id.starts_with("app:replymodal:")
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!(user = %ready.user.name, "connected to Discord");
        if self.ready_once.swap(true, Ordering::SeqCst) {
            return; // reconnects fire Ready again; startup work runs once
        }
        let st = &self.state;
        let guild = match st.cfg.guild_id.to_partial_guild(st.http()).await {
            Ok(g) => g.name,
            Err(e) => {
                tracing::error!(error = %e, "cannot read the configured guild; is GUILD_ID right and the bot installed?");
                "STYX Rivals".into()
            }
        };
        let mut commands_ok = true;
        if let Err(e) = commands::register(st).await {
            tracing::error!(error = %e, "failed to register slash commands");
            commands_ok = false;
        }
        if let Err(e) = actions::ensure_apply_panel(st).await {
            tracing::error!(error = %e, "failed to post the apply panel");
        }
        if let Err(e) = actions::refresh_roster(st).await {
            tracing::error!(error = %e, "failed to update roster");
        }
        if let Err(e) = repo::purge_stale_pending_actions(&st.db, Utc::now()).await {
            tracing::warn!(error = %e, "could not purge stale confirmations");
        }
        tracing::info!(
            "ϟ МРАК ONLINE | guild: {guild} | database: connected | commands: {} | application system: ready | roster system: ready | feed system: {}",
            if commands_ok { "registered" } else { "FAILED" },
            if st.cfg.feed.enabled { "ready" } else { "disabled" }
        );
        actions::send_log(st, embeds::startup(&guild)).await;
    }

    async fn interaction_create(&self, _ctx: Context, interaction: Interaction) {
        let st = &self.state;
        match interaction {
            Interaction::Command(cmd) => {
                if cmd.guild_id != Some(st.cfg.guild_id) {
                    return;
                }
                let roles = cmd.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
                let actor = self.actor(&cmd.user, &roles);
                if let Err(e) = commands::dispatch(st, &cmd, &actor).await {
                    interactions::report(st, &cmd, e).await;
                }
            }
            Interaction::Component(comp) => {
                if comp.guild_id.is_none() {
                    if !allowed_outside_guild(&comp.data.custom_id) {
                        return;
                    }
                } else if comp.guild_id != Some(st.cfg.guild_id) {
                    return;
                }
                let roles = comp.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
                let actor = self.actor(&comp.user, &roles);
                if let Err(e) = interactions::component(st, &comp, &actor).await {
                    interactions::report(st, &comp, e).await;
                }
            }
            Interaction::Modal(m) => {
                if m.guild_id.is_none() {
                    if !allowed_outside_guild(&m.data.custom_id) {
                        return;
                    }
                } else if m.guild_id != Some(st.cfg.guild_id) {
                    return;
                }
                let roles = m.member.as_ref().map(|mm| mm.roles.clone()).unwrap_or_default();
                let actor = self.actor(&m.user, &roles);
                if let Err(e) = interactions::modal(st, &m, &actor).await {
                    interactions::report(st, &m, e).await;
                }
            }
            _ => {}
        }
    }

    async fn message(&self, _ctx: Context, msg: Message) {
        let st = &self.state;
        if msg.author.bot || msg.guild_id != Some(st.cfg.guild_id) {
            return;
        }
        let tier = msg
            .member
            .as_ref()
            .map(|m| st.cfg.roles.tier_of(&m.roles))
            .unwrap_or(Tier::Guest);
        if tier >= Tier::Kerberos {
            return; // staff are never auto-moderated
        }
        let mentions = msg.mentions.len() + msg.mention_roles.len();
        let verdict = {
            let Ok(mut a) = st.automod.lock() else { return };
            let now = Instant::now();
            a.check(msg.author.id.get(), &msg.content, mentions, now).map(|v| {
                let escalate = a.record_strike(msg.author.id.get(), now);
                (v, escalate, a.timeout_minutes())
            })
        };
        let Some((violation, escalate, minutes)) = verdict else {
            return;
        };
        let user = msg.author.id;
        let reason = format!("automod: {}", violation.label());

        if let Err(e) = msg.channel_id.delete_message(st.http(), msg.id).await {
            tracing::warn!(error = %e, "automod could not delete a message");
        }
        let _ = repo::record_audit(
            &st.db,
            None,
            Some(user.get() as i64),
            AuditEvent::AutomodAction,
            json!({ "violation": violation.label(), "escalated": escalate }),
            Utc::now(),
        )
        .await;
        actions::send_log(st, embeds::moderation_log("DELETE", user.get(), None, &reason, None)).await;

        if escalate {
            let until = Utc::now() + Duration::minutes(minutes as i64);
            if let Ok(stamp) = Timestamp::from_unix_timestamp(until.timestamp()) {
                let edit = EditMember::new()
                    .disable_communication_until_datetime(stamp)
                    .audit_log_reason("STYX bot: repeated automod violations");
                match st.cfg.guild_id.edit_member(st.http(), user, edit).await {
                    Ok(_) => {
                        let _ = repo::record_audit(
                            &st.db,
                            None,
                            Some(user.get() as i64),
                            AuditEvent::Timeout,
                            json!({ "duration": format!("{minutes}m"), "reason": "repeated automod violations" }),
                            Utc::now(),
                        )
                        .await;
                        actions::send_log(
                            st,
                            embeds::moderation_log(
                                "TIMEOUT",
                                user.get(),
                                Some(&format!("{minutes}m")),
                                "repeated automod violations",
                                None,
                            ),
                        )
                        .await;
                    }
                    Err(e) => tracing::warn!(error = %e, "automod timeout failed"),
                }
            }
        }
    }
}
