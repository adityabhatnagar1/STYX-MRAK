#!/usr/bin/env bash
# Back up the SQLite database. Safe to run while the bot is running when the
# `sqlite3` CLI is installed (uses SQLite's online backup). Without it the
# script refuses unless you pass --offline (stop the bot first).
set -euo pipefail

DB="${DB_PATH:-data/styx.db}"
OUT_DIR="${BACKUP_DIR:-backups}"
KEEP="${BACKUP_KEEP:-30}"

[ -f "$DB" ] || { echo "database not found: $DB" >&2; exit 1; }
mkdir -p "$OUT_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DEST="$OUT_DIR/styx-$STAMP.db"

if command -v sqlite3 >/dev/null 2>&1; then
  sqlite3 "$DB" ".backup '$DEST'"
elif [ "${1:-}" = "--offline" ]; then
  cp "$DB" "$DEST"
  [ -f "$DB-wal" ] && cp "$DB-wal" "$DEST-wal"
else
  echo "sqlite3 is not installed. Install it, or stop the bot and re-run with --offline." >&2
  exit 2
fi

chmod 600 "$DEST" 2>/dev/null || true
echo "backup written: $DEST"

# Keep only the newest $KEEP backups.
ls -1t "$OUT_DIR"/styx-*.db 2>/dev/null | tail -n +"$((KEEP + 1))" | xargs -r rm -f
