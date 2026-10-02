# Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Slash commands missing | Registration failed, or hidden by permissions | Check startup log for `slash commands registered`. Bot must be installed with `applications.commands`. Staff commands need *Moderate Members* or an Integrations override (docs/DISCORD_SETUP.md). |
| Bot offline | Bad token, network, or disallowed intents | Run it in the foreground and read the error. `Disallowed intent` → enable Message Content Intent or set `AUTOMOD_ENABLED=false`. |
| `ROLE_X is required` at startup | Missing/invalid `.env` value | Compare with `.env.example`; IDs are numbers and the five roles must differ. |
| "Discord rejected part of that request" | Missing permission or role hierarchy | Give the bot Manage Roles / Kick / Ban / Moderate Members, and move its role above the clan roles. |
| Accepted but no role | Bot role below Nix, or applicant left the server | Fix hierarchy; assign Nix manually (the reply lists this as a warning). |
| Roster not updating | Bot cannot edit/send in `CHANNEL_ROSTER` | Fix channel permissions, then `/roster refresh`. Logs show `failed to update roster`. |
| DMs failing | User has DMs closed | Expected. Staff are told; info requests fall back to a prompt in the apply channel. |
| `database is locked` | Two bot processes on one file | Run one instance. Check `ps aux | grep styx-mrak`. |
| Database error at start | Corrupt/missing file or unwritable `data/` | Check permissions on `data/`; restore a backup (docs/OPERATIONS.md). |
| Feed `DEGRADED` | X API error, bad/expired bearer token, or no API access | `/health` shows consecutive failures; run with `RUST_LOG=debug`. HTTP 401/403 = token/plan problem. |
| Feed posted nothing | Posts filtered out | Check `include_keywords`/`exclude_keywords` in `config.toml`; filtered posts are stored in `feed_items` with status `filtered`. |
| "This confirmation has expired" | Pressed after 10 minutes or twice | Re-run the command. |

Inspect the database: `sqlite3 data/styx.db 'select * from audit_events order by event_id desc limit 20;'`
