-- ϟ МРАК initial schema. All timestamps are UTC, RFC 3339 text.
-- Discord snowflakes are stored as INTEGER (they always fit in i64).

CREATE TABLE applications (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    discord_user_id    INTEGER NOT NULL,
    discord_username   TEXT    NOT NULL,
    roblox_username    TEXT    NOT NULL,
    player_level       INTEGER NOT NULL,
    current_rank       TEXT    NOT NULL,
    elo                INTEGER NOT NULL,
    win_rate           REAL    NOT NULL,
    preferred_loadout  TEXT    NOT NULL,
    playstyle          TEXT    NOT NULL,
    timezone           TEXT    NOT NULL,
    motivation         TEXT    NOT NULL,
    status             TEXT    NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','under_review','accepted','rejected','withdrawn')),
    submitted_at       TEXT    NOT NULL,
    reviewed_at        TEXT,
    reviewed_by        INTEGER,
    review_notes       TEXT,
    info_request_text  TEXT,
    review_channel_id  INTEGER,
    review_message_id  INTEGER
);
CREATE INDEX idx_applications_user   ON applications (discord_user_id);
CREATE INDEX idx_applications_status ON applications (status);
-- One open application per Discord user, enforced by the database.
CREATE UNIQUE INDEX ux_applications_open
    ON applications (discord_user_id) WHERE status IN ('pending','under_review');

CREATE TABLE application_drafts (
    discord_user_id INTEGER PRIMARY KEY,
    payload         TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE TABLE members (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    discord_user_id   INTEGER NOT NULL,
    discord_username  TEXT    NOT NULL,
    roblox_username   TEXT,
    status            TEXT    NOT NULL DEFAULT 'trial'
        CHECK (status IN ('trial','active','inactive','retired','removed')),
    clan_rank         TEXT    NOT NULL DEFAULT 'nix'
        CHECK (clan_rank IN ('nix','styx','charon','kerberos','warden')),
    squad             TEXT,
    player_level      INTEGER,
    current_rank      TEXT,
    elo               INTEGER,
    win_rate          REAL,
    preferred_loadout TEXT,
    playstyle         TEXT,
    joined_at         TEXT    NOT NULL,
    last_updated_at   TEXT    NOT NULL,
    notes             TEXT
);
CREATE UNIQUE INDEX ux_members_discord_user ON members (discord_user_id);
CREATE INDEX idx_members_roblox ON members (roblox_username);
CREATE INDEX idx_members_status ON members (status);

CREATE TABLE audit_events (
    event_id         INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_discord_id  INTEGER,
    target_discord_id INTEGER,
    event_type       TEXT NOT NULL,
    metadata         TEXT NOT NULL DEFAULT '{}',
    timestamp        TEXT NOT NULL
);
CREATE INDEX idx_audit_timestamp ON audit_events (timestamp);
CREATE INDEX idx_audit_target    ON audit_events (target_discord_id);

CREATE TABLE feed_items (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    source       TEXT NOT NULL,
    source_id    TEXT NOT NULL,
    status       TEXT NOT NULL
        CHECK (status IN ('baseline','filtered','claimed','announced','failed')),
    attempts     INTEGER NOT NULL DEFAULT 0,
    text         TEXT NOT NULL DEFAULT '',
    discord_message_id INTEGER,
    first_seen_at TEXT NOT NULL,
    announced_at  TEXT
);
CREATE UNIQUE INDEX ux_feed_source_item ON feed_items (source, source_id);
CREATE INDEX idx_feed_source_id ON feed_items (source_id);

CREATE TABLE config (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Confirmation state for destructive actions. Lives in the database so
-- confirm/cancel buttons survive restarts.
CREATE TABLE pending_actions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_discord_id INTEGER NOT NULL,
    payload          TEXT    NOT NULL,
    created_at       TEXT    NOT NULL,
    expires_at       TEXT    NOT NULL,
    consumed_at      TEXT
);
