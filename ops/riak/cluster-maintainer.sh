#!/bin/sh
set -eu

RIAK_HOME="${RIAK_HOME:-/opt/riak}"
RIAK_NODENAME="${RIAK_NODENAME:?RIAK_NODENAME is required}"
RIAK_SEED_NODE="${RIAK_SEED_NODE:-}"
RIAK_CLUSTER_NODES="${RIAK_CLUSTER_NODES:-$RIAK_NODENAME}"
RIAK_MAINTAINER_EXIT_ON_CONVERGENCE="${RIAK_MAINTAINER_EXIT_ON_CONVERGENCE:-true}"

expected_nodes=$(printf '%s' "$RIAK_CLUSTER_NODES" | awk -F',' '{print NF}')
last_join_output=""
last_member_status=""
last_plan_output=""

all_members_joined() {
  member_status="$1"
  node_count=$(printf '%s\n' "$member_status" | grep -c 'riak@' || true)

  [ "$node_count" -eq "$expected_nodes" ] &&
    ! printf '%s\n' "$member_status" | grep -Eq '^[[:space:]]*(joining|leaving|exiting|down)\b'
}

print_if_changed() {
  current="$1"
  previous="$2"

  if [ -n "$current" ] && [ "$current" != "$previous" ]; then
    printf '%s\n' "$current"
  fi
}

while :; do
  if [ -n "$RIAK_SEED_NODE" ] && [ "$RIAK_SEED_NODE" != "$RIAK_NODENAME" ]; then
    join_output=$("$RIAK_HOME/bin/riak-admin" cluster join "$RIAK_SEED_NODE" 2>&1 || true)
    print_if_changed "$join_output" "$last_join_output"
    last_join_output="$join_output"
  fi

  member_status=$("$RIAK_HOME/bin/riak-admin" member-status 2>&1 || true)
  print_if_changed "$member_status" "$last_member_status"
  last_member_status="$member_status"

  if all_members_joined "$member_status"; then
    if [ "$RIAK_MAINTAINER_EXIT_ON_CONVERGENCE" = "true" ]; then
      exit 0
    fi

    sleep 10
    continue
  fi

  plan_output=$("$RIAK_HOME/bin/riak-admin" cluster plan 2>&1 || true)
  print_if_changed "$plan_output" "$last_plan_output"
  last_plan_output="$plan_output"

  if [ -n "$plan_output" ] && ! printf '%s\n' "$plan_output" | grep -Fq 'There are no staged changes'; then
    "$RIAK_HOME/bin/riak-admin" cluster commit 2>&1 || true
  fi

  sleep 2
done
