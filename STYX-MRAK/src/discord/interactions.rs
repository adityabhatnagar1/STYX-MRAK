//! Routes buttons and modal submissions by `custom_id`. All persistent UI
//! (the apply panel, review cards, confirmations) is stateless: the custom ID
//! plus the database carry everything, so it keeps working after a restart.

use crate::applications::handlers as app_handlers;
use crate::applications::model::{
    parse_elo, parse_level, parse_win_rate, validate_free_text, validate_rank, validate_roblox_username,
};
use crate::db::repositories::{self as repo, StatsUpdate};
use crate::discord::actions::{self, Actor, PendingAction};
use crate::discord::embeds;
use crate::discord::permissions::{require, Permission};
use crate::discord::responder::{ephemeral, Responder};
use crate::error::{Error, Result};
use crate::members::service as mem_svc;
use crate::state::AppState;
use chrono::Utc;
use serenity::all::{
    ComponentInteraction, CreateInteractionResponse, CreateInteractionResponseMessage, EditInteractionResponse,
    ModalInteraction,
};
use std::sync::Arc;

fn id_part(parts: &[&str], idx: usize) -> Result<i64> {
    parts
        .get(idx)
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| Error::not_found("That control is no longer valid."))
}

pub async fn component(st: &Arc<AppState>, comp: &ComponentInteraction, actor: &Actor) -> Result<()> {
    let custom = comp.data.custom_id.clone();
    let parts: Vec<&str> = custom.split(':').collect();
    match parts.as_slice() {
        ["apply", "start"] => app_handlers::start(st, comp, actor).await,
        ["apply", "next"] => app_handlers::next_step(st, comp, actor).await,
        ["app", action @ ("review" | "accept" | "reject" | "info"), _] => {
            app_handlers::review_button(st, comp, actor, action, id_part(&parts, 2)?).await
        }
        ["app", action @ ("viewq" | "reply"), _] => {
            app_handlers::applicant_button(st, comp, action, id_part(&parts, 2)?).await
        }
        ["act", verdict @ ("ok" | "no"), _] => confirm(st, comp, actor, *verdict == "ok", id_part(&parts, 2)?).await,
        _ => Err(Error::not_found("That control is no longer valid.")),
    }
}

async fn confirm(
    st: &Arc<AppState>,
    comp: &ComponentInteraction,
    actor: &Actor,
    ok: bool,
    pending_id: i64,
) -> Result<()> {
    let payload = repo::consume_pending_action(&st.db, pending_id, actor.id_i64(), Utc::now()).await?;
    let Some(payload) = payload else {
        return Err(Error::conflict("This confirmation has expired or was already used."));
    };
    if !ok {
        comp.create_response(
            st.http(),
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .content("Cancelled.")
                    .embeds(vec![])
                    .components(vec![]),
            ),
        )
        .await?;
        return Ok(());
    }
    let action: PendingAction = serde_json::from_str(&payload)?;
    comp.defer(st.http()).await?;
    let text = match actions::execute(st, actor, action).await {
        Ok(t) => t,
        Err(e) => {
            if !matches!(
                e,
                Error::Validation(_) | Error::Forbidden(_) | Error::Conflict(_) | Error::NotFound(_)
            ) {
                tracing::error!(error = %e, "confirmed action failed");
            }
            format!("Failed: {}", e.user_message())
        }
    };
    comp.edit_response(
        st.http(),
        EditInteractionResponse::new()
            .content(text)
            .embeds(vec![])
            .components(vec![]),
    )
    .await?;
    Ok(())
}

pub async fn modal(st: &Arc<AppState>, m: &ModalInteraction, actor: &Actor) -> Result<()> {
    let custom = m.data.custom_id.clone();
    let parts: Vec<&str> = custom.split(':').collect();
    match parts.as_slice() {
        ["apply", "s1"] => app_handlers::step_one_submit(st, m, actor).await,
        ["apply", "s2"] => app_handlers::step_two_submit(st, m, actor).await,
        ["app", "rejmodal", _] => app_handlers::reject_modal_submit(st, m, actor, id_part(&parts, 2)?).await,
        ["app", "infomodal", _] => app_handlers::info_modal_submit(st, m, actor, id_part(&parts, 2)?).await,
        ["app", "replymodal", _] => app_handlers::reply_modal_submit(st, m, id_part(&parts, 2)?).await,
        ["mem", "upd", section @ ("stats" | "profile"), _] => {
            member_update(st, m, actor, section, id_part(&parts, 3)?).await
        }
        _ => Err(Error::not_found("That form is no longer valid.")),
    }
}

fn optional(field: &str, raw: String, max: usize) -> Result<Option<String>> {
    if raw.trim().is_empty() {
        Ok(None)
    } else {
        validate_free_text(field, &raw, max).map(Some)
    }
}

async fn member_update(
    st: &Arc<AppState>,
    m: &ModalInteraction,
    actor: &Actor,
    section: &str,
    target: i64,
) -> Result<()> {
    require(actor.tier, Permission::Staff, st.cfg.charon_can_decide)?;
    let v = |id: &str| app_handlers::modal_value(m, id);
    let now = Utc::now();
    let member = if section == "stats" {
        let roblox = validate_roblox_username(&v("roblox"))?;
        let level = parse_level(&v("level"))?;
        let rank = validate_rank(&v("rank"))?;
        let elo = parse_elo(&v("elo"))?;
        let win = parse_win_rate(&v("winrate"))?;
        mem_svc::update_stats(
            &st.db,
            actor.id_i64(),
            target,
            &StatsUpdate {
                roblox_username: &roblox,
                player_level: level,
                current_rank: &rank,
                elo,
                win_rate: win,
            },
            now,
        )
        .await?
    } else {
        let loadout = optional("Loadout", v("loadout"), 100)?;
        let playstyle = optional("Playstyle", v("playstyle"), 100)?;
        let notes = optional("Notes", v("notes"), 1000)?;
        mem_svc::update_profile(
            &st.db,
            actor.id_i64(),
            target,
            loadout.as_deref(),
            playstyle.as_deref(),
            notes.as_deref(),
            now,
        )
        .await?
    };
    m.create_response(
        st.http(),
        CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .embed(embeds::member_card(&member, true))
                .ephemeral(true),
        ),
    )
    .await?;
    Ok(())
}

/// Send an error to the user without leaking internals. Works whether or not
/// the interaction was already acknowledged.
pub async fn report(st: &Arc<AppState>, resp: &dyn Responder, err: Error) {
    match &err {
        Error::Validation(_) | Error::Forbidden(_) | Error::Conflict(_) | Error::NotFound(_) => {
            tracing::debug!(error = %err, "request rejected")
        }
        _ => tracing::error!(error = %err, "interaction failed"),
    }
    let text = err.user_message();
    if resp.reply(st.http(), ephemeral(text.clone())).await.is_err() {
        let _ = resp
            .edit(
                st.http(),
                EditInteractionResponse::new()
                    .content(text)
                    .embeds(vec![])
                    .components(vec![]),
            )
            .await;
    }
}
