#!/usr/bin/env bash
# Throwaway Matrix homeserver for driving the real web client in a browser.
# See context/live-testing.md for the whole loop; this only owns the server.
#
#   homeserver.sh up       start Conduit in Docker and wait until it answers
#   homeserver.sh seed     register alice + bob, create a room, write sessions
#   homeserver.sh down     remove the container (all data goes with it)
#   homeserver.sh env      print the variables the other scripts read
#
# Nothing is bind-mounted, so the Windows-path mount rule does not apply and
# `down` leaves nothing behind.
set -euo pipefail

NAME=${PRINNY_HS_CONTAINER:-prinny-live-conduit}
PORT=${PRINNY_HS_PORT:-8228}
DIR=${PRINNY_LIVE_DIR:-/tmp/prinny-live-test}
PASSWORD=livetest-pass

# Inside a container, a published port is reached through Docker Desktop's
# host alias, not localhost. The browser runs next to this script, so it uses
# the same URL.
if [ -z "${PRINNY_HS_URL:-}" ]; then
  if [ -f /.dockerenv ]; then
    PRINNY_HS_URL="http://host.docker.internal:${PORT}"
  else
    PRINNY_HS_URL="http://127.0.0.1:${PORT}"
  fi
fi
HS=$PRINNY_HS_URL

need() {
  command -v "$1" >/dev/null || { echo "missing: $1" >&2; exit 1; }
}

wait_ready() {
  for _ in $(seq 1 60); do
    if curl -sf --max-time 2 "$HS/_matrix/client/versions" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  echo "homeserver did not answer at $HS — docker logs $NAME" >&2
  exit 1
}

# Transaction ids are unique per call. Conduit deduplicates them per device
# across ALL rooms, so a fixed id silently drops the second room's message.
txn() {
  echo "lt$(date +%s%N)$RANDOM"
}

register() {
  local user=$1 session
  session=$(curl -s -X POST "$HS/_matrix/client/v3/register" \
    -H 'Content-Type: application/json' -d '{}' | jq -r .session)
  curl -s -X POST "$HS/_matrix/client/v3/register" -H 'Content-Type: application/json' \
    -d "{\"username\":\"$user\",\"password\":\"$PASSWORD\",\"auth\":{\"type\":\"m.login.dummy\",\"session\":\"$session\"}}" \
    >/dev/null
}

# A fresh device per session file: the browser's session must not share a
# device with the curl calls, or its sync and the scripts' sends collide.
login() {
  local user=$1
  curl -s -X POST "$HS/_matrix/client/v3/login" -H 'Content-Type: application/json' \
    -d "{\"type\":\"m.login.password\",\"identifier\":{\"type\":\"m.id.user\",\"user\":\"$user\"},\"password\":\"$PASSWORD\",\"initial_device_display_name\":\"live-test\"}" |
    jq --arg hs "$HS" '{user_id, access_token, device_id, hs_base_url: $hs}'
}

cmd_up() {
  need docker
  need curl
  if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
    docker start "$NAME" >/dev/null
  else
    docker run -d --name "$NAME" -p "127.0.0.1:${PORT}:6167" \
      -e CONDUIT_SERVER_NAME=localhost \
      -e CONDUIT_DATABASE_PATH=/var/lib/matrix-conduit/ \
      -e CONDUIT_DATABASE_BACKEND=rocksdb \
      -e CONDUIT_PORT=6167 -e CONDUIT_ADDRESS=0.0.0.0 \
      -e CONDUIT_ALLOW_REGISTRATION=true \
      -e CONDUIT_ALLOW_FEDERATION=false \
      -e CONDUIT_MAX_REQUEST_SIZE=20000000 \
      -e CONDUIT_TRUSTED_SERVERS='[]' \
      -e CONDUIT_CONFIG='' \
      matrixconduit/matrix-conduit:latest >/dev/null
  fi
  wait_ready
  echo "homeserver up at $HS"
}

cmd_seed() {
  need jq
  wait_ready
  mkdir -p "$DIR"
  register alice
  register bob
  login alice >"$DIR/alice.json"
  login bob >"$DIR/bob.json"

  local a b room
  a=$(jq -r .access_token "$DIR/alice.json")
  b=$(jq -r .access_token "$DIR/bob.json")
  room=$(curl -s -X POST "$HS/_matrix/client/v3/createRoom" -H "Authorization: Bearer $a" \
    -H 'Content-Type: application/json' \
    -d '{"name":"Live Test","preset":"private_chat","invite":["@bob:localhost"]}' | jq -r .room_id)
  curl -s -X POST "$HS/_matrix/client/v3/join/$room" -H "Authorization: Bearer $b" \
    -H 'Content-Type: application/json' -d '{}' >/dev/null
  curl -s -X PUT "$HS/_matrix/client/v3/rooms/$room/send/m.room.message/$(txn)" \
    -H "Authorization: Bearer $b" -H 'Content-Type: application/json' \
    -d '{"msgtype":"m.text","body":"hello from bob"}' >/dev/null
  echo "$room" >"$DIR/room.txt"

  echo "seeded: @alice:localhost / @bob:localhost (password $PASSWORD)"
  echo "room:   $room"
  echo "files:  $DIR/alice.json $DIR/bob.json $DIR/room.txt"
}

cmd_down() {
  need docker
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  echo "removed $NAME"
}

cmd_env() {
  echo "PRINNY_HS_URL=$HS"
  echo "PRINNY_LIVE_DIR=$DIR"
  echo "PRINNY_HS_CONTAINER=$NAME"
}

case "${1:-}" in
  up) cmd_up ;;
  seed) cmd_seed ;;
  down) cmd_down ;;
  env) cmd_env ;;
  *)
    sed -n '2,9p' "$0"
    exit 2
    ;;
esac
