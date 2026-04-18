#!/bin/sh
set -eu

RIAK_HOME="${RIAK_HOME:-/opt/riak}"
RIAK_NODENAME="${RIAK_NODENAME:?RIAK_NODENAME is required}"
RIAK_COOKIE="${RIAK_COOKIE:-cache-next-riak}"
RIAK_HTTP_BIND="${RIAK_HTTP_BIND:-0.0.0.0}"
RIAK_HTTP_PORT="${RIAK_HTTP_PORT:-8098}"
RIAK_PB_BIND="${RIAK_PB_BIND:-0.0.0.0}"
RIAK_PB_PORT="${RIAK_PB_PORT:-8087}"
RIAK_SEED_NODE="${RIAK_SEED_NODE:-}"
RIAK_FOREGROUND_MODE="${RIAK_FOREGROUND_MODE:-tail}"

mkdir -p "$RIAK_HOME/data" "$RIAK_HOME/log" "$RIAK_HOME/etc"

printf '%s\n' "$RIAK_COOKIE" > /root/.erlang.cookie
chmod 400 /root/.erlang.cookie
export RELX_COOKIE="$RIAK_COOKIE"

cat > "$RIAK_HOME/etc/riak.conf" <<EOF
nodename = $RIAK_NODENAME
distributed_cookie = $RIAK_COOKIE
listener.http.internal = ${RIAK_HTTP_BIND}:${RIAK_HTTP_PORT}
listener.protobuf.internal = ${RIAK_PB_BIND}:${RIAK_PB_PORT}
storage_backend = leveldb
leveldb.data_root = ${RIAK_HOME}/data/leveldb
EOF

for vmargs in "$RIAK_HOME"/releases/*/vm.args; do
  [ -f "$vmargs" ] || continue
  sed -i'' -e "s/^-sname .*/-name $RIAK_NODENAME/" \
           -e "s/^-name .*/-name $RIAK_NODENAME/" \
           -e "s/^-setcookie .*/-setcookie $RIAK_COOKIE/" \
           "$vmargs"
done

term_handler() {
  "$RIAK_HOME/bin/riak" stop >/dev/null 2>&1 || true
  exit 0
}

trap term_handler INT TERM

"$RIAK_HOME/bin/riak" daemon &

until "$RIAK_HOME/bin/riak" ping >/dev/null 2>&1; do
  sleep 2
done

"$RIAK_HOME/bin/riak-admin" wait-for-service riak_kv >/dev/null 2>&1

case "$RIAK_FOREGROUND_MODE" in
  tail)
    export RIAK_MAINTAINER_EXIT_ON_CONVERGENCE=true
    /usr/local/bin/riak-cluster-maintainer
    touch "$RIAK_HOME/log/console.log" "$RIAK_HOME/log/error.log"
    exec tail -F "$RIAK_HOME/log/console.log" "$RIAK_HOME/log/error.log"
    ;;

  bootstrap)
    export RIAK_MAINTAINER_EXIT_ON_CONVERGENCE=true
    /usr/local/bin/riak-cluster-maintainer
    exit 0
    ;;

  start)
    exit 0
    ;;

  *)
    echo "Unsupported RIAK_FOREGROUND_MODE: $RIAK_FOREGROUND_MODE" >&2
    exit 1
    ;;
esac
