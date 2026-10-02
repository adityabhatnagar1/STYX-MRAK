# Architecture

```
Discord gateway ──► discord/handler.rs ──► discord/commands.rs      (slash commands)
                                      └──► discord/interactions.rs  (buttons, modals, confirmations)
                                                │
                              applications/handlers.rs (modals, review buttons)
                                                │
                  ┌─────────────────────────────┼────────────────────────┐
         applications/service.rs        members/service.rs        moderation/service.rs
                  └──────────────► db/repositories.rs (all SQL) ◄───────┘
                                                │
                                          SQLite (WAL)
Discord side effects (roles, DMs, roster, logs) live in discord/actions.rs
Feed worker: feeds/service.rs ──► feeds/source.rs (X API v2) ──► discord/actions.rs::DiscordAnnouncer
```

## Principles
- **Database is the source of truth.** Discord roles and the roster message are projections of it.
- **Services are Discord-free.** `applications/service.rs`, `members/service.rs`, `members/roster.rs`, `moderation/service.rs`, `feeds/service.rs` take a pool and plain data, so they are unit-testable. Each state change is one transaction that also writes the audit row.
- **Order of effects:** authorise → database transaction → best-effort Discord effects (role, DM, card update, roster, log). Discord failures are logged and reported to the staff member; they never roll back the decision and never panic.
- **Authorisation** is computed from configured role IDs (`RoleConfig::tier_of`) and checked in every handler *and again* when a confirmation is executed (`actions::execute`).
- **Stateless UI.** Buttons carry IDs in `custom_id` (`app:accept:12`, `act:ok:7`). Confirmations live in the `pending_actions` table (10-minute expiry, owner-bound, single-use), so everything survives restarts.

## Database (`migrations/0001_initial.sql`)
`applications`, `application_drafts`, `members`, `audit_events`, `feed_items`, `config` (canonical message IDs, feed cursor), `pending_actions`. A partial unique index enforces one open application per user. Unique indexes also enforce one member row per Discord user and one feed row per source post.

## Background work
Only the feed worker (`feeds::service::run_worker`), stopped through a `watch` channel at shutdown. Automod state is in memory by design (it is a rolling window).

## Application state machine
`pending → under_review → accepted | rejected | withdrawn`; `pending → accepted | rejected | withdrawn`. Terminal states have no exits (`ApplicationStatus::can_transition_to`). The SQL update also guards the status so two staff clicking at once cannot both win.
