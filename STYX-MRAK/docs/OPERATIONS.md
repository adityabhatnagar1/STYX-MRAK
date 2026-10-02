# Operations

## Applications
- Applicants press **APPLY** (panel) or run `/apply`. One open application per person; current members cannot apply.
- Review card (staff channel): **REVIEW** (mark under review) · **ACCEPT** · **REJECT** (optional private reason) · **REQUEST INFO**.
- Accept/reject need Kerberos or Warden (Charon too if `CHARON_CAN_DECIDE=true`). Charon and above can review and request info.
- Accept → member becomes **Nix / trial**, gets the Nix role and a DM. Reject → record kept, DM sent. The rejection reason is private and is never sent to the applicant.
- Request info → DM with a REPLY button. If DMs are closed, a prompt (no private details) is posted in the apply channel. Replies appear in the staff channel.
- Commands: `/application list [status]`, `view id`, `accept id`, `reject id [reason]`, `request-info id question`.

## Members and promotions
- `/member view|update|promote|demote|squad|remove|add`.
- Rank rules: Kerberos and Warden change Nix/Styx/Charon; only the Warden grants or removes Kerberos; nobody changes Warden through the bot.
- `/member remove` marks the member removed (record kept) and strips clan roles; it does not kick them from the server.
- Staff notes are only visible to Kerberos and above.

## Roster
One message in `CHANNEL_ROSTER`, edited in place after every accept/promote/demote/remove/squad change. If it drifts, run `/roster refresh`. If someone deletes it, the next refresh recreates it.

## Moderation
`/mod warn|timeout|kick|ban|purge`. Kick, ban, warn and purge ask for confirmation. You cannot act on yourself or on anyone of equal or higher rank. Every action is audited and logged. Automod (`AUTOMOD_ENABLED=true`) deletes violating messages and only times a user out after repeated violations (see `[automod]` in `config.toml`); Kerberos and Warden are exempt.

## Feed
Polls the configured X account every `FEED_INTERVAL_SECONDS` (min 30), keeps posts that contain an include keyword and no exclude keyword, and posts an "INTEL DROP" once per post. First run records the existing backlog silently. On errors it backs off exponentially up to one hour. `/health` shows feed state.

## Backups
```bash
./scripts/backup.sh          # Linux/macOS; writes backups/styx-<UTC>.db, keeps newest 30
.\scripts\backup.ps1         # Windows
```
Uses SQLite's online backup when `sqlite3` is installed. Without it, stop the bot and run `./scripts/backup.sh --offline`. Schedule daily, e.g. cron: `0 4 * * * cd /opt/styx-mrak && ./scripts/backup.sh`. Backups contain private applicant data: keep them off Git and restrict access.

### Restore
1. Stop the bot.
2. `mv data/styx.db data/styx.db.broken; rm -f data/styx.db-wal data/styx.db-shm`
3. `cp backups/styx-<STAMP>.db data/styx.db`
4. Start the bot, run `/health`, then `/roster refresh`.

## Start / stop
`cargo run --release` (foreground) or the systemd unit in `deploy/styx-mrak.service`. Stop with Ctrl-C or `systemctl stop styx-mrak` (SIGTERM → graceful shutdown).
