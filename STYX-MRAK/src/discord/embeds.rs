//! Every embed and button layout in one place. To restyle the bot, start here.

use crate::applications::model::{Application, ApplicationStatus};
use crate::feeds::source::FeedItem;
use crate::members::model::{ClanRank, Member};
use crate::members::roster::{chunk_lines, entry_line, RosterView};
use chrono::{DateTime, Utc};
use serenity::all::{ButtonStyle, Colour, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, Timestamp};

/// STYX palette.
pub const BLOOD_RED: u32 = 0x8B_00_00;
pub const COLD_WHITE: u32 = 0xE8_E8_E8;
pub const BLACK: u32 = 0x0A_0A_0A;
pub const FOOTER: &str = "STYX // 0x29A";
pub const RULE: &str = "━━━━━━━━━━━━━━━━━━━━";

pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn base(title: impl Into<String>) -> CreateEmbed {
    CreateEmbed::new()
        .colour(Colour::new(BLOOD_RED))
        .title(title)
        .footer(CreateEmbedFooter::new(FOOTER))
}

pub fn fmt_pct(v: f64) -> String {
    if (v.fract()).abs() < f64::EPSILON {
        format!("{}%", v as i64)
    } else {
        format!("{v:.1}%")
    }
}

fn ts(t: DateTime<Utc>) -> String {
    format!("<t:{}:f>", t.timestamp())
}

// ---------------------------------------------------------------------------
// Applications
// ---------------------------------------------------------------------------

pub fn apply_panel() -> CreateEmbed {
    base("ϟ МРАК // APPLICATIONS").description(
        "Want into STYX?\n\n\
         Submit your Rivals record.\n\
         Staff reviews every application.\n\n\
         No fake stats.\n\
         No bullshit.\n\
         No guaranteed acceptance.",
    )
}

pub fn apply_panel_buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![CreateButton::new("apply:start")
        .label("APPLY")
        .style(ButtonStyle::Danger)])]
}

pub fn application_received() -> CreateEmbed {
    base("ϟ МРАК").description("Application received.\n\nSit tight.\nStaff will review your request.")
}

pub fn application_step_two_prompt() -> CreateEmbed {
    base("ϟ МРАК // APPLICATION").description("Step 1 of 2 saved.\n\nContinue with your loadout and reasons.")
}

pub fn application_review(app: &Application) -> CreateEmbed {
    let colour = match app.status {
        ApplicationStatus::Pending | ApplicationStatus::UnderReview => BLOOD_RED,
        ApplicationStatus::Accepted => COLD_WHITE,
        ApplicationStatus::Rejected | ApplicationStatus::Withdrawn => BLACK,
    };
    let mut e = CreateEmbed::new()
        .colour(Colour::new(colour))
        .title("ϟ МРАК // NEW APPLICATION")
        .footer(CreateEmbedFooter::new(format!("Application #{} · {}", app.id, FOOTER)))
        .field("Applicant", format!("<@{}>", app.discord_user_id), false)
        .field("ROBLOX", clip(&app.roblox_username, 100), false)
        .field(
            "RIVALS",
            format!(
                "Level: {}\nRank: {}\nELO: {}\nWin Rate: {}",
                app.player_level,
                clip(&app.current_rank, 40),
                app.elo,
                fmt_pct(app.win_rate)
            ),
            false,
        )
        .field("LOADOUT", clip(&app.preferred_loadout, 200), false)
        .field("PLAYSTYLE", clip(&app.playstyle, 200), true)
        .field("TIMEZONE", clip(&app.timezone, 64), true)
        .field("WHY STYX", clip(&app.motivation, 1000), false)
        .field("STATUS", app.status.label(), false);
    if let (Some(by), Some(at)) = (app.reviewed_by, app.reviewed_at) {
        if !app.status.is_open() {
            e = e.field("DECISION", format!("by <@{by}> · {}", ts(at)), false);
        }
    }
    e
}

pub fn review_buttons(app_id: i64, open: bool) -> Vec<CreateActionRow> {
    let d = !open;
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("app:review:{app_id}"))
            .label("REVIEW")
            .style(ButtonStyle::Secondary)
            .disabled(d),
        CreateButton::new(format!("app:accept:{app_id}"))
            .label("ACCEPT")
            .style(ButtonStyle::Success)
            .disabled(d),
        CreateButton::new(format!("app:reject:{app_id}"))
            .label("REJECT")
            .style(ButtonStyle::Danger)
            .disabled(d),
        CreateButton::new(format!("app:info:{app_id}"))
            .label("REQUEST INFO")
            .style(ButtonStyle::Secondary)
            .disabled(d),
    ])]
}

pub fn application_row(app: &Application) -> String {
    format!(
        "`#{}` <@{}> · {} · {} · {}",
        app.id,
        app.discord_user_id,
        clip(&app.roblox_username, 20),
        app.status.label(),
        ts(app.submitted_at)
    )
}

pub fn application_list(apps: &[Application], title: &str) -> CreateEmbed {
    let body = if apps.is_empty() {
        "Nothing here.".to_string()
    } else {
        apps.iter().map(application_row).collect::<Vec<_>>().join("\n")
    };
    base(format!("ϟ МРАК // {title}")).description(clip(&body, 4000))
}

// ---------------------------------------------------------------------------
// DMs
// ---------------------------------------------------------------------------

pub fn dm_accepted() -> CreateEmbed {
    base("ϟ МРАК // STYX")
        .description("You're in.\n\nYour trial begins now.\n\nPlay with the squad.\nShow up.\nEarn your place.")
}

pub fn dm_rejected() -> CreateEmbed {
    base("ϟ МРАК // STYX").description(
        "Your application was reviewed and it's not a fit right now.\n\n\
         Keep playing. You're welcome to apply again later.",
    )
}

pub fn dm_info_request(question: &str) -> CreateEmbed {
    base("ϟ МРАК // STYX")
        .description("Staff need a bit more information before deciding on your application.")
        .field("QUESTION", clip(question, 1000), false)
}

pub fn info_reply_button(app_id: i64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![CreateButton::new(format!(
        "app:reply:{app_id}"
    ))
    .label("REPLY TO STAFF")
    .style(ButtonStyle::Primary)])]
}

pub fn info_view_button(app_id: i64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![CreateButton::new(format!(
        "app:viewq:{app_id}"
    ))
    .label("SEE STAFF QUESTION")
    .style(ButtonStyle::Primary)])]
}

pub fn dm_promoted(from: ClanRank, to: ClanRank) -> CreateEmbed {
    base("ϟ МРАК").description(format!(
        "You've crossed.\n\n{} → {}\n\nYou earned the next step.",
        from.label(),
        to.label()
    ))
}

// ---------------------------------------------------------------------------
// Members & roster
// ---------------------------------------------------------------------------

pub fn member_card(m: &Member, show_notes: bool) -> CreateEmbed {
    let opt = |o: &Option<String>| o.as_deref().map(|s| clip(s, 200)).unwrap_or_else(|| "—".into());
    let mut e = base("ϟ МРАК // MEMBER")
        .field("Discord", format!("<@{}>", m.discord_user_id), true)
        .field("Roblox", opt(&m.roblox_username), true)
        .field("Status", m.status.label(), true)
        .field("Clan rank", m.clan_rank.label(), true)
        .field("Squad", opt(&m.squad), true)
        .field(
            "Rivals level",
            m.player_level.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            true,
        )
        .field("Rank", opt(&m.current_rank), true)
        .field("ELO", m.elo.map(|v| v.to_string()).unwrap_or_else(|| "—".into()), true)
        .field("Win rate", m.win_rate.map(fmt_pct).unwrap_or_else(|| "—".into()), true)
        .field("Loadout", opt(&m.preferred_loadout), false)
        .field("Playstyle", opt(&m.playstyle), false)
        .field("Joined", ts(m.joined_at), true)
        .field("Last updated", ts(m.last_updated_at), true);
    if show_notes {
        if let Some(n) = &m.notes {
            e = e.field("Staff notes", clip(n, 1000), false);
        }
    }
    e
}

pub fn roster(view: &RosterView, now: DateTime<Utc>) -> CreateEmbed {
    let mut e = base("𖤐 STYX // ROSTER");
    // Discord caps an embed at 6000 characters; stay safely under it.
    let mut budget: usize = 5200;
    for section in &view.sections {
        let lines: Vec<String> = section.entries.iter().map(entry_line).collect();
        let max_lines = (budget / 24).max(1);
        let (chunks, dropped) = chunk_lines(&lines, 1000, max_lines);
        for (i, chunk) in chunks.iter().enumerate() {
            budget = budget.saturating_sub(chunk.chars().count() + 12);
            let name = if i == 0 { section.rank.label() } else { "\u{200b}" };
            e = e.field(name, chunk.clone(), false);
        }
        if dropped > 0 {
            e = e.field("\u{200b}", format!("+{dropped} more"), false);
        }
    }
    if view.sections.is_empty() {
        e = e.description("No members registered yet.");
    }
    let mut counts = format!("ACTIVE  {:>4}\nTRIAL   {:>4}", view.active, view.trial);
    if view.inactive > 0 {
        counts.push_str(&format!("\nINACTIVE{:>4}", view.inactive));
    }
    e.field(RULE, format!("```\n{counts}\n```"), false)
        .timestamp(Timestamp::from_unix_timestamp(now.timestamp()).unwrap_or_else(|_| Timestamp::now()))
}

// ---------------------------------------------------------------------------
// Logs, moderation, status
// ---------------------------------------------------------------------------

pub fn log_line(title: &str, body: impl Into<String>) -> CreateEmbed {
    base(format!("ϟ МРАК // {title}"))
        .description(clip(&body.into(), 3900))
        .timestamp(Timestamp::now())
}

pub fn moderation_log(
    action: &str,
    user: u64,
    duration: Option<&str>,
    reason: &str,
    moderator: Option<u64>,
) -> CreateEmbed {
    let mut body = format!("Action: {action}\nUser: <@{user}>\n");
    if let Some(d) = duration {
        body.push_str(&format!("Duration: {d}\n"));
    }
    body.push_str(&format!("Reason: {}\n", clip(reason, 400)));
    match moderator {
        Some(m) => body.push_str(&format!("Moderator: <@{m}>")),
        None => body.push_str("Moderator: automod"),
    }
    log_line("MODERATION", body)
}

pub fn intel_drop(item: &FeedItem, source_label: &str) -> CreateEmbed {
    let mut e = CreateEmbed::new()
        .colour(Colour::new(BLOOD_RED))
        .title("⸸ STYX // INTEL DROP")
        .description(format!(
            "**RIVALS UPDATE**\n\nNew RIVALS information detected.\n\n{}",
            clip(&item.text, 3000)
        ))
        .field("SOURCE", format!("{source_label}\n{}", item.url), false)
        .footer(CreateEmbedFooter::new(format!("{RULE}\n{FOOTER}")));
    if let Some(t) = item.created_at {
        if let Ok(stamp) = Timestamp::from_unix_timestamp(t.timestamp()) {
            e = e.timestamp(stamp);
        }
    }
    e
}

pub fn startup(guild: &str) -> CreateEmbed {
    log_line(
        "ONLINE",
        format!(
            "ϟ МРАК ONLINE\n\nGuild: {guild}\nDatabase: connected\nCommands: registered\n\
             Application system: ready\nRoster system: ready\nFeed system: ready"
        ),
    )
}

pub fn help(staff: bool) -> CreateEmbed {
    let mut text = String::from(
        "`/apply` — start an application\n`/profile` — your clan record\n`/roster view` — current roster\n`/help` — this list",
    );
    if staff {
        text.push_str(
            "\n\n**Staff**\n`/application list|view|accept|reject|request-info`\n\
             `/member view|update|promote|demote|squad|remove|add`\n\
             `/mod warn|timeout|kick|ban|purge`\n`/roster refresh` · `/panel apply` · `/stats` · `/health`",
        );
    }
    base("ϟ МРАК // HELP").description(text)
}

pub fn confirm_buttons(pending_id: i64, confirm_label: &str, danger: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("act:ok:{pending_id}"))
            .label(confirm_label)
            .style(if danger {
                ButtonStyle::Danger
            } else {
                ButtonStyle::Success
            }),
        CreateButton::new(format!("act:no:{pending_id}"))
            .label("CANCEL")
            .style(ButtonStyle::Secondary),
    ])]
}

pub fn confirm_prompt(text: &str) -> CreateEmbed {
    base("ϟ МРАК // CONFIRM").description(format!("{text}\n\nThis action will be recorded."))
}

pub fn result(text: &str) -> CreateEmbed {
    base("ϟ МРАК").description(clip(text, 3900))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_is_char_safe() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("ϟϟϟϟϟϟ", 3).chars().count(), 3);
    }

    #[test]
    fn percent_format() {
        assert_eq!(fmt_pct(71.0), "71%");
        assert_eq!(fmt_pct(71.5), "71.5%");
        assert_eq!(fmt_pct(0.0), "0%");
    }
}
