# STYX go-live checklist

Tick each step. Every step is explicit.

1. **Create the Discord application** at https://discord.com/developers/applications (New Application → `МРАК`).
2. **Create the bot** (Bot tab → Add Bot if shown).
3. **Copy the bot token** (Bot → Reset Token) into `.env` → `DISCORD_TOKEN=`.
4. **Enable only required intents**: all privileged intents OFF; Message Content ON only if `AUTOMOD_ENABLED=true`.
5. **Install the bot into STYX Rivals** with scopes `bot` + `applications.commands` and the permissions in docs/SETUP.md. Move the bot's role above Nix/Styx/Charon/Kerberos.
6. **Copy the guild ID** → `.env` → `GUILD_ID=`.
7. **Copy the role IDs** → `ROLE_WARDEN`, `ROLE_KERBEROS`, `ROLE_CHARON`, `ROLE_STYX`, `ROLE_NIX` (all five must differ).
8. **Copy the channel IDs** → `CHANNEL_APPLY` (APPLY panel), `CHANNEL_ROSTER`, `CHANNEL_ANNOUNCEMENTS` (intel feed), `CHANNEL_STAFF` (review cards), `CHANNEL_LOGS` (audit mirror). The bot needs Send Messages + Embed Links in each; make `CHANNEL_STAFF` and `CHANNEL_LOGS` staff-only.
9. **Configure `.env`** (and optionally `config.toml`).
10. **Run migrations**: they run automatically when the bot starts; to run them without Discord, `cargo test` exercises them against a temp database.
11. **Start the bot**: `cargo run --release`. Expect log lines `connected to Discord`, `slash commands registered`, `roster updated`, and an "ϟ МРАК ONLINE" embed in the logs channel.
12. **Run `/health`** (staff). Bot, Database and Discord must say OK.
13. **Post the application panel**: it is posted automatically at startup. To repost/refresh: `/panel apply`.
14. **Configure the roster message**: `/member add` each existing member, then `/roster refresh`. The roster message is created once and edited from then on.
15. **Test an application** with a non-staff test account: press APPLY, finish both modals, confirm a card appears in the staff channel.
16. **Test acceptance**: press ACCEPT on the card → CONFIRM ACCEPT. Check the applicant got the Nix role, a DM, a roster entry, and a log line.
17. **Test rejection**: submit another application, REJECT → optional reason → CONFIRM REJECTION. Check the DM and that the record is kept (`/application view id:N`).
18. **Test moderation** on a throwaway account: `/mod warn`, `/mod timeout duration:1m`, `/mod purge amount:2`. Check the logs channel for each.
19. **Test the roster update**: `/member promote user:@test rank:styx` → roster message changes in place (no new message).
20. **Test the feed**: set `FEED_ENABLED=true`, `FEED_X_BEARER_TOKEN`, restart. The first run records existing posts silently; the next matching RIVALS post is announced once.

**Staff command visibility:** staff commands are registered with the *Moderate Members* default permission so ordinary members do not see them. If your Kerberos/Charon roles lack that permission, grant it, or open Server Settings → Integrations → МРАК → Commands and allow the staff roles. Hiding is cosmetic; every handler also checks roles from `.env`.
