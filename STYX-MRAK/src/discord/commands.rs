//! Slash command definitions and handlers.
//!
//! Visibility is two-layered: staff commands are registered with
//! `default_member_permissions` (hidden from ordinary members in the UI), and
//! every handler also authorises in code from configured role IDs. Hiding a
//! command is never the only protection.

use crate::applications::handlers as app_handlers;
use crate::applications::model::{validate_free_text, ApplicationStatus};
use crate::db::repositories::{self as repo};
use crate::discord::actions::{self, ask_confirm, Actor, PendingAction};
use crate::discord::embeds;
use crate::discord::permissions::{can_act_on, require, Permission, Tier};
use crate::discord::responder::Responder;
use crate::error::{Error, Result};
use crate::members::model::{check_rank_change, validate_squad, ClanRank};
use crate::members::{roster, service as mem_svc};
use crate::moderation::service as mod_svc;
use crate::state::AppState;
use chrono::{Duration, Utc};
use serenity::all::{
    CommandInteraction, CommandOptionType, CreateActionRow, CreateCommand, CreateCommandOption, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal, EditInteractionResponse, InputTextStyle,
    PartialMember, Permissions, ResolvedOption, ResolvedValue, User,
};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------

fn sub(name: &str, desc: &str) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::SubCommand, name, desc)
}
fn user_opt(desc: &str) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::User, "user", desc).required(true)
}
fn str_opt(name: &str, desc: &str, required: bool, max: u16) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::String, name, desc)
        .required(required)
        .max_length(max)
}
fn int_opt(name: &str, desc: &str, min: u64, max: u64) -> CreateCommandOption {
    CreateCommandOption::new(CommandOptionType::Integer, name, desc)
        .required(true)
        .min_int_value(min)
        .max_int_value(max)
}
fn rank_opt(include_top: bool) -> CreateCommandOption {
    let mut o = CreateCommandOption::new(CommandOptionType::String, "rank", "Target clan rank").required(true);
    for r in [ClanRank::Nix, ClanRank::Styx, ClanRank::Charon, ClanRank::Kerberos] {
        o = o.add_string_choice(r.label(), r.as_str());
    }
    let _ = include_top;
    o
}

fn staff(name: &str, desc: &str) -> CreateCommand {
    CreateCommand::new(name)
        .description(desc)
        .dm_permission(false)
        .default_member_permissions(Permissions::MODERATE_MEMBERS)
}

pub fn definitions() -> Vec<CreateCommand> {
    let public = |n: &str, d: &str| CreateCommand::new(n).description(d).dm_permission(false);
    vec![
        public("apply", "Apply to join STYX"),
        public("profile", "Show your STYX record"),
        public("help", "What this bot can do"),
        staff("member", "Manage the STYX member registry")
            .add_option(sub("view", "Show a member record").add_sub_option(user_opt("Member")))
            .add_option(
                sub("update", "Edit a member record")
                    .add_sub_option(user_opt("Member"))
                    .add_sub_option(
                        CreateCommandOption::new(CommandOptionType::String, "section", "What to edit")
                            .required(true)
                            .add_string_choice("Rivals stats", "stats")
                            .add_string_choice("Loadout, playstyle, notes", "profile"),
                    ),
            )
            .add_option(
                sub("promote", "Raise a member's rank")
                    .add_sub_option(user_opt("Member"))
                    .add_sub_option(rank_opt(false)),
            )
            .add_option(
                sub("demote", "Lower a member's rank")
                    .add_sub_option(user_opt("Member"))
                    .add_sub_option(rank_opt(false)),
            )
            .add_option(
                sub("squad", "Assign a squad (type none to clear)")
                    .add_sub_option(user_opt("Member"))
                    .add_sub_option(str_opt("squad", "Squad name", true, 24)),
            )
            .add_option(sub("remove", "Remove a member from the registry").add_sub_option(user_opt("Member")))
            .add_option(
                sub("add", "Register someone who already holds a clan role").add_sub_option(user_opt("Member")),
            ),
        staff("application", "Review applications")
            .add_option(
                sub("list", "List applications").add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "status", "Filter (default: open)")
                        .required(false)
                        .add_string_choice("Open", "open")
                        .add_string_choice("Accepted", "accepted")
                        .add_string_choice("Rejected", "rejected")
                        .add_string_choice("Withdrawn", "withdrawn"),
                ),
            )
            .add_option(sub("view", "Show one application").add_sub_option(int_opt(
                "id",
                "Application ID",
                1,
                i64::MAX as u64,
            )))
            .add_option(sub("accept", "Accept an application").add_sub_option(int_opt(
                "id",
                "Application ID",
                1,
                i64::MAX as u64,
            )))
            .add_option(
                sub("reject", "Reject an application")
                    .add_sub_option(int_opt("id", "Application ID", 1, i64::MAX as u64))
                    .add_sub_option(str_opt("reason", "Private reason (optional)", false, 500)),
            )
            .add_option(
                sub("request-info", "Ask the applicant for more information")
                    .add_sub_option(int_opt("id", "Application ID", 1, i64::MAX as u64))
                    .add_sub_option(str_opt("question", "What do you need?", true, 1000)),
            ),
        staff("mod", "Moderation")
            .add_option(
                sub("warn", "Record a warning")
                    .add_sub_option(user_opt("User"))
                    .add_sub_option(str_opt("reason", "Reason", true, 400)),
            )
            .add_option(
                sub("timeout", "Time a user out")
                    .add_sub_option(user_opt("User"))
                    .add_sub_option(str_opt("duration", "e.g. 10m, 2h, 1d (max 28d)", true, 8))
                    .add_sub_option(str_opt("reason", "Reason", true, 400)),
            )
            .add_option(
                sub("kick", "Kick a user")
                    .add_sub_option(user_opt("User"))
                    .add_sub_option(str_opt("reason", "Reason", true, 400)),
            )
            .add_option(
                sub("ban", "Ban a user")
                    .add_sub_option(user_opt("User"))
                    .add_sub_option(str_opt("reason", "Reason", true, 400)),
            )
            .add_option(sub("purge", "Delete recent messages here").add_sub_option(int_opt("amount", "1-100", 1, 100))),
        public("roster", "The STYX roster")
            .add_option(sub("view", "Show the current roster"))
            .add_option(sub("refresh", "Regenerate the roster message (staff)")),
        staff("stats", "Clan and application statistics"),
        staff("health", "Bot health"),
        staff("panel", "Post persistent panels").add_option(sub("apply", "Post or refresh the apply panel")),
    ]
}

pub async fn register(st: &Arc<AppState>) -> Result<()> {
    let cmds = st.cfg.guild_id.set_commands(st.http(), definitions()).await?;
    tracing::info!(count = cmds.len(), "slash commands registered");
    Ok(())
}

// ---------------------------------------------------------------------------
// Option helpers
// ---------------------------------------------------------------------------

type Opts<'a> = Vec<ResolvedOption<'a>>;

fn split_sub(opts: Opts<'_>) -> (&str, Opts<'_>) {
    match opts.into_iter().next() {
        Some(ResolvedOption {
            name,
            value: ResolvedValue::SubCommand(inner),
            ..
        }) => (name, inner),
        _ => ("", Vec::new()),
    }
}

fn find<'a, 'b>(opts: &'b [ResolvedOption<'a>], name: &str) -> Option<&'b ResolvedValue<'a>> {
    opts.iter().find(|o| o.name == name).map(|o| &o.value)
}

fn opt_str<'a>(opts: &[ResolvedOption<'a>], name: &str) -> Option<&'a str> {
    match find(opts, name) {
        Some(ResolvedValue::String(s)) => Some(*s),
        _ => None,
    }
}

fn opt_int(opts: &[ResolvedOption<'_>], name: &str) -> Option<i64> {
    match find(opts, name) {
        Some(ResolvedValue::Integer(i)) => Some(*i),
        _ => None,
    }
}

fn opt_user<'a>(opts: &[ResolvedOption<'a>]) -> Result<(&'a User, Option<&'a PartialMember>)> {
    match find(opts, "user") {
        Some(ResolvedValue::User(u, m)) => Ok((*u, *m)),
        _ => Err(Error::validation("Pick a user.")),
    }
}

fn need_str<'a>(opts: &[ResolvedOption<'a>], name: &str) -> Result<&'a str> {
    opt_str(opts, name).ok_or_else(|| Error::validation(format!("`{name}` is required.")))
}

fn need_int(opts: &[ResolvedOption<'_>], name: &str) -> Result<i64> {
    opt_int(opts, name).ok_or_else(|| Error::validation(format!("`{name}` is required.")))
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch(st: &Arc<AppState>, cmd: &CommandInteraction, actor: &Actor) -> Result<()> {
    let flag = st.cfg.charon_can_decide;
    let opts = cmd.data.options();
    let (sub, o) = split_sub(opts);
    match cmd.data.name.as_str() {
        "apply" => app_handlers::start(st, cmd, actor).await,
        "profile" => {
            require(actor.tier, Permission::Member, flag)?;
            let m = mem_svc::require_current(&st.db, actor.id_i64()).await?;
            cmd.reply(
                st.http(),
                CreateInteractionResponseMessage::new()
                    .embed(embeds::member_card(&m, false))
                    .ephemeral(true),
            )
            .await?;
            Ok(())
        }
        "help" => {
            let staff = crate::discord::permissions::allowed(actor.tier, Permission::Review, flag);
            cmd.reply(
                st.http(),
                CreateInteractionResponseMessage::new()
                    .embed(embeds::help(staff))
                    .ephemeral(true),
            )
            .await?;
            Ok(())
        }
        "member" => member(st, cmd, actor, sub, &o).await,
        "application" => application(st, cmd, actor, sub, &o).await,
        "mod" => moderation(st, cmd, actor, sub, &o).await,
        "roster" => roster_cmd(st, cmd, actor, sub).await,
        "stats" => {
            require(actor.tier, Permission::Staff, flag)?;
            stats(st, cmd).await
        }
        "health" => {
            require(actor.tier, Permission::Staff, flag)?;
            health(st, cmd).await
        }
        "panel" => {
            require(actor.tier, Permission::Staff, flag)?;
            cmd.defer_ephemeral(st.http()).await?;
            actions::ensure_apply_panel(st).await?;
            cmd.edit_response(st.http(), EditInteractionResponse::new().content("Apply panel posted."))
                .await?;
            Ok(())
        }
        _ => Err(Error::not_found("Unknown command.")),
    }
}

// ---------------------------------------------------------------------------
// /member
// ---------------------------------------------------------------------------

async fn member(
    st: &Arc<AppState>,
    cmd: &CommandInteraction,
    actor: &Actor,
    sub: &str,
    o: &[ResolvedOption<'_>],
) -> Result<()> {
    let flag = st.cfg.charon_can_decide;
    match sub {
        "view" => {
            require(actor.tier, Permission::Review, flag)?;
            let (u, _) = opt_user(o)?;
            let m = repo::get_member(&st.db, u.id.get() as i64)
                .await?
                .ok_or_else(|| Error::not_found("That user is not in the registry."))?;
            let notes = actor.tier >= Tier::Kerberos;
            cmd.reply(
                st.http(),
                CreateInteractionResponseMessage::new()
                    .embed(embeds::member_card(&m, notes))
                    .ephemeral(true),
            )
            .await?;
        }
        "update" => {
            require(actor.tier, Permission::Staff, flag)?;
            let (u, _) = opt_user(o)?;
            let m = mem_svc::require_current(&st.db, u.id.get() as i64).await?;
            let modal = match need_str(o, "section")? {
                "stats" => stats_modal(&m),
                _ => profile_modal(&m),
            };
            cmd.create_response(st.http(), CreateInteractionResponse::Modal(modal))
                .await?;
        }
        "promote" | "demote" => {
            require(actor.tier, Permission::Staff, flag)?;
            let promote = sub == "promote";
            let (u, _) = opt_user(o)?;
            let to: ClanRank = need_str(o, "rank")?.parse()?;
            let m = mem_svc::require_current(&st.db, u.id.get() as i64).await?;
            // Validate now so staff get an immediate answer instead of a doomed confirmation.
            check_rank_change(actor.tier, m.clan_rank, to, promote)?;
            ask_confirm(
                st,
                cmd,
                actor,
                PendingAction::ChangeRank {
                    user_id: m.discord_user_id,
                    from: m.clan_rank,
                    to,
                    promote,
                },
            )
            .await?;
        }
        "squad" => {
            require(actor.tier, Permission::Staff, flag)?;
            let (u, _) = opt_user(o)?;
            let squad = validate_squad(need_str(o, "squad")?)?;
            cmd.defer_ephemeral(st.http()).await?;
            let m = mem_svc::set_squad(&st.db, actor.id_i64(), u.id.get() as i64, squad.as_deref(), Utc::now()).await?;
            let mut text = match &m.squad {
                Some(s) => format!("<@{}> assigned to squad **{s}**.", m.discord_user_id),
                None => format!("<@{}> removed from their squad.", m.discord_user_id),
            };
            if let Err(e) = actions::refresh_roster(st).await {
                tracing::error!(error = %e, "failed to update roster");
                text.push_str("\n\n⚠ Roster refresh failed. Run `/roster refresh`.");
            }
            cmd.edit_response(st.http(), EditInteractionResponse::new().content(text))
                .await?;
        }
        "remove" => {
            require(actor.tier, Permission::Staff, flag)?;
            let (u, _) = opt_user(o)?;
            let m = mem_svc::require_current(&st.db, u.id.get() as i64).await?;
            if !can_act_on(actor.tier, Tier::from(m.clan_rank), actor.id_i64() == m.discord_user_id) {
                return Err(Error::forbidden("You cannot remove that member."));
            }
            ask_confirm(
                st,
                cmd,
                actor,
                PendingAction::RemoveMember {
                    user_id: m.discord_user_id,
                },
            )
            .await?;
        }
        "add" => {
            require(actor.tier, Permission::Staff, flag)?;
            let (u, pm) = opt_user(o)?;
            let roles = pm.map(|p| p.roles.clone()).unwrap_or_default();
            let rank = st
                .cfg
                .roles
                .tier_of(&roles)
                .clan_rank()
                .ok_or_else(|| Error::validation("That user holds none of the STYX roles."))?;
            if rank >= ClanRank::Kerberos && actor.tier != Tier::Warden {
                return Err(Error::forbidden("Only the Warden can register Kerberos or Warden."));
            }
            cmd.defer_ephemeral(st.http()).await?;
            let m =
                mem_svc::import_existing(&st.db, actor.id_i64(), u.id.get() as i64, &u.name, rank, Utc::now()).await?;
            let mut text = format!("<@{}> registered as {}.", m.discord_user_id, m.clan_rank.label());
            if let Err(e) = actions::refresh_roster(st).await {
                tracing::error!(error = %e, "failed to update roster");
                text.push_str("\n\n⚠ Roster refresh failed. Run `/roster refresh`.");
            }
            cmd.edit_response(st.http(), EditInteractionResponse::new().content(text))
                .await?;
        }
        _ => return Err(Error::not_found("Unknown subcommand.")),
    }
    Ok(())
}

fn prefilled(
    label: &str,
    id: &str,
    style: InputTextStyle,
    value: Option<String>,
    max: u16,
    required: bool,
) -> CreateActionRow {
    let mut i = CreateInputText::new(style, label, id)
        .max_length(max)
        .required(required);
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        i = i.value(v);
    }
    CreateActionRow::InputText(i)
}

fn stats_modal(m: &crate::members::model::Member) -> CreateModal {
    CreateModal::new(format!("mem:upd:stats:{}", m.discord_user_id), "Update Rivals stats").components(vec![
        prefilled(
            "Roblox Username",
            "roblox",
            InputTextStyle::Short,
            m.roblox_username.clone(),
            20,
            true,
        ),
        prefilled(
            "Rivals Player Level",
            "level",
            InputTextStyle::Short,
            m.player_level.map(|v| v.to_string()),
            6,
            true,
        ),
        prefilled(
            "Current Rank",
            "rank",
            InputTextStyle::Short,
            m.current_rank.clone(),
            32,
            true,
        ),
        prefilled(
            "Current ELO",
            "elo",
            InputTextStyle::Short,
            m.elo.map(|v| v.to_string()),
            6,
            true,
        ),
        prefilled(
            "Win Rate",
            "winrate",
            InputTextStyle::Short,
            m.win_rate.map(embeds::fmt_pct),
            8,
            true,
        ),
    ])
}

fn profile_modal(m: &crate::members::model::Member) -> CreateModal {
    CreateModal::new(
        format!("mem:upd:profile:{}", m.discord_user_id),
        "Update member profile",
    )
    .components(vec![
        prefilled(
            "Preferred Loadout",
            "loadout",
            InputTextStyle::Short,
            m.preferred_loadout.clone(),
            100,
            false,
        ),
        prefilled(
            "Playstyle",
            "playstyle",
            InputTextStyle::Short,
            m.playstyle.clone(),
            100,
            false,
        ),
        prefilled(
            "Staff notes (private)",
            "notes",
            InputTextStyle::Paragraph,
            m.notes.clone(),
            1000,
            false,
        ),
    ])
}

// ---------------------------------------------------------------------------
// /application
// ---------------------------------------------------------------------------

async fn application(
    st: &Arc<AppState>,
    cmd: &CommandInteraction,
    actor: &Actor,
    sub: &str,
    o: &[ResolvedOption<'_>],
) -> Result<()> {
    let flag = st.cfg.charon_can_decide;
    match sub {
        "list" => {
            require(actor.tier, Permission::Review, flag)?;
            let (apps, title) = match opt_str(o, "status").unwrap_or("open") {
                "open" => (repo::list_open_applications(&st.db, 25).await?, "OPEN APPLICATIONS"),
                other => {
                    let s: ApplicationStatus = other.parse()?;
                    (repo::list_applications_by_status(&st.db, s, 25).await?, "APPLICATIONS")
                }
            };
            cmd.reply(
                st.http(),
                CreateInteractionResponseMessage::new()
                    .embed(embeds::application_list(&apps, title))
                    .ephemeral(true),
            )
            .await?;
        }
        "view" => {
            require(actor.tier, Permission::Review, flag)?;
            let app = crate::applications::service::get(&st.db, need_int(o, "id")?).await?;
            let mut e = embeds::application_review(&app);
            if let Some(n) = &app.review_notes {
                e = e.field("STAFF NOTES", embeds::clip(n, 1000), false);
            }
            if let Some(q) = &app.info_request_text {
                e = e.field("INFO REQUESTED", embeds::clip(q, 1000), false);
            }
            cmd.reply(
                st.http(),
                CreateInteractionResponseMessage::new().embed(e).ephemeral(true),
            )
            .await?;
        }
        "accept" => {
            require(actor.tier, Permission::Decide, flag)?;
            let id = need_int(o, "id")?;
            ensure_open(st, id).await?;
            ask_confirm(st, cmd, actor, PendingAction::AcceptApplication { id }).await?;
        }
        "reject" => {
            require(actor.tier, Permission::Decide, flag)?;
            let id = need_int(o, "id")?;
            ensure_open(st, id).await?;
            let reason = opt_str(o, "reason")
                .map(|r| validate_free_text("Reason", r, 500))
                .transpose()?;
            ask_confirm(st, cmd, actor, PendingAction::RejectApplication { id, reason }).await?;
        }
        "request-info" => {
            require(actor.tier, Permission::Review, flag)?;
            let id = need_int(o, "id")?;
            let q = validate_free_text("Question", need_str(o, "question")?, 1000)?;
            cmd.defer_ephemeral(st.http()).await?;
            let text = app_handlers::request_info(st, actor, id, &q).await?;
            cmd.edit_response(st.http(), EditInteractionResponse::new().content(text))
                .await?;
        }
        _ => return Err(Error::not_found("Unknown subcommand.")),
    }
    Ok(())
}

async fn ensure_open(st: &Arc<AppState>, id: i64) -> Result<()> {
    let app = crate::applications::service::get(&st.db, id).await?;
    if app.status.is_open() {
        Ok(())
    } else {
        Err(Error::conflict(format!(
            "Application #{id} is already {}.",
            app.status.label().to_lowercase()
        )))
    }
}

// ---------------------------------------------------------------------------
// /mod
// ---------------------------------------------------------------------------

async fn moderation(
    st: &Arc<AppState>,
    cmd: &CommandInteraction,
    actor: &Actor,
    sub: &str,
    o: &[ResolvedOption<'_>],
) -> Result<()> {
    require(actor.tier, Permission::Staff, st.cfg.charon_can_decide)?;
    match sub {
        "warn" | "kick" | "ban" => {
            let (u, _) = opt_user(o)?;
            let target = u.id.get() as i64;
            let reason = mod_svc::validate_reason(need_str(o, "reason")?)?;
            // Fail fast on hierarchy before asking for confirmation.
            actions::guard_target(st, actor, target).await?;
            let action = match sub {
                "warn" => PendingAction::Warn {
                    user_id: target,
                    reason,
                },
                "kick" => PendingAction::Kick {
                    user_id: target,
                    reason,
                },
                _ => PendingAction::Ban {
                    user_id: target,
                    reason,
                },
            };
            ask_confirm(st, cmd, actor, action).await?;
        }
        "timeout" => {
            let (u, _) = opt_user(o)?;
            let duration: Duration = mod_svc::parse_duration(need_str(o, "duration")?)?;
            let reason = mod_svc::validate_reason(need_str(o, "reason")?)?;
            cmd.defer_ephemeral(st.http()).await?;
            let text = actions::timeout_member(st, actor, u.id.get() as i64, duration, &reason).await?;
            cmd.edit_response(st.http(), EditInteractionResponse::new().content(text))
                .await?;
        }
        "purge" => {
            let amount = mod_svc::validate_purge_amount(need_int(o, "amount")?)?;
            ask_confirm(
                st,
                cmd,
                actor,
                PendingAction::Purge {
                    channel_id: cmd.channel_id.get() as i64,
                    amount,
                },
            )
            .await?;
        }
        _ => return Err(Error::not_found("Unknown subcommand.")),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /roster, /stats, /health
// ---------------------------------------------------------------------------

async fn roster_cmd(st: &Arc<AppState>, cmd: &CommandInteraction, actor: &Actor, sub: &str) -> Result<()> {
    let flag = st.cfg.charon_can_decide;
    match sub {
        "view" => {
            require(actor.tier, Permission::Member, flag)?;
            let members = repo::list_current_members(&st.db).await?;
            let view = roster::build_roster(&members);
            cmd.reply(
                st.http(),
                CreateInteractionResponseMessage::new()
                    .embed(embeds::roster(&view, Utc::now()))
                    .ephemeral(true),
            )
            .await?;
        }
        "refresh" => {
            require(actor.tier, Permission::Staff, flag)?;
            cmd.defer_ephemeral(st.http()).await?;
            actions::refresh_roster(st).await?;
            cmd.edit_response(st.http(), EditInteractionResponse::new().content("Roster refreshed."))
                .await?;
        }
        _ => return Err(Error::not_found("Unknown subcommand.")),
    }
    Ok(())
}

async fn stats(st: &Arc<AppState>, cmd: &CommandInteraction) -> Result<()> {
    let apps = repo::application_counts(&st.db).await?;
    let members = repo::member_counts(&st.db).await?;
    let week = repo::count_audit_since(&st.db, Utc::now() - Duration::days(7)).await?;
    let fmt = |rows: &[(String, i64)]| {
        if rows.is_empty() {
            "none".to_string()
        } else {
            rows.iter()
                .map(|(s, n)| format!("{}: {n}", s.to_uppercase()))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    let e = embeds::result(&format!(
        "**APPLICATIONS**\n{}\n\n**MEMBERS**\n{}\n\n**AUDIT EVENTS (7d)**\n{week}",
        fmt(&apps),
        fmt(&members)
    ));
    cmd.reply(
        st.http(),
        CreateInteractionResponseMessage::new().embed(e).ephemeral(true),
    )
    .await?;
    Ok(())
}

async fn health(st: &Arc<AppState>, cmd: &CommandInteraction) -> Result<()> {
    cmd.defer_ephemeral(st.http()).await?;
    let db_ok = crate::db::ping(&st.db).await;
    let discord_ok = st.http().get_current_user().await.is_ok();
    let feed = st.feed_status.lock().await.clone();
    let feed_line = if !feed.enabled {
        "DISABLED".to_string()
    } else if feed.last_check.is_none() {
        "STARTING".to_string()
    } else if feed.last_ok {
        "OK".to_string()
    } else {
        format!("DEGRADED ({} consecutive failures)", feed.consecutive_failures)
    };
    let last_feed = feed
        .last_check
        .map(|t| format!("<t:{}:R>", t.timestamp()))
        .unwrap_or_else(|| "never".into());
    let last_roster = {
        let mem = st.last_roster_update.lock().ok().and_then(|g| *g);
        match mem {
            Some(t) => Some(t),
            None => repo::get_config(&st.db, "roster.updated_at")
                .await
                .ok()
                .flatten()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|t| t.with_timezone(&Utc)),
        }
    }
    .map(|t| format!("<t:{}:R>", t.timestamp()))
    .unwrap_or_else(|| "never".into());
    let up = st.started_at.elapsed().as_secs();
    let text = format!(
        "Bot: ONLINE\nDatabase: {}\nDiscord: {}\nFeed: {feed_line}\nLast feed check: {last_feed}\nLast roster update: {last_roster}\nUptime: {}h {}m",
        if db_ok { "OK" } else { "FAIL" },
        if discord_ok { "OK" } else { "FAIL" },
        up / 3600,
        (up % 3600) / 60
    );
    cmd.edit_response(
        st.http(),
        EditInteractionResponse::new().embeds(vec![embeds::result(&text)]),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_set_is_complete_and_within_discord_limits() {
        let defs = serde_json::to_value(definitions()).unwrap();
        let names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        for n in [
            "apply",
            "profile",
            "help",
            "member",
            "application",
            "mod",
            "roster",
            "stats",
            "health",
            "panel",
        ] {
            assert!(names.contains(&n), "missing /{n}");
        }
        // Discord allows at most 100 global/guild commands and 25 subcommands each.
        assert!(names.len() <= 100);
        for c in defs.as_array().unwrap() {
            if let Some(opts) = c["options"].as_array() {
                assert!(opts.len() <= 25);
                for o in opts {
                    assert!(o["name"].as_str().unwrap().len() <= 32);
                    assert!(o["description"].as_str().unwrap().chars().count() <= 100);
                }
            }
        }
    }

    #[test]
    fn staff_commands_are_hidden_from_regular_members() {
        let defs = serde_json::to_value(definitions()).unwrap();
        for c in defs.as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let restricted = !c["default_member_permissions"].is_null();
            let public = ["apply", "profile", "help", "roster"].contains(&name);
            assert_eq!(restricted, !public, "/{name}");
        }
    }
}
