//! Discord glue for the application flow: modals, panel button, staff review
//! buttons and the applicant's info-request replies.
//!
//! To change the application questions edit `build_application_step_one()` and
//! `build_application_step_two()` (and the matching validation in
//! `applications/model.rs`).

use super::model::{
    validate_free_text, validate_step_one, validate_step_two, ApplicationStatus, NewApplication, StepOne,
};
use super::service;
use crate::db::repositories as repo;
use crate::discord::actions::{self, ask_confirm, user_id, Actor, PendingAction};
use crate::discord::embeds;
use crate::discord::permissions::{require, Permission};
use crate::discord::responder::{ephemeral, Responder};
use crate::error::{Error, Result};
use crate::state::AppState;
use chrono::Utc;
use serenity::all::{
    ActionRowComponent, ComponentInteraction, CreateActionRow, CreateAllowedMentions, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateModal, EditInteractionResponse,
    InputTextStyle, ModalInteraction,
};
use std::sync::Arc;

fn input(style: InputTextStyle, label: &str, id: &str, placeholder: &str, max: u16, required: bool) -> CreateActionRow {
    let mut i = CreateInputText::new(style, label, id)
        .max_length(max)
        .required(required);
    if !placeholder.is_empty() {
        i = i.placeholder(placeholder);
    }
    CreateActionRow::InputText(i)
}

/// Application modal, step 1 of 2. Discord modals hold at most 5 inputs.
pub fn build_application_step_one() -> CreateModal {
    CreateModal::new("apply:s1", "STYX Application · 1/2").components(vec![
        input(
            InputTextStyle::Short,
            "Roblox Username",
            "roblox",
            "Your exact Roblox username",
            20,
            true,
        ),
        input(
            InputTextStyle::Short,
            "Rivals Player Level",
            "level",
            "e.g. 312",
            6,
            true,
        ),
        input(
            InputTextStyle::Short,
            "Current Rank",
            "rank",
            "e.g. Diamond 2",
            32,
            true,
        ),
        input(InputTextStyle::Short, "Current ELO", "elo", "e.g. 1840", 6, true),
        input(InputTextStyle::Short, "Win Rate", "winrate", "e.g. 71%", 8, true),
    ])
}

/// Application modal, step 2 of 2.
pub fn build_application_step_two() -> CreateModal {
    CreateModal::new("apply:s2", "STYX Application · 2/2").components(vec![
        input(
            InputTextStyle::Short,
            "Preferred Loadout",
            "loadout",
            "e.g. Shotgun / Uzi / Knife / Flash",
            100,
            true,
        ),
        input(
            InputTextStyle::Short,
            "Playstyle",
            "playstyle",
            "e.g. Aggressive entry",
            100,
            true,
        ),
        input(
            InputTextStyle::Short,
            "Timezone",
            "timezone",
            "e.g. IST, EST, CET",
            32,
            true,
        ),
        input(
            InputTextStyle::Paragraph,
            "Why do you want to join STYX?",
            "motivation",
            "",
            1000,
            true,
        ),
    ])
}

fn reject_modal(id: i64) -> CreateModal {
    CreateModal::new(format!("app:rejmodal:{id}"), "Reject application").components(vec![input(
        InputTextStyle::Paragraph,
        "Reason (private, optional)",
        "reason",
        "Stored in the audit trail. Not sent to the applicant.",
        500,
        false,
    )])
}

fn info_modal(id: i64) -> CreateModal {
    CreateModal::new(format!("app:infomodal:{id}"), "Request more information").components(vec![input(
        InputTextStyle::Paragraph,
        "What do you need from the applicant?",
        "question",
        "",
        1000,
        true,
    )])
}

fn reply_modal(id: i64) -> CreateModal {
    CreateModal::new(format!("app:replymodal:{id}"), "Reply to STYX staff").components(vec![input(
        InputTextStyle::Paragraph,
        "Your reply",
        "reply",
        "",
        1000,
        true,
    )])
}

/// Read a text input out of a submitted modal.
pub fn modal_value(modal: &ModalInteraction, id: &str) -> String {
    for row in &modal.data.components {
        for c in &row.components {
            if let ActionRowComponent::InputText(t) = c {
                if t.custom_id == id {
                    return t.value.clone().unwrap_or_default();
                }
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Applicant flow
// ---------------------------------------------------------------------------

/// `/apply` or the panel button.
pub async fn start(st: &Arc<AppState>, resp: &dyn Responder, actor: &Actor) -> Result<()> {
    let user = actor.id_i64();
    if let Some(m) = repo::get_member(&st.db, user).await? {
        if m.status.is_current() {
            return Err(Error::conflict("You are already part of STYX."));
        }
    }
    if repo::open_application_for_user(&st.db, user).await?.is_some() {
        return Err(Error::conflict(
            "You already have an open application. Staff will get to it.",
        ));
    }
    resp.show_modal(st.http(), build_application_step_one()).await?;
    Ok(())
}

fn retry_hint(e: Error) -> Error {
    match e {
        Error::Validation(m) => Error::Validation(format!("{m}\n\nRun `/apply` or press APPLY again to retry.")),
        other => other,
    }
}

/// Step-1 modal submitted: validate, keep a draft, offer step 2.
pub async fn step_one_submit(st: &Arc<AppState>, modal: &ModalInteraction, actor: &Actor) -> Result<()> {
    let one = validate_step_one(
        &modal_value(modal, "roblox"),
        &modal_value(modal, "level"),
        &modal_value(modal, "rank"),
        &modal_value(modal, "elo"),
        &modal_value(modal, "winrate"),
    )
    .map_err(retry_hint)?;
    repo::save_draft(&st.db, actor.id_i64(), &serde_json::to_string(&one)?, Utc::now()).await?;
    modal
        .create_response(
            st.http(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .embed(embeds::application_step_two_prompt())
                    .components(vec![CreateActionRow::Buttons(vec![serenity::all::CreateButton::new(
                        "apply:next",
                    )
                    .label("CONTINUE")
                    .style(serenity::all::ButtonStyle::Danger)])])
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

/// "Continue" pressed: open step 2.
pub async fn next_step(st: &Arc<AppState>, comp: &ComponentInteraction, actor: &Actor) -> Result<()> {
    if repo::load_draft(&st.db, actor.id_i64(), Utc::now()).await?.is_none() {
        return Err(Error::not_found("Your draft expired. Run `/apply` again."));
    }
    comp.create_response(
        st.http(),
        CreateInteractionResponse::Modal(build_application_step_two()),
    )
    .await?;
    Ok(())
}

/// Step-2 modal submitted: store the application and notify staff.
pub async fn step_two_submit(st: &Arc<AppState>, modal: &ModalInteraction, actor: &Actor) -> Result<()> {
    let draft = repo::load_draft(&st.db, actor.id_i64(), Utc::now())
        .await?
        .ok_or_else(|| Error::not_found("Your draft expired. Run `/apply` again."))?;
    let one: StepOne = serde_json::from_str(&draft)?;
    let two = validate_step_two(
        &modal_value(modal, "loadout"),
        &modal_value(modal, "playstyle"),
        &modal_value(modal, "timezone"),
        &modal_value(modal, "motivation"),
    )
    .map_err(retry_hint)?;

    modal.defer_ephemeral(st.http()).await?;
    let new = NewApplication {
        discord_user_id: actor.id_i64(),
        discord_username: modal.user.name.clone(),
        one,
        two,
    };
    let app = service::submit(&st.db, &new, Utc::now()).await?;
    repo::delete_draft(&st.db, actor.id_i64()).await?;
    tracing::info!(application_id = app.id, "application submitted");

    if let Err(e) = actions::post_review_card(st, &app).await {
        tracing::error!(error = %e, application_id = app.id, "failed to post review card");
        actions::send_log(
            st,
            embeds::log_line(
                "WARNING",
                format!(
                    "Application #{} was saved but its review card could not be posted. Use `/application view {}`.",
                    app.id, app.id
                ),
            ),
        )
        .await;
    }
    modal
        .edit_response(
            st.http(),
            EditInteractionResponse::new().embeds(vec![embeds::application_received()]),
        )
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Staff review buttons
// ---------------------------------------------------------------------------

async fn open_application(st: &Arc<AppState>, id: i64) -> Result<super::model::Application> {
    let app = service::get(&st.db, id).await?;
    if !app.status.is_open() {
        return Err(Error::conflict(format!(
            "Application #{id} is already {}.",
            app.status.label().to_lowercase()
        )));
    }
    Ok(app)
}

pub async fn review_button(
    st: &Arc<AppState>,
    comp: &ComponentInteraction,
    actor: &Actor,
    action: &str,
    id: i64,
) -> Result<()> {
    let flag = st.cfg.charon_can_decide;
    match action {
        "review" => {
            require(actor.tier, Permission::Review, flag)?;
            let current = service::get(&st.db, id).await?;
            if current.status == ApplicationStatus::UnderReview {
                comp.create_response(
                    st.http(),
                    CreateInteractionResponse::Message(ephemeral("Already under review.")),
                )
                .await?;
                return Ok(());
            }
            let app = service::mark_under_review(&st.db, id, actor.id_i64(), Utc::now()).await?;
            comp.create_response(
                st.http(),
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embeds(vec![embeds::application_review(&app)])
                        .components(embeds::review_buttons(app.id, true)),
                ),
            )
            .await?;
        }
        "accept" => {
            require(actor.tier, Permission::Decide, flag)?;
            open_application(st, id).await?;
            ask_confirm(st, comp, actor, PendingAction::AcceptApplication { id }).await?;
        }
        "reject" => {
            require(actor.tier, Permission::Decide, flag)?;
            open_application(st, id).await?;
            comp.create_response(st.http(), CreateInteractionResponse::Modal(reject_modal(id)))
                .await?;
        }
        "info" => {
            require(actor.tier, Permission::Review, flag)?;
            open_application(st, id).await?;
            comp.create_response(st.http(), CreateInteractionResponse::Modal(info_modal(id)))
                .await?;
        }
        _ => return Err(Error::not_found("Unknown application action.")),
    }
    Ok(())
}

/// Reject-reason modal submitted: stage the rejection behind a confirmation.
pub async fn reject_modal_submit(st: &Arc<AppState>, modal: &ModalInteraction, actor: &Actor, id: i64) -> Result<()> {
    require(actor.tier, Permission::Decide, st.cfg.charon_can_decide)?;
    open_application(st, id).await?;
    let raw = modal_value(modal, "reason");
    let reason = if raw.trim().is_empty() {
        None
    } else {
        Some(validate_free_text("Reason", &raw, 500)?)
    };
    ask_confirm(st, modal, actor, PendingAction::RejectApplication { id, reason }).await
}

/// Info-request modal submitted.
pub async fn info_modal_submit(st: &Arc<AppState>, modal: &ModalInteraction, actor: &Actor, id: i64) -> Result<()> {
    let question = validate_free_text("Question", &modal_value(modal, "question"), 1000)?;
    modal.defer_ephemeral(st.http()).await?;
    let text = request_info(st, actor, id, &question).await?;
    modal
        .edit_response(st.http(), EditInteractionResponse::new().content(text))
        .await?;
    Ok(())
}

/// Ask the applicant for more information: DM first, controlled fallback second.
pub async fn request_info(st: &Arc<AppState>, actor: &Actor, id: i64, question: &str) -> Result<String> {
    require(actor.tier, Permission::Review, st.cfg.charon_can_decide)?;
    let app = service::request_info(&st.db, id, actor.id_i64(), question, Utc::now()).await?;
    let applicant = user_id(app.discord_user_id);
    let dm = CreateMessage::new()
        .embed(embeds::dm_info_request(question))
        .components(embeds::info_reply_button(id));
    let delivered = actions::dm_user(st, applicant, dm).await;
    let mut note = "Question sent by DM.".to_string();
    if !delivered {
        tracing::warn!(application_id = id, "failed to DM applicant");
        // Fallback: a prompt in the apply channel that contains no private details.
        let msg = CreateMessage::new()
            .content(format!(
                "<@{applicant}> staff need more information about your application. Press the button below; only you can open it."
            ))
            .components(embeds::info_view_button(id))
            .allowed_mentions(CreateAllowedMentions::new().users(vec![applicant]));
        match st.cfg.channels.apply.send_message(st.http(), msg).await {
            Ok(_) => {
                note = "The applicant has DMs closed. Posted a prompt in the apply channel (no private details).".into()
            }
            Err(e) => {
                tracing::error!(error = %e, application_id = id, "fallback info prompt failed");
                note = "The applicant has DMs closed and the fallback prompt failed. Contact them directly.".into();
            }
        }
    }
    actions::send_log(
        st,
        embeds::log_line(
            "INFO REQUESTED",
            format!(
                "Application #{id}\nApplicant: <@{applicant}>\nRequested by: <@{}>",
                actor.id
            ),
        ),
    )
    .await;
    Ok(note)
}

// ---------------------------------------------------------------------------
// Applicant replies to info requests
// ---------------------------------------------------------------------------

async fn applicant_application(st: &Arc<AppState>, user: i64, id: i64) -> Result<super::model::Application> {
    let app = service::get(&st.db, id).await?;
    if app.discord_user_id != user {
        return Err(Error::forbidden("That is not your application."));
    }
    if !app.status.is_open() {
        return Err(Error::conflict("This application is already closed."));
    }
    Ok(app)
}

/// Applicant pressed "see staff question" (fallback path) or "reply".
pub async fn applicant_button(st: &Arc<AppState>, comp: &ComponentInteraction, action: &str, id: i64) -> Result<()> {
    let app = applicant_application(st, uid_of(&comp.user), id).await?;
    match action {
        "viewq" => {
            let q = app
                .info_request_text
                .clone()
                .unwrap_or_else(|| "Staff have no open question.".into());
            comp.create_response(
                st.http(),
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .embed(embeds::dm_info_request(&q))
                        .components(embeds::info_reply_button(id))
                        .ephemeral(true),
                ),
            )
            .await?;
        }
        "reply" => {
            comp.create_response(st.http(), CreateInteractionResponse::Modal(reply_modal(id)))
                .await?;
        }
        _ => return Err(Error::not_found("Unknown action.")),
    }
    Ok(())
}

fn uid_of(u: &serenity::all::User) -> i64 {
    u.id.get() as i64
}

/// Applicant's reply submitted: forward it to staff.
pub async fn reply_modal_submit(st: &Arc<AppState>, modal: &ModalInteraction, id: i64) -> Result<()> {
    let user = uid_of(&modal.user);
    applicant_application(st, user, id).await?;
    let reply = validate_free_text("Reply", &modal_value(modal, "reply"), 1000)?;
    service::record_info_reply(&st.db, id, user, &reply, Utc::now()).await?;
    let embed = embeds::log_line(
        "INFO REPLY",
        format!(
            "Application #{id}\nApplicant: <@{user}>\n\n{}",
            embeds::clip(&reply, 3000)
        ),
    );
    st.cfg
        .channels
        .staff
        .send_message(st.http(), CreateMessage::new().embed(embed))
        .await?;
    modal
        .create_response(
            st.http(),
            CreateInteractionResponse::Message(ephemeral("Sent to staff. Thank you.")),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_one_fits_discord_modal_limits() {
        // Discord: at most 5 action rows per modal, title <= 45 chars, labels <= 45 chars.
        let m = serde_json::to_value(build_application_step_one()).unwrap();
        let rows = m["components"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        assert!(m["title"].as_str().unwrap().chars().count() <= 45);
        for r in rows {
            let label = r["components"][0]["label"].as_str().unwrap();
            assert!(label.chars().count() <= 45, "{label}");
        }
    }

    #[test]
    fn step_two_fits_discord_modal_limits() {
        let m = serde_json::to_value(build_application_step_two()).unwrap();
        let rows = m["components"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        for r in rows {
            let label = r["components"][0]["label"].as_str().unwrap();
            assert!(label.chars().count() <= 45, "{label}");
        }
    }

    #[test]
    fn all_application_fields_are_collected() {
        let one = serde_json::to_string(&build_application_step_one()).unwrap();
        let two = serde_json::to_string(&build_application_step_two()).unwrap();
        for needle in ["Roblox Username", "Level", "Rank", "ELO", "Win Rate"] {
            assert!(one.contains(needle), "{needle}");
        }
        for needle in ["Loadout", "Playstyle", "Timezone", "Why do you want to join STYX?"] {
            assert!(two.contains(needle), "{needle}");
        }
    }
}
