# ϟ МРАК — STYX Rivals clan bot

A private Discord bot for the STYX Rivals clan (Roblox **RIVALS**). One Rust
binary, one SQLite file.

**What it does**

- **Applications** — a permanent APPLY panel → two modals → a review card for staff with Review / Accept / Reject / Request Info buttons. Accept and reject always ask for confirmation.
- **Member registry** — the database is the source of truth: Roblox name, level, rank, ELO, win rate, loadout, playstyle, squad, status, clan rank.
- **Roles & promotions** — Nix → Styx → Charon → Kerberos, with Discord role sync and hierarchy rules.
- **Roster** — one canonical, auto-edited roster message generated from the registry.
- **Moderation** — warn, timeout, kick, ban, purge (confirmed, hierarchy-checked, audited) plus optional conservative automod.
- **Audit log** — every state change is stored in the database and mirrored to the logs channel.
- **Intel feed** — optional poller that announces new RIVALS posts from the studio's X account, deduplicated and filtered.

## Requirements

- Rust 1.85+ (developed on 1.89), a C compiler (SQLite is bundled), internet access for crates.
- A Discord application with a bot token (see [docs/DISCORD_SETUP.md](docs/DISCORD_SETUP.md)).

## Quick start

```bash
cp .env.example .env            # fill in token, guild, role and channel IDs
cp config.example.toml config.toml   # optional tunables
cargo test
cargo run                       # creates data/styx.db and applies migrations
```

Then follow the 20-step checklist in [docs/DISCORD_SETUP.md](docs/DISCORD_SETUP.md).

## Documentation

| File | Purpose |
|---|---|
| [HANDOFF.md](HANDOFF.md) | **Start here.** Exact files, symbols and commands for every change. |
| [docs/SETUP.md](docs/SETUP.md) | Developer Portal, intents, permissions, `.env`, run |
| [docs/DISCORD_SETUP.md](docs/DISCORD_SETUP.md) | STYX-specific go-live checklist |
| [docs/OPERATIONS.md](docs/OPERATIONS.md) | Day-to-day staff and admin operations, backups |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Modules, data flow, database |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | Symptom → cause → fix |
| [docs/SECURITY.md](docs/SECURITY.md) | Secrets, permissions, threat model |

## Development

```bash
make ci      # cargo check, test, fmt --check, clippy -D warnings
make run
make backup
```

Nothing in development requires Docker.
