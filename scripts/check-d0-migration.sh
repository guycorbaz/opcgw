#!/usr/bin/env bash
# check-d0-migration.sh — Post-D-0 singleton-config migration verification tool.
#
# Usage: bash scripts/check-d0-migration.sh [opcgw.db]
#
# Queries the SQLite database at the given path (default: opcgw.db in the
# current directory) and prints a green/red summary of the D-0 singleton-
# config migration status.
#
# Exit codes:
#   0  — migration completed and counts look valid
#   1  — migration not yet run, or counts are zero
#   2  — sqlite3 not found or DB not readable

set -euo pipefail

DB="${1:-opcgw.db}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Colour

fail() { echo -e "${RED}FAIL${NC}: $1"; exit "${2:-1}"; }
pass() { echo -e "${GREEN}PASS${NC}: $1"; }
info() { echo -e "${YELLOW}INFO${NC}: $1"; }

# ------------------------------------------------------------------
# 0. Prerequisites
# ------------------------------------------------------------------
if ! command -v sqlite3 >/dev/null 2>&1; then
    fail "sqlite3 not found. Install the sqlite3 CLI to run this check." 2
fi

if [[ ! -r "$DB" ]]; then
    fail "Cannot read database at '$DB'. Pass the correct path as the first argument." 2
fi

echo "Checking D-0 singleton-config migration status for: $DB"
echo "------------------------------------------------------"

# ------------------------------------------------------------------
# 1. Schema version
# ------------------------------------------------------------------
SCHEMA_VER=$(sqlite3 "$DB" "PRAGMA user_version;" 2>/dev/null || echo "0")

if [[ "$SCHEMA_VER" -ge 10 ]] 2>/dev/null; then
    pass "Schema version: $SCHEMA_VER (>= 10, D-0 schema applied)"
else
    fail "Schema version is '$SCHEMA_VER' — migration schema v010 has not been applied. Upgrade the binary and restart opcgw." 1
fi

# ------------------------------------------------------------------
# 2. Migration done-flag
# ------------------------------------------------------------------
# Written by migrate_singleton_toml_to_sqlite() after the migration commits.
# Key is 'd0_migration_done'; value is the ISO-8601 timestamp.
MIG_TS=$(sqlite3 "$DB" "SELECT value FROM meta WHERE key='d0_migration_done';" 2>/dev/null || echo "")
if [[ -n "$MIG_TS" ]]; then
    pass "D-0 migration done-flag: $MIG_TS"
else
    info "No D-0 migration done-flag found. Migration may not have run yet, or the gateway was started before D-0 ever wrote to SQLite (placeholder-secrets path)."
fi

# ------------------------------------------------------------------
# 3. Per-section row counts
# ------------------------------------------------------------------
TOTAL=$(sqlite3 "$DB" "SELECT COUNT(*) FROM singleton_config;" 2>/dev/null || echo "-1")
GLOBAL_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM singleton_config WHERE section='global';" 2>/dev/null || echo "-1")
CHIRP_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM singleton_config WHERE section='chirpstack';" 2>/dev/null || echo "-1")
OPCUA_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM singleton_config WHERE section='opcua';" 2>/dev/null || echo "-1")
WEB_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM singleton_config WHERE section='web';" 2>/dev/null || echo "-1")

echo ""
echo "Per-section row counts:"
echo "  global     : $GLOBAL_COUNT"
echo "  chirpstack : $CHIRP_COUNT"
echo "  opcua      : $OPCUA_COUNT"
echo "  web        : $WEB_COUNT"
echo "  TOTAL      : $TOTAL"
echo ""

if [[ "$TOTAL" -lt 0 ]]; then
    fail "Could not query singleton_config — schema v010 may not have been applied." 1
fi

if [[ "$TOTAL" -eq 0 && -z "$MIG_TS" ]]; then
    info "singleton_config is empty and no done-flag. The gateway has likely not started yet on a post-D-0 binary, or it started in placeholder-secrets mode (operator hasn't supplied [chirpstack].api_token or [opcua].user_password yet)."
    exit 0
fi

if [[ "$TOTAL" -eq 0 && -n "$MIG_TS" ]]; then
    info "Migration done-flag is set ($MIG_TS) but singleton_config is empty — operator may have cleared the singleton tables manually (unusual; expected state has rows)."
    pass "D-0 migration: done-flag present, singleton tables intentionally empty."
    exit 0
fi

# ------------------------------------------------------------------
# 4. SQLite file permissions (AI-C-SEC-2)
# ------------------------------------------------------------------
if command -v stat >/dev/null 2>&1; then
    # GNU stat (Linux) vs BSD stat (macOS) differ on `-c` flag.
    MODE=$(stat -c '%a' "$DB" 2>/dev/null || stat -f '%Lp' "$DB" 2>/dev/null || echo "")
    if [[ -n "$MODE" ]]; then
        if [[ "$MODE" == "600" ]]; then
            pass "SQLite file mode: 0o$MODE (secure)"
        else
            info "SQLite file mode is 0o$MODE — wider than 0o600. Consider running 'chmod 0600 $DB' to restrict access to the gateway user."
        fi
    fi
fi

# ------------------------------------------------------------------
# 5. Sample rows
# ------------------------------------------------------------------
echo "Sample rows (first 5 per section):"
for SECTION in global chirpstack opcua web; do
    echo "  [$SECTION]:"
    sqlite3 "$DB" "SELECT key, value FROM singleton_config WHERE section='$SECTION' LIMIT 5;" 2>/dev/null \
        | sed 's/^/    /'
done
echo ""

# ------------------------------------------------------------------
# 6. Summary
# ------------------------------------------------------------------
pass "D-0 singleton-config migration check complete. $TOTAL row(s) across 4 sections."
exit 0
