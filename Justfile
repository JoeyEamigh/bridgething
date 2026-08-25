# bridgething Justfile

# --- Path config ---

cross_target := 'aarch64-unknown-linux-gnu'
wasm_target := 'wasm32-unknown-unknown'
wasm_crates := '-p libbridgething -p bridgething-sdk-runtime -p bridgething-gateway -p bridgething-delivery -p bridgething-delivery-wasm'
napi_dir := justfile_directory() / 'crates/delivery/napi'
wasm_dir := justfile_directory() / 'crates/delivery/wasm'
cross_target_dir := justfile_directory() / 'target-cross'
cross_release_dir := justfile_directory() / 'target-cross-release'
device_features := 'superbird'
dev_profile := '--config profile.release.lto=false --config profile.release.codegen-units=32'
release_build := 'cargo build --release --locked -p bridgething --target ' + cross_target + ' --no-default-features --features ' + device_features
device_bt_mac := env_var_or_default('SUPERBIRD_BT_MAC', '30:E3:D6:03:96:1E')
dev_dir := justfile_directory() / '.dev'
dev_gateway_url := 'ws://127.0.0.1:8892/'
swupdate_vendor := justfile_directory() / 'crates/swupdate-sys/vendor/swupdate'
swupdate_libdir := justfile_directory() / 'target/libswupdate'

# --- Build image ---

# The image mounts the repo at /work, so container recipes need the container's view of a path.
container_vendor := '/work/crates/swupdate-sys/vendor/swupdate'
# Registry and target live in named volumes: virtiofs drops writes between rapid container runs
container_run := 'docker run --rm -v ' + justfile_directory() + ':/work -w /work -v bridgething-cargo-registry:/usr/local/cargo/registry -v bridgething-target-cross:/target -e CARGO_TARGET_DIR=/target -e CARGO_INCREMENTAL=0 bridgething-build'
# mic-debug binds alsa + evdev, which a mac host has no toolchain for; test-shipping covers it there.
host_test_excludes := if os() == 'macos' { '--exclude bridgething-mic-debug' } else { '' }

# --- Local dev ---

run:
  cargo run -p bridgething

build:
  just typescript
  cargo build
  bun run build

fmt:
  cargo +nightly fmt --all
  bun run format

# --- Host dev daemon ---

# Run the daemon here with the radio untouched (no bluez session, no adapter), loopback binds, state under .dev/.
dev-daemon:
  mkdir -p {{dev_dir}}/state {{dev_dir}}/webapps {{dev_dir}}/examples
  BRIDGETHING_STATE_DIR={{dev_dir}}/state BRIDGETHING_WEBAPPS_DIR={{dev_dir}}/webapps BRIDGETHING_EXAMPLES_DIR={{dev_dir}}/examples RUST_LOG="${RUST_LOG:-bridgething=debug,bridgething::chrome=info,libbridgething=info}" cargo run -p bridgething --features test-tap -- --dev

# Build and launch the dev daemon in the background; pidfile + log under .dev/.
dev-daemon-start:
  scripts/dev-daemon.sh start

# SIGTERM the backgrounded dev daemon via its pidfile and wait for exit.
dev-daemon-stop:
  scripts/dev-daemon.sh stop

# Report the backgrounded dev daemon's pid and whether the gateway port is reachable.
dev-daemon-status:
  scripts/dev-daemon.sh status

# Dial the dev daemon with the reference companion gateway: `just dev-gateway connect`, or any host-gateway subcommand.
dev-gateway *args:
  cargo run -p bridgething-host-gateway -- --url {{dev_gateway_url}} {{args}}

# --- Codegen ---

typescript:
  cargo run -q -p bridgething-codegen -- ts
  bun run format

rust:
  cargo run -q -p bridgething-codegen -- rust
  cargo +nightly fmt --all

codegen:
  cargo run -q -p bridgething-codegen -- all
  just fmt

# --- Uniffi mobile packaging ---

# Regenerate the shared-core kotlin + swift bindings from the host-arch cdylib.
companion-bindings:
  bash scripts/generate-companion-bindings.sh

# Build the shared core as an ios xcframework + swift wrapper. `sim` builds the simulator slice alone.
companion-ios slices="all":
  {{ if slices == "sim" { "XCFRAMEWORK_SIM_ONLY=1" } else { "" } }} bash scripts/build-uniffi-xcframework.sh companion BridgethingCompanionCore

# --- Mobile app artifacts ---

# Build a release apk (debug-signed for sideload). Runs on mac and linux.
apk:
  bash mobile/scripts/build-apk.sh

apk-emulator:
  bash mobile/scripts/build-apk.sh emulator

# Drive the android app on the emulator against a seeded host daemon
e2e-android *flows:
  bash scripts/e2e-mobile.sh android {{flows}}

# Drive the ios app on the simulator against a seeded host daemon
e2e-ios *flows: (companion-ios "sim")
  bash scripts/e2e-mobile.sh ios {{flows}}

# Build an unsigned ipa for sideloading. Requires macos.
ipa: companion-ios
  bash mobile/scripts/build-ipa.sh

# Build a signed app-store ipa for TestFlight. Requires macos + a configured asc profile.
testflight: companion-ios
  bash mobile/scripts/build-testflight.sh

goldens:
  UPDATE_GOLDEN=1 cargo test -p libbridgething --test golden golden_vectors_match_fixture_file

# --- Test suites ---

# FORCE=1 makes the cache-backed suites ignore what they think is already green.
gradle_force := if env_var_or_default('FORCE', '') == '1' { '--rerun-tasks' } else { '' }
turbo_force := if env_var_or_default('FORCE', '') == '1' { '--force' } else { '' }

# Everything CI gates on
test-all:
  #!/usr/bin/env bash
  set -uo pipefail
  # cheapest signal first; nothing short-circuits, so this only changes what you see soonest
  recipes=(typecheck-ts test-rust test-kotlin test-swift test-ts test-napi test-wasm)
  declare -A result
  failed=0

  for recipe in "${recipes[@]}"; do
    printf '\n===> %s\n' "$recipe"
    if {{just_executable()}} --justfile {{justfile()}} "$recipe"; then
      result[$recipe]=pass
    else
      result[$recipe]=FAIL
      failed=1
    fi
  done

  printf '\n%-13s %s\n' recipe result
  printf '%-13s %s\n' ------------- ------
  for recipe in "${recipes[@]}"; do
    printf '%-13s %s\n' "$recipe" "${result[$recipe]}"
  done

  exit "$failed"

# full test battery
release-check:
  #!/usr/bin/env bash
  set -uo pipefail
  recipes=(test-all e2e-android e2e-ios)
  declare -A result
  failed=0

  for recipe in "${recipes[@]}"; do
    printf '\n===> %s\n' "$recipe"
    if {{just_executable()}} --justfile {{justfile()}} "$recipe"; then
      result[$recipe]=pass
    else
      result[$recipe]=FAIL
      failed=1
    fi
  done

  printf '\n%-13s %s\n' recipe result
  printf '%-13s %s\n' ------------- ------
  for recipe in "${recipes[@]}"; do
    printf '%-13s %s\n' "$recipe" "${result[$recipe]}"
  done

  exit "$failed"

# daemon unit tests plus the full harness, host-native, proptest dialed down
test-fast:
  cargo test -p bridgething --lib
  PROPTEST_CASES=8 cargo test -p bridgething-test-harness

# rust workspace plus the shipping feature set
test-rust: test-shipping check-windows
  cargo test --workspace {{host_test_excludes}} --locked --no-fail-fast

# whatever this host cannot build natively
test-shipping:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ '{{os()}}' = 'macos' ]; then
    {{just_executable()}} --justfile {{justfile()}} build-image
    {{container_run}} bash -c "scripts/libswupdate-stub.sh {{container_vendor}} cc /usr/lib \
      && cargo test -p bridgething --locked --no-default-features --features {{device_features}} -j 2 -- --test-threads 2 \
      && cargo test -p bridgething-mic-debug --locked -j 2 -- --test-threads 2"
  else
    scripts/libswupdate-stub.sh {{swupdate_vendor}} cc {{swupdate_libdir}}
    RUSTFLAGS='-L {{swupdate_libdir}}' LD_LIBRARY_PATH={{swupdate_libdir}} cargo test -p bridgething --locked --no-default-features --features {{device_features}}
  fi

# The workspace suite against the device target inside the build image
test-cross: build-image
  {{container_run}} cargo test --workspace --exclude bridgething-desktop --target {{cross_target}} --locked --no-fail-fast -j 2 -- --test-threads 2

# The desktop shell's headless suite
test-desktop:
  cargo test -p bridgething-desktop --locked

# The windows backend
check-windows:
  #!/usr/bin/env bash
  set -euo pipefail
  if ! rustup target list --installed | grep -qx x86_64-pc-windows-msvc; then
    echo 'skipping check-windows: rustup target add x86_64-pc-windows-msvc' >&2
    exit 0
  fi
  lanes="{{justfile_directory()}}/desktop/src-tauri/wincheck"
  export CARGO_TARGET_DIR="{{justfile_directory()}}/target/wincheck"
  cargo clippy --manifest-path "$lanes/audit/Cargo.toml" --target x86_64-pc-windows-msvc --all-targets -- -D warnings
  cargo clippy --manifest-path "$lanes/host/Cargo.toml" --all-targets -- -D warnings
  cargo test --manifest-path "$lanes/host/Cargo.toml"

# Tray app against the vite dev server
desktop-dev:
  cd desktop && bun run tauri dev

# Release bundle for the host platform
desktop-build:
  cd desktop && bun run tauri build

# JVM suites for every gradle subproject
test-kotlin:
  @bash -c 'source scripts/gradle-jdk.sh && gradle_jdk_env && JAVA_HOME="$GRADLE_JAVA" ./gradlew test :packages:companion:kotlin:companion:compileDebugAndroidTestSources {{gradle_force}} --no-daemon --console=plain --stacktrace -Porg.gradle.java.installations.paths="$GRADLE_INSTALLS" -Porg.gradle.java.installations.auto-download=false </dev/null'

# Swift package suite
test-swift:
  cargo build -p bridgething-companion --lib
  BRIDGETHING_COMPANION_CDYLIB=1 swift test

# Every bun workspace member that declares a test script
test-ts:
  bun run test -- {{turbo_force}}

# Every bun workspace member that declares a typecheck script
typecheck-ts:
  bun run typecheck -- {{turbo_force}}

# The node addon's own suite
test-napi: build-napi
  cd {{napi_dir}} && bun test

# The browser artifact's suite, run headlessly under node.
test-wasm:
  cd {{wasm_dir}} && wasm-pack test --node

# --- Test harness ---

# Host tiers (T1 in-process + T2 chromium): no hardware, runs in parallel.
test-host:
  cargo test -p bridgething-test-harness

# Over-air tier (T3): needs a booted Car Thing with the test-tap daemon + a host BT radio
test-device:
  SUPERBIRD_BT_MAC={{device_bt_mac}} cargo test -p bridgething-test-harness --test seam --test t3_infra -- --ignored --test-threads=1 --nocapture

# Browse for a real Car Thing advertising itself
test-discovery-live:
  cargo test -p bridgething-delivery --lib discovery -- --ignored --nocapture

# --- Device iteration ---

# check out vendored submodules; a plain clone leaves swupdate's ipc sources empty and the cross image build dies
submodules:
  git submodule update --init --recursive

# Build the daemon build image
build-image: submodules
  docker build -t bridgething-build -f scripts/cross-aarch64.Dockerfile .

# Cross-build the daemon. `extra` appends to the shipping feature set: `mic` for voice, `test-tap` for the test binary.
cross-build extra="": build-image
  docker run --rm -v {{justfile_directory()}}:/work -w /work -v bridgething-cargo-registry:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/work/target-cross -e RUSTFLAGS='--remap-path-prefix=/work=/bridgething --remap-path-prefix=/usr/local/cargo=/cargo' bridgething-build cargo build --release -p bridgething --target {{cross_target}} --no-default-features --features "{{device_features}}{{ if extra == '' { '' } else { ',' + extra } }}" {{dev_profile}}

# Cross-check the daemon, `extra` as above.
check extra="": build-image
  docker run --rm -v {{justfile_directory()}}:/work -w /work -v bridgething-cargo-registry:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/work/target-cross bridgething-build cargo check -p bridgething --target {{cross_target}} --no-default-features --features "{{device_features}}{{ if extra == '' { '' } else { ',' + extra } }}" --locked

# Release-build the daemon inside the cross image. for any host without an aarch64 toolchain (mac).
cross-release: build-image
  docker run --rm -v {{justfile_directory()}}:/work -w /work -v bridgething-cargo-registry:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/work/target-cross-release bridgething-build {{release_build}}

# Release-build the daemon against a toolchain already on the host, provisioned by scripts/cross-aarch64-deps.sh
cross-release-native:
  CARGO_TARGET_DIR={{cross_release_dir}} {{release_build}}

# Cross-build then push the daemon to /opt/bridgething/daemon/. `just push mic` gets the voice stack.
push extra="": (cross-build extra)
  scripts/bridgething-push-daemon {{cross_target_dir}}/{{cross_target}}/release/bridgething

# Swap the wake-word phrase model on the connected device and restart the daemon
push-wakeword:
  scripts/bridgething-push-wakeword

# Publish the wake-word phrase model, printing the manifest fragment
publish-wakeword *args:
  scripts/bridgething-publish-wakeword {{args}}

# Publish the phone-side nlu bundle zips + channel manifest
publish-nlu *args:
  scripts/bridgething-publish-nlu {{args}}

# Publish the android asr model + channel manifest
publish-asr *args:
  scripts/bridgething-publish-asr {{args}}

# Cross-build the MFi i2c-3 dev proxy and push it + its unit to the device
push-mfi-proxy: build-image
  docker run --rm -v {{justfile_directory()}}:/work -w /work -v bridgething-cargo-registry:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/work/target-cross bridgething-build cargo build --release -p bridgething-mfi-proxy --target {{cross_target}} --locked
  scripts/bridgething-push-mfi-proxy

# Push a webapp bundle into /var/bridgething/webapps/<name>/
push-webapp local name="":
  scripts/bridgething-push-webapp {{local}} {{name}}

# SSH into the device. Forwards the command as one string, so `;` and `&&` reach the device.
ssh *args:
  @bash -c 'source scripts/device.sh && device_ssh "$1"' -- {{quote(args)}}

# Tail bridgething.service journal.
logs:
  @bash -c 'source scripts/device.sh && device_ssh journalctl -fu bridgething.service'

# Set the device's bridgething to trace. rootfs is read-only so the drop-in is reboot-scoped.
trace-dropin:
  @bash -c 'source scripts/device.sh && device_ssh "mkdir -p /run/systemd/system/bridgething.service.d && printf \"[Service]\nEnvironment=RUST_LOG=bridgething=trace,bridgething::ws::connection::send=debug,bridgething::net=debug,libbridgething=trace,bridgething_iap2=trace,bridgething_mfi=trace\n\" > /run/systemd/system/bridgething.service.d/zz-trace.conf && systemctl daemon-reload && systemctl restart bridgething.service"'

# Drop the trace override and restart on the normal log level.
trace-off:
  @bash -c 'source scripts/device.sh && device_ssh "rm -rf /run/systemd/system/bridgething.service.d && systemctl daemon-reload && systemctl restart bridgething.service"'

# Tunnel chromium's CDP socket from the device's 127.0.0.1:9222 to the host.
cdp port="9222":
  scripts/bridgething-cdp {{port}}

# Build, push, and tail logs in one shot.
iter: push logs

# --- MFi dev proxy (dev-image only) ---

# Stop bridgething + ALS (via systemd Conflicts=) and start the i2c-3 proxy.
mfi-proxy-up:
  @bash -c 'source scripts/device.sh && device_ssh "systemctl start bridgething-mfi-proxy.service"'

# Stop the proxy and bring bridgething + ALS back up.
mfi-proxy-down:
  @bash -c 'source scripts/device.sh && device_ssh "systemctl stop bridgething-mfi-proxy.service; systemctl start bridgething-als.service bridgething.service"'

# Tail the proxy's journal.
mfi-proxy-logs:
  @bash -c 'source scripts/device.sh && device_ssh journalctl -fu bridgething-mfi-proxy.service'

# --- wasm ---

# Build the browser-targeted core crates
build-wasm *args:
  cargo build --target {{wasm_target}} {{wasm_crates}} {{args}}

# Compile the core into packages/browser, which is what @bridgething/browser ships
pack-wasm:
  cd {{justfile_directory()}}/packages/browser && bun run wasm

# --- node addon ---

# Build the n-api addon in place
build-napi variant="debug":
  cd {{napi_dir}} && bun run {{ if variant == "release" { "napi:build" } else { "napi:build:debug" } }}

# --- Misc ---

tokei:
  tokei -t Rust,TypeScript,TSX,Kotlin,Swift
