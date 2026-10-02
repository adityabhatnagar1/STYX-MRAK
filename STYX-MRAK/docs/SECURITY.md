# Security

**Secrets.** `DISCORD_TOKEN` and `FEED_X_BEARER_TOKEN` live only in `.env` (git-ignored). `Config`'s `Debug` output redacts the token and database URL, and a test enforces it. Reset the token in the Developer Portal if it is ever exposed.

**Permissions.** Roles are mapped from IDs in `.env`, never from role names or user input. Kerberos/Warden gate registry changes, moderation and confirmed decisions; Charon is review-only unless `CHARON_CAN_DECIDE=true`. Authorisation is re-checked when a confirmation is executed. Moderation targets must be strictly lower in rank; self-targeting is refused. Staff commands are also hidden from members by default permissions (cosmetic only).

**Input.** Every modal field is validated server-side (length, charset, numeric bounds). Free text is rejected if it looks like a password, cookie or 2FA code. SQL uses bound parameters only. Bot messages that mention applicants restrict allowed mentions, and application text is shown inside embeds, so it cannot ping `@everyone`.

**Threat model.** Protects against: curious members using staff features, double-click / replayed confirmations, concurrent staff decisions, malformed input, and duplicate feed posts. Does *not* protect against: a compromised Discord admin account, a leaked bot token, or someone with filesystem access to `data/styx.db` (stored unencrypted).

**Backups.** Contain applicant data (Discord IDs, Roblox names, free-text answers). Keep them out of Git, readable only by the bot's user, and delete old ones (`BACKUP_KEEP`).

**Retention.** Applications, audit events and removed-member records are kept indefinitely so decisions stay auditable. To erase a person, delete their rows from `applications`, `members`, `audit_events` with `sqlite3` while the bot is stopped.

**Logging.** Logs never contain the token or DB credentials and do not print application answers.
