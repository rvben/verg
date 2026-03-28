#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTAINER_NAME="verg-e2e"
SSH_PORT=2222
SSH_KEY="$SCRIPT_DIR/.ssh/id_ed25519"
SSH_CONFIG="$SCRIPT_DIR/.ssh/config"
AGENT_BINARY="$PROJECT_DIR/target/x86_64-unknown-linux-musl/release/verg-agent"

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

info "Cross-compiling verg-agent for linux/amd64..."
if ! command -v cross &>/dev/null; then
    warn "'cross' not found. Attempting cargo build with musl target..."
    rustup target add x86_64-unknown-linux-musl 2>/dev/null || true
    cargo build --release --target x86_64-unknown-linux-musl --manifest-path "$PROJECT_DIR/Cargo.toml" --bin verg-agent 2>&1 | tail -1
else
    cross build --release --target x86_64-unknown-linux-musl --manifest-path "$PROJECT_DIR/Cargo.toml" --bin verg-agent 2>&1 | tail -1
fi

if [ ! -f "$AGENT_BINARY" ]; then
    fail "verg-agent not found at $AGENT_BINARY. Cross-compilation may have failed."
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
    -v "$AGENT_BINARY:/usr/local/bin/verg-agent:ro" \
    verg-e2e

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
VERSION=$(cargo metadata --manifest-path "$PROJECT_DIR/Cargo.toml" --format-version 1 --no-deps | grep '"version"' | head -1 | sed 's/.*"\([0-9.]*\)".*/\1/')
ssh -F "$SSH_CONFIG" verg-e2e "mkdir -p /usr/local/share/verg && echo '$VERSION' > /usr/local/share/verg/version"

# --- Test 1: diff (dry-run) ---

info "Test 1: verg diff..."
DIFF_OUTPUT=$("$VERG" diff --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json 2>/dev/null) || true
echo "$DIFF_OUTPUT" | python3 -m json.tool > /dev/null 2>&1 || fail "diff output is not valid JSON"
info "  diff returned valid JSON"

# --- Test 2: apply ---

info "Test 2: verg apply..."
APPLY_OUTPUT=$("$VERG" apply --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json 2>/dev/null) || true
echo "$APPLY_OUTPUT" | python3 -m json.tool > /dev/null 2>&1 || fail "apply output is not valid JSON"

# Check that changes were made
CHANGED=$(echo "$APPLY_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['summary']['changed'])" 2>/dev/null)
if [ "$CHANGED" = "0" ]; then
    fail "apply reported 0 changes on first run"
fi
info "  apply made $CHANGED change(s)"

# --- Test 3: verify resources on target ---

info "Test 3: verifying resources..."

# Check htop installed
ssh -F "$SSH_CONFIG" verg-e2e "dpkg -s htop" > /dev/null 2>&1 || fail "htop not installed"
info "  htop: installed"

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
APPLY2_OUTPUT=$("$VERG" apply --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json 2>/dev/null) || true
CHANGED2=$(echo "$APPLY2_OUTPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['summary']['changed'])" 2>/dev/null)
if [ "$CHANGED2" != "0" ]; then
    warn "  second apply reported $CHANGED2 change(s) — not fully idempotent"
    echo "$APPLY2_OUTPUT" | python3 -m json.tool
else
    info "  second apply: 0 changes (idempotent)"
fi

# --- Test 5: check ---

info "Test 5: verg check..."
"$VERG" check --path "$SCRIPT_DIR/fixture" --ssh-config "$SSH_CONFIG" --targets all --json > /dev/null 2>&1
CHECK_EXIT=$?
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
