# Setup

## 1. Discord Developer Portal

1. https://discord.com/developers/applications → **New Application** → name it `МРАК`.
2. **Bot** tab → **Reset Token** → copy it into `.env` as `DISCORD_TOKEN`. Anyone with this token controls the bot; never paste it in chat or commit it.
3. **Privileged Gateway Intents** (Bot tab):
   - Server Members Intent: **OFF**
   - Presence Intent: **OFF**
   - Message Content Intent: **OFF**, unless you set `AUTOMOD_ENABLED=true` — then turn it **ON** (automod must read message text). If automod is enabled and this intent is off, Discord closes the connection with an "disallowed intents" error.
4. **Installation / OAuth2 → URL Generator**: scopes `bot` and `applications.commands`.
5. Bot permissions (the minimum the bot uses):

| Permission | Used for |
|---|---|
| View Channels, Send Messages, Embed Links, Read Message History | posting panels, cards, roster, logs, feed |
| Manage Roles | granting/removing Nix, Styx, Charon, Kerberos |
| Moderate Members | timeouts |
| Kick Members, Ban Members | `/mod kick`, `/mod ban` |
| Manage Messages | `/mod purge`, automod deletes |

6. Open the generated URL and install the bot into STYX Rivals.
7. **Role order (important):** Server Settings → Roles → drag the bot's role **above** Nix, Styx, Charon and Kerberos. Discord refuses role changes on roles at or above the bot's own.

## 2. Collect IDs

Discord → User Settings → Advanced → **Developer Mode** ON. Then right-click the server icon / each role / each channel → **Copy ID**. Where each goes: `.env` (see `.env.example`).

## 3. Configure and run

```bash
cp .env.example .env     # edit
cp config.example.toml config.toml   # optional
cargo run --release      # migrations run automatically at startup
```

Stop with Ctrl-C or `SIGTERM`; the bot disconnects, stops the feed worker and closes the database.

## 4. First-run bootstrap

The registry starts empty. For people who already hold clan roles, run `/member add user:@person` (staff). The rank comes from the roles the person holds in Discord, so `/member add` can only record what Discord already says. Do this for the Warden and every current member, then `/roster refresh`.
