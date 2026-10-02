//! Roster generation. Pure data transformation; the Discord layer renders
//! and posts the result.

use super::model::{ClanRank, Member, MemberStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterEntry {
    pub user_id: i64,
    pub squad: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterSection {
    pub rank: ClanRank,
    pub entries: Vec<RosterEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterView {
    /// Highest rank first. Empty ranks are omitted.
    pub sections: Vec<RosterSection>,
    pub active: usize,
    pub trial: usize,
    pub inactive: usize,
}

/// Build the roster from registry rows. Only current members (trial, active,
/// inactive) are listed; removed/retired members are excluded.
pub fn build_roster(members: &[Member]) -> RosterView {
    let mut sections = Vec::new();
    for rank in ClanRank::ALL.iter().rev() {
        let mut entries: Vec<RosterEntry> = members
            .iter()
            .filter(|m| m.status.is_current() && m.clan_rank == *rank)
            .map(|m| RosterEntry {
                user_id: m.discord_user_id,
                squad: m.squad.clone(),
            })
            .collect();
        if entries.is_empty() {
            continue;
        }
        entries.sort_by_key(|e| e.user_id);
        sections.push(RosterSection { rank: *rank, entries });
    }
    let count = |s: MemberStatus| members.iter().filter(|m| m.status == s).count();
    RosterView {
        sections,
        active: count(MemberStatus::Active),
        trial: count(MemberStatus::Trial),
        inactive: count(MemberStatus::Inactive),
    }
}

pub fn entry_line(e: &RosterEntry) -> String {
    match &e.squad {
        Some(s) => format!("<@{}> · {}", e.user_id, s),
        None => format!("<@{}>", e.user_id),
    }
}

/// Split lines into chunks of at most `max_chars` (Discord field values are
/// limited to 1024). Returns the chunks and how many lines were dropped when
/// `max_lines` was exceeded.
pub fn chunk_lines(lines: &[String], max_chars: usize, max_lines: usize) -> (Vec<String>, usize) {
    let dropped = lines.len().saturating_sub(max_lines);
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for line in lines.iter().take(max_lines) {
        if !cur.is_empty() && cur.chars().count() + 1 + line.chars().count() > max_chars {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    (chunks, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn member(id: i64, rank: ClanRank, status: MemberStatus, squad: Option<&str>) -> Member {
        Member {
            id,
            discord_user_id: id,
            discord_username: format!("u{id}"),
            roblox_username: None,
            status,
            clan_rank: rank,
            squad: squad.map(str::to_string),
            player_level: None,
            current_rank: None,
            elo: None,
            win_rate: None,
            preferred_loadout: None,
            playstyle: None,
            joined_at: Utc::now(),
            last_updated_at: Utc::now(),
            notes: None,
        }
    }

    #[test]
    fn sections_are_ordered_and_counted() {
        let ms = vec![
            member(1, ClanRank::Nix, MemberStatus::Trial, None),
            member(2, ClanRank::Warden, MemberStatus::Active, None),
            member(3, ClanRank::Styx, MemberStatus::Active, Some("Alpha")),
            member(4, ClanRank::Kerberos, MemberStatus::Active, None),
            member(5, ClanRank::Styx, MemberStatus::Active, None),
            member(6, ClanRank::Nix, MemberStatus::Trial, None),
        ];
        let v = build_roster(&ms);
        let order: Vec<_> = v.sections.iter().map(|s| s.rank).collect();
        assert_eq!(
            order,
            vec![ClanRank::Warden, ClanRank::Kerberos, ClanRank::Styx, ClanRank::Nix]
        );
        assert_eq!(v.active, 4);
        assert_eq!(v.trial, 2);
        assert_eq!(entry_line(&v.sections[2].entries[0]), "<@3> · Alpha");
        assert_eq!(entry_line(&v.sections[2].entries[1]), "<@5>");
    }

    #[test]
    fn removed_and_retired_are_hidden() {
        let ms = vec![
            member(1, ClanRank::Styx, MemberStatus::Removed, None),
            member(2, ClanRank::Styx, MemberStatus::Retired, None),
            member(3, ClanRank::Styx, MemberStatus::Inactive, None),
        ];
        let v = build_roster(&ms);
        assert_eq!(v.sections.len(), 1);
        assert_eq!(v.sections[0].entries.len(), 1);
        assert_eq!(v.inactive, 1);
        assert_eq!(v.active, 0);
    }

    #[test]
    fn empty_roster() {
        let v = build_roster(&[]);
        assert!(v.sections.is_empty());
        assert_eq!((v.active, v.trial), (0, 0));
    }

    #[test]
    fn chunking_respects_limits() {
        let lines: Vec<String> = (0..100).map(|i| format!("<@{:018}>", i)).collect();
        let (chunks, dropped) = chunk_lines(&lines, 1000, 200);
        assert_eq!(dropped, 0);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.chars().count() <= 1000));
        let rejoined: usize = chunks.iter().map(|c| c.lines().count()).sum();
        assert_eq!(rejoined, 100);

        let (chunks, dropped) = chunk_lines(&lines, 1000, 10);
        assert_eq!(dropped, 90);
        assert_eq!(chunks.iter().map(|c| c.lines().count()).sum::<usize>(), 10);
    }
}
