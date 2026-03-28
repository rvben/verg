#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTAINER_NAME="verg-e2e"
SSH_PORT=2222
SSH_KEY="$SCRIPT_DIR/.ssh/id_ed25519"
SSH_CONFIG="$SCRIPT_DIR/.ssh/config"
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[e2e]${NC} $*"; }
warn()  { echo -e "${YELLOW}[e2e]${NC} $*"; }
fail()  { echo -e "${RED}[e2e] FAIL:${NC} $*"; exit 1; }

cleanup() {
    info "Cleaning up..."
    docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
}
trap cleanup EXIT

# --- Build ---

info "Building verg (host)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" 2>&1 | tail -1

info "Building verg-agent for linux/amd64 (in Docker)..."
AGENT_BINARY="$PROJECT_DIR/target/e2e/verg-agent"
mkdir -p "$(dirname "$AGENT_BINARY")"
docker run --rm \
    -v "$PROJECT_DIR:/src" \
    -w /src \
    rust:1.90-slim \
    sh -c "cargo build --release --bin verg-agent 2>&1 | tail -3 && \
           cp target/release/verg-agent target/e2e/verg-agent"

if [ ! -f "$AGENT_BINARY" ]; then
    fail "verg-agent not found at $AGENT_BINARY. Docker build may have failed."
fi

VERG="$PROJECT_DIR/target/release/verg"

# --- SSH key setup ---

info "Setting up SSH keys..."
mkdir -p "$SCRIPT_DIR/.ssh"
if [ ! -f "$SSH_KEY" ]; then
    ssh-keygen -t ed25519 -f "$SSH_KEY" -N "" -q
fi

cat > "$SSH_CONFIG" <<EOF
Host verg-e2e
    HostName 127.0.0.1
    Port $SSH_PORT
    User root
    IdentityFile $SSH_KEY
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
EOF

# --- Docker ---

info "Building Docker image..."
docker build -t verg-e2e "$SCRIPT_DIR" -q

info "Starting container..."
docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
docker run -d \
    --name "$CONTAINER_NAME" \
    -p "$SSH_PORT:22" \
    -v "$SSH_KEY.pub:/root/.ssh/authorized_keys:ro" \
    verg-e2e

info "Copying agent binary into container..."
docker cp "$AGENT_BINARY" "$CONTAINER_NAME:/usr/local/bin/verg-agent"
docker exec "$CONTAINER_NAME" chmod +x /usr/local/bin/verg-agent

info "Waiting for SSH..."
for i in $(seq 1 30); do
    if ssh -F "$SSH_CONFIG" verg-e2e "echo ok" 2>/dev/null; then
        break
    fi
    if [ "$i" -eq 30 ]; then
        fail "SSH not ready after 30 seconds"
    fi
    sleep 1
done

# --- Pre-flight: write version stamp so verg skips binary push ---

info "Writing agent version stamp..."
VERSION=$(grep '^version' "$PROJECT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
info "  version: $VERSION"
ssh -F "$SSH_CONFIG" verg-e2e "mkdir -p /usr/local/share/verg && echo '$VERSION' > /usr/local/share/verg/version"

# --- Test 1: diff (dry-run) ---

info "Test 1: verg diff..."
DIFF_EXIT=0
DIFF_OUTPUT=$("$VERG" diff --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json 2>&1) || DIFF_EXIT=$?
if [ "$DIFF_EXIT" -ge 2 ]; then
    fail "diff exited with error code $DIFF_EXIT"
fi
if [ -z "$DIFF_OUTPUT" ]; then
    fail "diff returned empty output"
fi
echo "$DIFF_OUTPUT" | python3 -m json.tool > /dev/null 2>&1 || fail "diff output is not valid JSON: $DIFF_OUTPUT"
info "  diff returned valid JSON (exit $DIFF_EXIT)"

# --- Test 2: apply ---

info "Test 2: verg apply..."
APPLY_EXIT=0
APPLY_OUTPUT=$("$VERG" apply --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json 2>/dev/null) || APPLY_EXIT=$?
if [ "$APPLY_EXIT" -ge 2 ]; then
    fail "apply exited with error code $APPLY_EXIT"
fi
echo "$APPLY_OUTPUT" | python3 -m json.tool > /dev/null 2>&1 || fail "apply output is not valid JSON"

# Check that changes were made
CHANGED=$(echo "$APPLY_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['summary']['changed'])" 2>/dev/null)
if [ "$CHANGED" = "0" ]; then
    fail "apply reported 0 changes on first run"
fi
info "  apply made $CHANGED change(s)"

# --- Test 3: verify resources on target ---

info "Test 3: verifying resources..."

# Check jq installed
ssh -F "$SSH_CONFIG" verg-e2e "dpkg -s jq" > /dev/null 2>&1 || fail "jq not installed"
info "  jq: installed"

# Check file content
FILE_CONTENT=$(ssh -F "$SSH_CONFIG" verg-e2e "cat /tmp/verg-test.txt" 2>/dev/null)
if [ "$FILE_CONTENT" != "hello from verg" ]; then
    fail "file content mismatch: got '$FILE_CONTENT'"
fi
info "  /tmp/verg-test.txt: correct content"

# Check command marker
ssh -F "$SSH_CONFIG" verg-e2e "test -f /tmp/verg-marker" || fail "marker file not created"
info "  /tmp/verg-marker: exists"

# --- Test 4: idempotency ---

info "Test 4: idempotency (second apply)..."
APPLY2_EXIT=0
APPLY2_OUTPUT=$("$VERG" apply --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json 2>/dev/null) || APPLY2_EXIT=$?
if [ "$APPLY2_EXIT" -ge 2 ]; then
    fail "second apply exited with error code $APPLY2_EXIT"
fi
CHANGED2=$(echo "$APPLY2_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['summary']['changed'])" 2>/dev/null)
if [ "$CHANGED2" != "0" ]; then
    warn "  second apply reported $CHANGED2 change(s) — not fully idempotent"
    echo "$APPLY2_OUTPUT" | python3 -m json.tool
else
    info "  second apply: 0 changes (idempotent)"
fi

# --- Test 5: check ---

info "Test 5: verg check..."
CHECK_EXIT=0
"$VERG" check --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json > /dev/null 2>&1 || CHECK_EXIT=$?
if [ "$CHECK_EXIT" -ge 2 ]; then
    fail "check exited with failure code $CHECK_EXIT (expected 0 or 1)"
fi
info "  check exit code: $CHECK_EXIT"

# --- Test 6: changelog written ---

info "Test 6: changelog..."
LOG_COUNT=$(find "$SCRIPT_DIR/fixture/.verg/logs" -name "*.json" 2>/dev/null | wc -l | tr -d ' ')
if [ "$LOG_COUNT" -gt 0 ]; then
    info "  $LOG_COUNT log file(s) written"
else
    warn "  no changelog files found"
fi

# --- Done ---

echo ""
info "All e2e tests passed!"
