#![allow(dead_code)]
use styx_mrak::applications::model::{NewApplication, StepOne, StepTwo};

pub fn application(user: i64) -> NewApplication {
    NewApplication {
        discord_user_id: user,
        discord_username: format!("user{user}"),
        one: StepOne {
            roblox_username: "Cool_Guy1".into(),
            player_level: 312,
            current_rank: "Diamond".into(),
            elo: 1840,
            win_rate: 71.0,
        },
        two: StepTwo {
            preferred_loadout: "Shotgun / Uzi / Knife / Flash".into(),
            playstyle: "Aggressive entry".into(),
            timezone: "IST".into(),
            motivation: "I want to play with a real squad.".into(),
        },
    }
}
