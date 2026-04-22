#!/usr/bin/env bats

setup_file() {
  export COMPOSE_PROJECT_NAME="kura-clients"
  export KURA_US_PORT=4401
  export KURA_EU_PORT=4402
  export KURA_AP_PORT=4403
  export KURA_US_GRPC_PORT=5501
  export KURA_EU_GRPC_PORT=5502
  export KURA_AP_GRPC_PORT=5503
  export KURA_US_URL="http://localhost:${KURA_US_PORT}"
  export KURA_EU_URL="http://localhost:${KURA_EU_PORT}"
  export KURA_AP_URL="http://localhost:${KURA_AP_PORT}"
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up --build -d kura-us kura-eu kura-ap

  wait_for_http "${KURA_US_URL}/up"
  wait_for_http "${KURA_EU_URL}/up"
  wait_for_http "${KURA_AP_URL}/up"
  wait_for_contains "${KURA_US_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_EU_URL}/up" '"ring_members":3'
  wait_for_contains "${KURA_AP_URL}/up" '"ring_members":3'
}

teardown_file() {
  local buck2_path
  buck2_path="$(mise exec -- which buck2 2>/dev/null || true)"
  if [ -n "$buck2_path" ]; then
    "$buck2_path" killall >/dev/null 2>&1 || true
  fi
  dc logs --no-color >"${BATS_FILE_TMPDIR}/compose.log" 2>&1 || true
  dc down -v --remove-orphans >/dev/null 2>&1 || true
}

dc() {
  docker compose "$@"
}

wait_for_http() {
  local url="$1"

  for _ in $(seq 1 90); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  echo "Timed out waiting for $url" >&2
  return 1
}

wait_for_contains() {
  local url="$1"
  local needle="$2"

  for _ in $(seq 1 45); do
    local body
    body="$(curl -fsS "$url" 2>/dev/null || true)"
    if [[ "$body" == *"$needle"* ]]; then
      printf '%s' "$body"
      return 0
    fi
    sleep 2
  done

  return 1
}

new_marker() {
  python3 - <<'PY'
import secrets
print(secrets.token_hex(8))
PY
}

create_bazel_workspace() {
  local dir="$1"
  local marker="$2"

  mkdir -p "$dir"
  cat >"$dir/MODULE.bazel" <<'EOF'
module(name = "kura_bazel_demo")
EOF
  cat >"$dir/BUILD.bazel" <<EOF
genrule(
    name = "hello",
    outs = ["hello.txt"],
    cmd = "echo ${marker} > \$@",
)
EOF
}

bazel_build() {
  local dir="$1"
  local grpc_port="$2"
  local instance_name="$3"
  local bazel_path
  bazel_path="$(mise exec -- which bazel)"

  (
    cd "$dir"
    "$bazel_path" \
      build //:hello \
      --remote_cache="grpc://127.0.0.1:${grpc_port}" \
      --remote_instance_name="${instance_name}" \
      --remote_upload_local_results=true \
      --remote_download_outputs=all \
      --show_result=0 \
      --noshow_loading_progress \
      --noshow_progress
  )
}

create_buck_workspace() {
  local dir="$1"
  local grpc_port="$2"
  local marker="$3"
  local instance_name="$4"
  local buck2_path
  buck2_path="$(mise exec -- which buck2)"

  mkdir -p "$dir"
  (
    cd "$dir"
    "$buck2_path" init --git >/dev/null
    mkdir -p platforms
    cat > platforms/defs.bzl <<'EOF'
def _impl(ctx):
    configuration = ConfigurationInfo(constraints = {}, values = {})

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = True,
            remote_enabled = False,
            remote_cache_enabled = True,
            allow_cache_uploads = True,
            use_limited_hybrid = False,
        ),
    )

    return [DefaultInfo(), ExecutionPlatformRegistrationInfo(platforms = [platform])]

platforms = rule(attrs = {}, impl = _impl)
EOF
    cat > platforms/BUCK <<'EOF'
load(":defs.bzl", "platforms")
platforms(name = "platforms")
EOF
    cat > .buckconfig.local <<EOF
[build]
  execution_platforms = root//platforms:platforms

[buck2_re_client]
  action_cache_address = grpc://127.0.0.1:${grpc_port}
  cas_address = grpc://127.0.0.1:${grpc_port}
  engine_address = grpc://127.0.0.1:${grpc_port}
  tls = false
  instance_name = ${instance_name}
EOF
    cat > BUCK <<EOF
genrule(
    name = "hello_world",
    out = "out.txt",
    cmd = "echo ${marker} > \$OUT",
    cacheable = True,
    labels = ["network_access"],
)
EOF
  )
}

buck_build() {
  local dir="$1"
  local isolation_dir="$2"
  local buck2_path
  buck2_path="$(mise exec -- which buck2)"

  (
    cd "$dir"
    "$buck2_path" build //:hello_world --show-output --console=simple --isolation-dir "${isolation_dir}"
  )
}

create_nx_workspace() {
  local dir="$1"
  local marker="$2"

  mkdir -p "$dir/apps/demo"
  cat >"$dir/package.json" <<'EOF'
{
  "name": "kura-nx-demo",
  "private": true,
  "devDependencies": {
    "nx": "22.6.5"
  }
}
EOF
  cat >"$dir/nx.json" <<'EOF'
{
  "$schema": "./node_modules/nx/schemas/nx-schema.json",
  "targetDefaults": {
    "build": {
      "cache": true,
      "outputs": ["{workspaceRoot}/dist/{projectRoot}"]
    }
  }
}
EOF
  cat >"$dir/apps/demo/project.json" <<EOF
{
  "name": "demo",
  "\$schema": "../../node_modules/nx/schemas/project-schema.json",
  "targets": {
    "build": {
      "executor": "nx:run-commands",
      "options": {
        "command": "mkdir -p dist/apps/demo && echo ${marker} > dist/apps/demo/out.txt"
      }
    }
  }
}
EOF
  (
    cd "$dir"
    npm install --silent --no-audit --no-fund >/dev/null
  )
}

nx_build() {
  local dir="$1"
  local server_url="$2"

  (
    cd "$dir"
    NX_DAEMON=false NX_SELF_HOSTED_REMOTE_CACHE_SERVER="$server_url" npx nx run demo:build --outputStyle=static
  )
}

create_metro_workspace() {
  local dir="$1"

  mkdir -p "$dir"
  cat >"$dir/package.json" <<'EOF'
{
  "name": "kura-metro-demo",
  "private": true,
  "dependencies": {
    "metro-cache": "0.84.3"
  }
}
EOF
  (
    cd "$dir"
    npm install --silent --no-audit --no-fund >/dev/null
  )
}

metro_put() {
  local dir="$1"
  local endpoint="$2"
  local key_hex="$3"
  local payload="$4"

  (
    cd "$dir"
    ENDPOINT="$endpoint" KEY_HEX="$key_hex" PAYLOAD="$payload" node - <<'EOF'
const { HttpStore } = require('metro-cache');

(async () => {
  const store = new HttpStore({ endpoint: process.env.ENDPOINT });
  await store.set(Buffer.from(process.env.KEY_HEX, 'hex'), Buffer.from(process.env.PAYLOAD));
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
EOF
  )
}

metro_get() {
  local dir="$1"
  local endpoint="$2"
  local key_hex="$3"

  (
    cd "$dir"
    ENDPOINT="$endpoint" KEY_HEX="$key_hex" node - <<'EOF'
const { HttpGetStore } = require('metro-cache');

(async () => {
  const store = new HttpGetStore({ endpoint: process.env.ENDPOINT });
  const value = await store.get(Buffer.from(process.env.KEY_HEX, 'hex'));
  if (!value) {
    process.exit(2);
  }
  process.stdout.write(Buffer.isBuffer(value) ? value : Buffer.from(JSON.stringify(value)));
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
EOF
  )
}

@test "Bazel reuses remote cache entries across regions" {
  local marker="bazel-$(new_marker)"
  local instance_name="bazel/${marker}"
  local work1
  local work2
  work1="$(mktemp -d "${BATS_FILE_TMPDIR}/bazel-1.XXXXXX")"
  work2="$(mktemp -d "${BATS_FILE_TMPDIR}/bazel-2.XXXXXX")"

  create_bazel_workspace "$work1" "$marker"
  create_bazel_workspace "$work2" "$marker"

  run bazel_build "$work1" "$KURA_US_GRPC_PORT" "$instance_name"
  [ "$status" -eq 0 ]
  [[ "$output" != *"remote cache hit"* ]]
  grep -F "$marker" "$work1/bazel-bin/hello.txt"

  run bazel_build "$work2" "$KURA_EU_GRPC_PORT" "$instance_name"
  [ "$status" -eq 0 ]
  [[ "$output" == *"remote cache hit"* ]]
  grep -F "$marker" "$work2/bazel-bin/hello.txt"
}

@test "Buck2 reuses REAPI cache entries across regions" {
  local marker="buck-$(new_marker)"
  local instance_name="buck/${marker}"
  local work1
  local work2
  work1="$(mktemp -d "${BATS_FILE_TMPDIR}/buck-1.XXXXXX")"
  work2="$(mktemp -d "${BATS_FILE_TMPDIR}/buck-2.XXXXXX")"

  create_buck_workspace "$work1" "$KURA_US_GRPC_PORT" "$marker" "$instance_name"
  create_buck_workspace "$work2" "$KURA_EU_GRPC_PORT" "$marker" "$instance_name"

  run buck_build "$work1" "buck-${marker}-us"
  [ "$status" -eq 0 ]
  [[ "$output" == *"Cache hits: 0%"* ]]
  local output_path
  output_path="$(printf '%s\n' "$output" | awk '/^root\/\/:hello_world / { print $2 }' | tail -n1)"
  [ -n "$output_path" ]
  grep -F "$marker" "$work1/$output_path"

  run buck_build "$work2" "buck-${marker}-eu"
  [ "$status" -eq 0 ]
  [[ "$output" == *"Cache hits: 100%"* ]]
  [[ "$output" == *"cached: 1"* ]]
  output_path="$(printf '%s\n' "$output" | awk '/^root\/\/:hello_world / { print $2 }' | tail -n1)"
  [ -n "$output_path" ]
  grep -F "$marker" "$work2/$output_path"
}

@test "Nx self-hosted cache entries replicate across regions" {
  local marker="nx-$(new_marker)"
  local work1
  local work2
  work1="$(mktemp -d "${BATS_FILE_TMPDIR}/nx-1.XXXXXX")"

  create_nx_workspace "$work1" "$marker"
  work2="$(mktemp -d "${BATS_FILE_TMPDIR}/nx-2.XXXXXX")"
  cp -R "$work1/." "$work2/"
  rm -rf "$work2/.nx" "$work2/dist"

  run nx_build "$work1" "${KURA_US_URL}"
  [ "$status" -eq 0 ]
  [[ "$output" != *"[remote cache]"* ]]
  grep -F "$marker" "$work1/dist/apps/demo/out.txt"

  run nx_build "$work2" "${KURA_EU_URL}"
  [ "$status" -eq 0 ]
  [[ "$output" == *"[remote cache]"* ]]
  grep -F "$marker" "$work2/dist/apps/demo/out.txt"
}

@test "Metro cache artifacts sync across regions" {
  local work
  local key_hex
  local payload
  work="$(mktemp -d "${BATS_FILE_TMPDIR}/metro.XXXXXX")"
  key_hex="$(new_marker)$(new_marker)"
  payload="metro-$(new_marker)"

  create_metro_workspace "$work"

  run metro_put "$work" "${KURA_US_URL}/api/metro/cache" "$key_hex" "$payload"
  [ "$status" -eq 0 ]

  for _ in $(seq 1 20); do
    run metro_get "$work" "${KURA_EU_URL}/api/metro/cache" "$key_hex"
    if [ "$status" -eq 0 ]; then
      break
    fi
    sleep 1
  done

  [ "$status" -eq 0 ]
  [ "$output" = "$payload" ]
}
