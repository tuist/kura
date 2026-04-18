#!/bin/sh
set -eu

RIAK_HOME="${RIAK_HOME:-/opt/riak}"
PHOENIX_COMMAND="${PHOENIX_COMMAND:-elixir -S mix phx.server}"
RIAK_FAILURE_FLAG="/tmp/cache-next-riak.failed"

cleanup() {
  trap - INT TERM

  if [ -n "${RIAK_CLUSTER_PID:-}" ]; then
    kill "$RIAK_CLUSTER_PID" >/dev/null 2>&1 || true
    wait "$RIAK_CLUSTER_PID" 2>/dev/null || true
  fi

  if [ -n "${RIAK_WATCH_PID:-}" ]; then
    kill "$RIAK_WATCH_PID" >/dev/null 2>&1 || true
    wait "$RIAK_WATCH_PID" 2>/dev/null || true
  fi

  if [ -n "${PHOENIX_PID:-}" ] && kill -0 "$PHOENIX_PID" >/dev/null 2>&1; then
    kill -TERM "$PHOENIX_PID" >/dev/null 2>&1 || true
    wait "$PHOENIX_PID" 2>/dev/null || true
  fi

  "$RIAK_HOME/bin/riak" stop >/dev/null 2>&1 || true

  if [ -n "${RIAK_LOG_PID:-}" ]; then
    kill "$RIAK_LOG_PID" >/dev/null 2>&1 || true
    wait "$RIAK_LOG_PID" 2>/dev/null || true
  fi
}

term_handler() {
  cleanup
  exit 0
}

watch_riak() {
  while sleep 5; do
    if ! "$RIAK_HOME/bin/riak" ping >/dev/null 2>&1; then
      echo "Riak became unhealthy, stopping Phoenix"
      touch "$RIAK_FAILURE_FLAG"

      if [ -n "${PHOENIX_PID:-}" ] && kill -0 "$PHOENIX_PID" >/dev/null 2>&1; then
        kill -TERM "$PHOENIX_PID" >/dev/null 2>&1 || true
      fi

      return 0
    fi
  done
}

trap term_handler INT TERM

rm -f "$RIAK_FAILURE_FLAG"

export RIAK_FOREGROUND_MODE=start
/usr/local/bin/riak-entrypoint

export RIAK_MAINTAINER_EXIT_ON_CONVERGENCE=true
/usr/local/bin/riak-cluster-maintainer &
RIAK_CLUSTER_PID=$!

touch "$RIAK_HOME/log/console.log" "$RIAK_HOME/log/error.log"
tail -F "$RIAK_HOME/log/console.log" "$RIAK_HOME/log/error.log" &
RIAK_LOG_PID=$!

watch_riak &
RIAK_WATCH_PID=$!

sh -lc "exec $PHOENIX_COMMAND" &
PHOENIX_PID=$!

set +e
wait "$PHOENIX_PID"
PHOENIX_STATUS=$?
set -e

cleanup

if [ -f "$RIAK_FAILURE_FLAG" ]; then
  exit 1
fi

exit "$PHOENIX_STATUS"
