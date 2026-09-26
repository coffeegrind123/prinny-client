#!/usr/bin/env bash
# Build the web client into the live-test directory and serve it.
#
#   serve.sh build   production build with source maps -> $PRINNY_LIVE_DIR/dist
#   serve.sh start   serve that build on 127.0.0.1:$PRINNY_WEB_PORT (default 5199)
#   serve.sh stop
#
# Why a production build and not `vite`: the dev server's dependency
# optimisation fails under Vite 8 ("Error during dependency optimization: Not
# implemented", from @esbuild-plugins/node-globals-polyfill), so there is no
# dev mode to test against. The build goes to its own directory rather than
# cinny/dist so a test build never ends up embedded in a desktop bundle.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CINNY=$(cd "$HERE/../../cinny" && pwd)
DIR=${PRINNY_LIVE_DIR:-/tmp/prinny-live-test}
PORT=${PRINNY_WEB_PORT:-5199}
PIDFILE=$DIR/preview.pid

cmd_build() {
  mkdir -p "$DIR"
  cd "$CINNY"
  # Several minutes; 15+ when the machine is short of memory. Judge success by
  # the exit status, never by piping into tail.
  npx vite build --outDir "$DIR/dist" --emptyOutDir --sourcemap true >"$DIR/build.log" 2>&1 ||
    { tail -30 "$DIR/build.log" >&2; exit 1; }
  echo "built -> $DIR/dist"
}

cmd_start() {
  [ -f "$DIR/dist/index.html" ] || { echo "no build in $DIR/dist — run: $0 build" >&2; exit 1; }
  cmd_stop >/dev/null
  cd "$CINNY"
  # Own session, so `stop` can take down npx -> sh -> node as one process
  # group. Killing the recorded pid alone orphans the node server, which then
  # keeps the port.
  setsid nohup npx vite preview --outDir "$DIR/dist" --port "$PORT" --strictPort --host 127.0.0.1 \
    >"$DIR/preview.log" 2>&1 &
  echo $! >"$PIDFILE"
  for _ in $(seq 1 90); do
    if curl -sf -o /dev/null "http://127.0.0.1:$PORT/"; then
      echo "serving http://127.0.0.1:$PORT/"
      return 0
    fi
    sleep 1
  done
  echo "preview did not come up — see $DIR/preview.log" >&2
  exit 1
}

cmd_stop() {
  if [ -f "$PIDFILE" ]; then
    kill -- "-$(cat "$PIDFILE")" 2>/dev/null || true
    rm -f "$PIDFILE"
  fi
  echo "stopped"
}

case "${1:-}" in
  build) cmd_build ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  *)
    sed -n '2,6p' "$0"
    exit 2
    ;;
esac
