#!/usr/bin/env bash
# Smoke test for team item API endpoints.
# Requires: curl, jq
#
# Usage:
#   set -a; source .env; set +a; ./scripts/smoke-test.sh
#
# The server must be running in internal auth mode (TODO_AUTH_MODE=internal),
# which validates the Bearer JWT directly. The caddy-fronted live server uses
# session cookies and will redirect this script to the auth portal.
#
# To test against a local instance:
#   TODO_API_URL=http://localhost:3000 TODO_API_TOKEN=<jwt> ./scripts/smoke-test.sh
#
# Get a token from a running internal-mode server:
#   Sign in via browser → visit /auth/token → copy the token value.
#
# The script creates a team, creates/gets/updates/deletes a team item, then
# cleans up. It exits non-zero if any assertion fails.

set -euo pipefail

API_URL="${TODO_API_URL:-http://localhost:3000}"
TOKEN="${TODO_API_TOKEN:-}"

if [[ -z "$TOKEN" ]]; then
  echo "ERROR: TODO_API_TOKEN is not set" >&2
  exit 1
fi

# Extract user ID from JWT payload (base64url-encoded middle segment).
JWT_PAYLOAD=$(echo "$TOKEN" | cut -d'.' -f2 | tr '_-' '/+' | base64 -d 2>/dev/null)
USER_ID=$(echo "$JWT_PAYLOAD" | jq -r '.sub')
if [[ -z "$USER_ID" || "$USER_ID" == "null" ]]; then
  echo "ERROR: could not extract user ID from token" >&2
  exit 1
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; exit 1; }

api() {
  local method="$1"; shift
  curl -sf -X "$method" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    "$@"
}

echo "=== Team item smoke test against $API_URL (user: $USER_ID) ==="
echo

# ── Create a team ─────────────────────────────────────────────────────────────
TEAM=$(api POST "$API_URL/api/users/$USER_ID/teams" -d '{"name":"smoke-test-team"}')
TEAM_ID=$(echo "$TEAM" | jq -r '.teamId')
[[ -n "$TEAM_ID" && "$TEAM_ID" != "null" ]] || fail "create team — no teamId in response"
pass "create team ($TEAM_ID)"

cleanup() {
  # Best-effort: delete the item if it still exists, then leave the team.
  if [[ -n "${ITEM_ID:-}" ]]; then
    api DELETE "$API_URL/api/teams/$TEAM_ID/items/$ITEM_ID" > /dev/null 2>&1 || true
  fi
  api DELETE "$API_URL/api/users/$USER_ID/teams/$TEAM_ID/membership" > /dev/null 2>&1 || true
}
trap cleanup EXIT

# ── Create a team item ────────────────────────────────────────────────────────
CREATE=$(api POST "$API_URL/api/teams/$TEAM_ID/items" \
  -d '{"name":"smoke-test item","complete":false}')
ITEM_ID=$(echo "$CREATE" | jq -r '.itemId')
[[ -n "$ITEM_ID" && "$ITEM_ID" != "null" ]] || fail "create team item — no itemId in response"
pass "create team item ($ITEM_ID)"

# ── Get the team item ─────────────────────────────────────────────────────────
GET=$(api GET "$API_URL/api/teams/$TEAM_ID/items/$ITEM_ID")
GOT_NAME=$(echo "$GET" | jq -r '.name')
[[ "$GOT_NAME" == "smoke-test item" ]] || fail "get team item — expected name 'smoke-test item', got '$GOT_NAME'"
GOT_COMPLETE=$(echo "$GET" | jq -r '.complete')
[[ "$GOT_COMPLETE" == "false" ]] || fail "get team item — expected complete=false, got '$GOT_COMPLETE'"
pass "get team item"

# ── List team items ───────────────────────────────────────────────────────────
LIST=$(api GET "$API_URL/api/teams/$TEAM_ID/items")
COUNT=$(echo "$LIST" | jq '.items | length')
[[ "$COUNT" -ge 1 ]] || fail "list team items — expected ≥1 item, got $COUNT"
FOUND=$(echo "$LIST" | jq -r --arg id "$ITEM_ID" '.items[] | select(.itemId == $id) | .itemId')
[[ "$FOUND" == "$ITEM_ID" ]] || fail "list team items — created item not in list"
pass "list team items ($COUNT item(s))"

# ── Update the team item ──────────────────────────────────────────────────────
api PUT "$API_URL/api/teams/$TEAM_ID/items/$ITEM_ID" \
  -d '{"name":"smoke-test item (updated)","complete":false}' > /dev/null
UPDATED=$(api GET "$API_URL/api/teams/$TEAM_ID/items/$ITEM_ID")
UPDATED_NAME=$(echo "$UPDATED" | jq -r '.name')
[[ "$UPDATED_NAME" == "smoke-test item (updated)" ]] || \
  fail "update team item — expected updated name, got '$UPDATED_NAME'"
pass "update team item"

# ── Delete the team item ──────────────────────────────────────────────────────
api DELETE "$API_URL/api/teams/$TEAM_ID/items/$ITEM_ID" > /dev/null
# Verify it's gone — expect a non-2xx response
HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
  -H "Authorization: Bearer $TOKEN" \
  "$API_URL/api/teams/$TEAM_ID/items/$ITEM_ID")
[[ "$HTTP_STATUS" != "200" ]] || fail "delete team item — item still reachable after delete (HTTP 200)"
pass "delete team item (confirmed gone, HTTP $HTTP_STATUS)"
ITEM_ID=""  # prevent cleanup from trying to delete again

echo
echo "=== All checks passed ==="
