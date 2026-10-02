# HANDOFF — ϟ МРАК (STYX Rivals clan bot)

Plain-language guide. Paths are relative to the project root.

## WHAT WAS BUILT
A Rust Discord bot (Serenity 0.12, Tokio, SQLx/SQLite) with: application flow (panel → 2 modals → staff card → confirmed accept/reject/request-info), member registry, rank promotion/demotion with role sync, a self-updating roster message, moderation (warn/timeout/kick/ban/purge + optional automod), audit log, health/stats commands, and an optional X-API RIVALS intel feed. 80 unit tests + 13 integration tests; `cargo check`, `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` all pass.

## WHERE EVERYTHING IS
| Path | Contents |
|---|---|
| `src/main.rs` | startup, intents, feed worker spawn, graceful shutdown |
| `src/config.rs` | `Config::load()`, `.env` + `config.toml` parsing and validation |
| `src/error.rs` | `Error` enum, `Error::user_message()` |
| `src/state.rs` | `AppState` shared by all handlers |
| `src/db/mod.rs`, `src/db/repositories.rs` | connection, migrations, **all SQL**, `AuditEvent` |
| `migrations/0001_initial.sql` | schema |
| `src/applications/model.rs` | fields, validation (`validate_step_one/two`, `parse_win_rate`…), status state machine |
| `src/applications/service.rs` | submit / accept / reject / info (transactions) |
| `src/applications/handlers.rs` | modals, APPLY flow, review buttons |
| `src/members/model.rs` | `ClanRank`, `MemberStatus`, `check_rank_change` |
| `src/members/service.rs` | rank change, squad, update, remove, import |
| `src/members/roster.rs` | `build_roster`, `chunk_lines` |
| `src/moderation/service.rs` | `parse_duration`, `Automod` rule engine |
| `src/feeds/source.rs` | `FeedSource` trait, `XFeedSource` |
| `src/feeds/service.rs` | `poll_once`, `run_worker`, `is_relevant`, `backoff_delay` |
| `src/discord/commands.rs` | slash command definitions (`definitions()`) and handlers |
| `src/discord/interactions.rs` | button/modal router, confirmation executor |
| `src/discord/actions.rs` | Discord side effects, `PendingAction`, `execute`, `refresh_roster` |
| `src/discord/embeds.rs` | every embed/button layout and the colour constants |
| `src/discord/permissions.rs` | `Tier`, `Permission`, `allowed`, `can_act_on` |
| `src/discord/handler.rs` | gateway events, startup, automod message hook |
| `tests/` | integration tests |

## HOW TO CONFIGURE IT
1. `cp .env.example .env`, then edit `.env`.
2. Optional: `cp config.example.toml config.toml`.

**Where Discord IDs go** — all in `.env`:
- Server ID: `GUILD_ID`
- Role IDs: `ROLE_WARDEN`, `ROLE_KERBEROS`, `ROLE_CHARON`, `ROLE_STYX`, `ROLE_NIX`
- Channel IDs: `CHANNEL_APPLY`, `CHANNEL_ROSTER`, `CHANNEL_ANNOUNCEMENTS`, `CHANNEL_STAFF`, `CHANNEL_LOGS`
- Token: `DISCORD_TOKEN`

Other variables: `DATABASE_URL` (default `sqlite://data/styx.db`), `CHARON_CAN_DECIDE`, `AUTOMOD_ENABLED`, `FEED_ENABLED`, `FEED_INTERVAL_SECONDS`, `FEED_X_BEARER_TOKEN`, `FEED_X_USERNAME`, `RUST_LOG`.

## HOW TO START / STOP
- Start: `cargo run --release` (or `make run`). Systemd: `deploy/styx-mrak.service`.
- Stop: Ctrl-C or `SIGTERM`. The bot disconnects, stops the feed worker, closes SQLite.

## HOW TO RUN TESTS
`cargo test` · full gate: `make ci`.

## HOW TO RUN MIGRATIONS
Automatic at startup (`db::connect` → `db::migrate`, from `migrations/`). To add one, create `migrations/0002_<name>.sql` (never edit `0001`), then `cargo test`.

## BACKUP / RESTORE
Backup: `./scripts/backup.sh` (Windows: `.\scripts\backup.ps1`) → `backups/styx-<UTC>.db`.
Restore: stop the bot; `rm -f data/styx.db-wal data/styx.db-shm`; `cp backups/styx-<STAMP>.db data/styx.db`; start; `/health`; `/roster refresh`.

## WHAT TO CHANGE
**Add a new command.** (1) `src/discord/commands.rs` → add a `CreateCommand` in `definitions()` (use `staff(..)` for staff-only). (2) Add an arm in `dispatch()`. (3) Put logic in a service module, call `require(actor.tier, Permission::…, flag)?` first. (4) If it is destructive, add a variant to `PendingAction` in `src/discord/actions.rs` and handle it in `execute()`, then use `ask_confirm(..)`. (5) `cargo test` (the test `command_set_is_complete_and_within_discord_limits` guards limits; add the name to it).

**Add a new role/rank.** `src/members/model.rs` (`ClanRank` variant in hierarchy order, `as_str`, `label`, `FromStr`, `ALL`); `src/discord/permissions.rs` (`Tier`, `tier_of`, `role_for`, `all_rank_roles`, `clan_rank`); `src/config.rs` (`RoleConfig` field + `ROLE_*` variable + the duplicate check); `migrations/0002_*.sql` must widen the `clan_rank` CHECK (SQLite needs a table rebuild); `.env.example`. Update `check_rank_change` if the ceiling changes.

**Change application questions.** `src/applications/handlers.rs` → `build_application_step_one()` and `build_application_step_two()` (Discord limit: 5 inputs per modal, labels ≤ 45 chars; tests enforce this). Match the validation in `src/applications/model.rs` (`validate_step_one`, `validate_step_two`), the DB columns in a new migration, `NewApplication`, and the card in `embeds::application_review`. Then `cargo test && cargo check`.

**Change roster format.** `src/discord/embeds.rs` → `roster()` (layout), `src/members/roster.rs` → `entry_line()` (per-member line), `build_roster()` (grouping/counts).

**Change embed style.** `src/discord/embeds.rs`: `BLOOD_RED`, `COLD_WHITE`, `BLACK`, `FOOTER`, `RULE`, and each function (`apply_panel`, `application_review`, `intel_drop`, `dm_accepted`…).

**Change the feed source.** Implement `FeedSource` (`src/feeds/source.rs`: `name()`, `fetch_new_items(since_id)`), then construct it in `src/main.rs` instead of `XFeedSource`. Keyword filters: `config.toml` `[feed]`. Posting format: `embeds::intel_drop`. The service never needs to change.

## HOW TO DEBUG
**Permissions.** Run `RUST_LOG=debug cargo run`; rejected requests log `request rejected`. Authorisation = `RoleConfig::tier_of` over `ROLE_*` IDs → `allowed()`. A Discord user holding no configured role is `Guest`. Unit tests: `cargo test discord::permissions`.

**Discord API failures.** Replies say "Discord rejected part of that request" and the log has `discord error: …`. 403 / Missing Permissions = bot permission or role hierarchy (move the bot role above the clan roles). 404 on a message = canonical message deleted (roster/panel are recreated automatically). 10013/Unknown User = target left the server.

**Database failures.** `RUST_LOG=sqlx=debug`. Inspect: `sqlite3 data/styx.db`. "database is locked" = two processes. Audit trail: `select * from audit_events order by event_id desc limit 20;`.

## KNOWN LIMITATIONS (read this)
- **Not tested against live Discord.** I had no bot token or server. Everything that talks to Discord (gateway, slash registration, modals, buttons, role changes, DMs, kick/ban/timeout/purge, posting/editing embeds) compiles and its logic is unit/integration tested around the edges, but has never run against the real API. Run the 20-step checklist in `docs/DISCORD_SETUP.md` before trusting it. I did smoke-run the binary: with no config it exits with a clear error; with a fake token it creates the database, applies migrations and tries to connect to Discord.
- **X feed is unverified live.** `XFeedSource` uses the documented X API v2 endpoints (`/2/users/by/username/{u}`, `/2/users/{id}/tweets`), tested only against a fixture shaped like the documented response. It needs a bearer token with timeline access; X's access tiers and pricing change, so confirm yours covers it. Unofficial scraping was deliberately not used. I confirmed the studio uses the X handle `NosniyGames`; I did not verify any other source (e.g. a Roblox update feed), and the bot never labels posts "official" beyond naming the source account.
- `FeedSource::fetch_new_items` takes a `since_id` cursor (the spec's version had no argument, which cannot support deduplicated polling).
- Feed delivery is at-most-once: if the process dies between claiming and announcing a post, that post is skipped rather than risking a duplicate.
- Review cards go to the staff channel (not private threads) for reliability. Rejection reasons are private and never sent to the applicant.
- The spec's `/admin …` group is implemented as top-level staff commands (`/application`, `/member`, `/mod`, `/roster`, `/panel`, `/stats`, `/health`) as in the command list. There is no `/admin config` editor; configuration is `.env`/`config.toml`.
- Withdraw exists in `applications::service::withdraw` and the state machine, but has no command or button yet.
- `/member remove` does not kick from Discord. The bot never edits the Warden role.
- The bot cannot list guild members (no Server Members intent), so existing members must be registered once with `/member add`.
- Staff command visibility uses the Moderate Members default permission; adjust under Server Settings → Integrations if your staff roles lack it.
- Automod needs the Message Content privileged intent.
- A release build and live run were not performed on a production host.

## SECURITY NOTES
See `docs/SECURITY.md`. Key points: token only in `.env`; roles come from IDs not names; confirmations are owner-bound, single-use and expire after 10 minutes; moderation is hierarchy-checked; backups hold private data.
