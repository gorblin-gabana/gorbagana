#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Solana Validator Complete Setup Script
# =============================================================================
# This script provides a one-click setup for a Solana validator:
# 1. Automatically clones gorbagana repository from GitHub if not present
# 2. Installs all dependencies (Rust, build tools, etc.)
# 3. Builds Solana binaries with CPU compatibility
# 4. Configures system settings for optimal performance
# 5. Creates validator keys and management scripts
# 6. Sets up a complete validator environment
#
# Usage: sudo ./deploy.sh [optional_repo_path]
# =============================================================================

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m' # No Color

print_header() {
    echo -e "${BOLD}${BLUE}🚀 =============================================="
    echo -e "   Solana Validator Complete Setup Script"
    echo -e "============================================== ${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ️  $1${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

# Check if running as root
if [[ $EUID -ne 0 ]]; then
   print_error "This script must be run as root. Please run with: sudo $0" 
   exit 1
fi

# Extract user info early
SETUP_USER=$(logname 2>/dev/null || echo "gorblin")
USER_HOME=$(eval echo "~$SETUP_USER")

# Detect or clone repository
REPO_URL="https://github.com/gorblin-gabana/gorbagana.git"
REPO_PATH=""

if [ $# -gt 0 ]; then
    REPO_PATH="$1"
    print_info "Using provided repository path: $REPO_PATH"
elif [ -d "./gorbagana" ]; then
    REPO_PATH="$(pwd)/gorbagana"
    print_info "Found gorbagana repository in current directory"
elif [ -d "../gorbagana" ]; then
    REPO_PATH="$(cd .. && pwd)/gorbagana"
    print_info "Found gorbagana repository in parent directory"
else
    print_info "Gorbagana repository not found locally, cloning from GitHub..."
    REPO_PATH="$(pwd)/gorbagana"
    
    # First, install essential dependencies including git and curl
    print_info "Installing essential dependencies (git, curl)..."
    apt-get update -qq
    apt-get install -y -qq git curl
    
    # Check internet connectivity
    print_info "Checking internet connectivity..."
    if ! curl -s --connect-timeout 10 https://github.com > /dev/null; then
        print_error "No internet connection. Please check your network and try again."
        exit 1
    fi
    
    # Configure git for safe operations
    git config --global --add safe.directory "$REPO_PATH"
    
    # Clone the repository
    print_info "Cloning gorbagana repository..."
    if git clone "$REPO_URL" "$REPO_PATH"; then
        print_success "Successfully cloned gorbagana repository"
        # Set proper ownership
        chown -R "$SETUP_USER:$SETUP_USER" "$REPO_PATH"
    else
        print_error "Failed to clone repository from $REPO_URL"
        print_error "Please check your internet connection and GitHub access"
        exit 1
    fi
fi

# Validate repository path
if [ ! -f "$REPO_PATH/Cargo.toml" ]; then
    print_error "Invalid repository path: $REPO_PATH/Cargo.toml not found"
    exit 1
fi

BINARIES_PATH="$REPO_PATH/target/release"

print_header
print_info "Binaries will be built in: $BINARIES_PATH"
print_info "Starting complete Solana validator setup..."
print_info "Repository: $REPO_PATH"
print_info "User: $SETUP_USER"
print_info "Home: $USER_HOME"

# Ensure git ownership and configuration
print_info "Ensuring git repository ownership and configuration..."
git config --global --add safe.directory "$REPO_PATH"
# Set ownership if not already set (handles both cloned and existing repos)
chown -R "$SETUP_USER:$SETUP_USER" "$REPO_PATH"
print_success "Git ownership and configuration verified"

# Check and backup any existing validator data in ledger directory
if [ -d "$REPO_PATH/ledger" ] && [ -f "$REPO_PATH/ledger/genesis.bin" ]; then
    print_warning "Found validator data in ledger directory, backing up..."
    mv "$REPO_PATH/ledger" "$USER_HOME/ledger-backup-$(date +%Y%m%d-%H%M%S)"
    print_success "Validator data backed up"
fi

# Restore the ledger source code if missing
if [ ! -f "$REPO_PATH/ledger/Cargo.toml" ]; then
    print_info "Restoring ledger source code..."
    cd "$REPO_PATH"
    
    # Check if ledger directory exists as source code
    if ! sudo -u "$SETUP_USER" git ls-files --error-unmatch ledger/Cargo.toml &>/dev/null; then
        print_warning "ledger/Cargo.toml not tracked in git, attempting checkout..."
        sudo -u "$SETUP_USER" git checkout HEAD -- ledger/ 2>/dev/null || true
    fi
    
    # If still missing, the repository might not have this directory
    if [ ! -f "$REPO_PATH/ledger/Cargo.toml" ]; then
        print_warning "ledger/Cargo.toml still missing, this might be normal for this repository"
        # Check what the build actually needs
        print_info "Checking workspace members..."
        if grep -q "ledger" "$REPO_PATH/Cargo.toml"; then
            print_error "Repository expects ledger crate but it's missing"
            exit 1
        else
            print_info "Repository doesn't require ledger crate, proceeding..."
        fi
    fi
    
    print_success "Ledger source code restored"
fi

# Apply comprehensive system tuning
print_info "Setting up comprehensive system tuning for Solana validator..."

# Backup existing sysctl.conf
cp /etc/sysctl.conf /etc/sysctl.conf.backup-$(date +%Y%m%d-%H%M%S)

# Apply network and system optimizations
cat >> /etc/sysctl.conf << 'EOF'
# Solana Validator Network Optimizations
net.core.rmem_default = 134217728
net.core.rmem_max = 134217728
net.core.wmem_default = 134217728
net.core.wmem_max = 134217728
net.core.netdev_max_backlog = 5000
net.ipv4.udp_rmem_min = 8192
net.ipv4.udp_wmem_min = 8192

# File system optimizations
vm.max_map_count = 1000000
fs.nr_open = 1000000
EOF

# Apply settings immediately
sysctl -p

# Configure systemd limits
if [ -f /etc/systemd/system.conf ]; then
    sed -i 's/#DefaultLimitNOFILE=/DefaultLimitNOFILE=1000000/' /etc/systemd/system.conf
fi

print_success "Comprehensive system tuning applied successfully"

# Update system packages
print_info "Updating system and installing dependencies..."
apt-get update -qq && apt-get upgrade -y -qq

# Install comprehensive dependencies
apt-get install -y -qq \
    curl \
    git \
    gcc \
    g++ \
    make \
    pkg-config \
    libssl-dev \
    libudev-dev \
    zlib1g-dev \
    llvm \
    clang \
    cmake \
    libprotobuf-dev \
    protobuf-compiler \
    python3-pip \
    jq \
    tmux \
    lsof \
    nodejs \
    npm \
    wget \
    ca-certificates

print_success "Dependencies installed successfully"

# Install/Update Rust
print_info "Installing Rust..."
if ! command -v rustc &> /dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source /root/.cargo/env
else
    source /root/.cargo/env
fi

rustup update
rustup component add rustfmt

# Install Rust for the user as well
print_info "Installing Rust for user $SETUP_USER..."
sudo -u "$SETUP_USER" bash -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'
sudo -u "$SETUP_USER" bash -c 'source ~/.cargo/env && rustup update && rustup component add rustfmt'

print_success "Rust installed and configured"

# Setup environment variables
print_info "Setting up environment variables..."

# Create cargo cache directory
mkdir -p /root/.cache/release

# Setup environment for root
cat >> /root/.bashrc << EOF
# Solana Development Environment
export CARGO_TARGET_DIR=/root/.cache/release
export PATH="$BINARIES_PATH:/usr/local/bin:\$PATH"
source /root/.cargo/env
EOF

# Setup environment for the user
sudo -u "$SETUP_USER" mkdir -p "$USER_HOME/.cache/release"
cat >> "$USER_HOME/.bashrc" << EOF
# Solana Development Environment  
export CARGO_TARGET_DIR=$USER_HOME/.cache/release
export PATH="$BINARIES_PATH:/usr/local/bin:\$PATH"
source ~/.cargo/env
EOF

chown "$SETUP_USER:$SETUP_USER" "$USER_HOME/.bashrc"

print_success "Environment variables configured"

# Build Solana with CPU compatibility
print_info "Building Solana with CPU compatibility fixes..."

cd "$REPO_PATH"

# Set build environment with CPU compatibility
export CARGO_TARGET_DIR="$REPO_PATH/target"
export RUSTFLAGS="-C target-cpu=generic"

# Build as the repository owner to avoid permission issues
print_info "Building Solana binaries (this may take 15-30 minutes)..."
sudo -u "$SETUP_USER" bash -c "
    export CARGO_TARGET_DIR='$REPO_PATH/target'
    export RUSTFLAGS='-C target-cpu=generic'
    source ~/.cargo/env
    cd '$REPO_PATH'
    cargo build --release --bin solana-validator --bin solana-keygen --bin solana-genesis
"

# Verify binaries were created
REQUIRED_BINARIES=("solana-validator" "solana-keygen" "solana-genesis")
for binary in "${REQUIRED_BINARIES[@]}"; do
    if [ ! -f "$BINARIES_PATH/$binary" ]; then
        print_error "Failed to build $binary"
        exit 1
    fi
done

print_success "Solana binaries built successfully with CPU compatibility"

# Create symlinks for global access
print_info "Creating symlinks for global access to Solana binaries..."
ln -sf "$BINARIES_PATH/solana-validator" /usr/local/bin/solana-validator
ln -sf "$BINARIES_PATH/solana-keygen" /usr/local/bin/solana-keygen
ln -sf "$BINARIES_PATH/solana-genesis" /usr/local/bin/solana-genesis

# Update PATH in current session
export PATH="$BINARIES_PATH:$PATH"
print_success "Solana binaries are now globally accessible"

# Create validator directory structure
print_info "Setting up validator environment..."

VALIDATOR_HOME="$USER_HOME"
mkdir -p "$VALIDATOR_HOME/ledger-data" "$VALIDATOR_HOME/keys" "$VALIDATOR_HOME/fixtures"

# Generate validator keypairs
print_info "Generating validator keypairs..."
cd "$VALIDATOR_HOME"

# Ensure proper ownership of directories before key generation
chown -R "$SETUP_USER:$SETUP_USER" "$VALIDATOR_HOME/keys"

sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/identity-keypair.json --force"
sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/vote-account-keypair.json --force"
sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/stake-account-keypair.json --force"
sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/faucet-keypair.json --force"

# Create enhanced validator management script
print_info "Creating validator management script..."
cat > "$USER_HOME/run-validator.sh" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail

# Configuration - Auto-detect paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMUX_SESSION_NAME="solana-validator"
LEDGER_DIR="$SCRIPT_DIR/ledger-data"
KEYS_DIR="$SCRIPT_DIR/keys"
FIXTURES_DIR="$SCRIPT_DIR/fixtures"
INITIAL_STAKE=500000000000000000
RPC_PORT=8890
RPC_HOST="0.0.0.0"

# Auto-detect Solana binaries path
POSSIBLE_PATHS=(
    "$SCRIPT_DIR/gorbagana/target/release"
    "$(dirname "$SCRIPT_DIR")/gorbagana/target/release"
    "/home/*/gorbagana/target/release"
)

SOLANA_BIN_PATH=""
for path in "${POSSIBLE_PATHS[@]}"; do
    if [ -f "$path/solana-validator" ]; then
        SOLANA_BIN_PATH="$path"
        break
    fi
done

if [ -z "$SOLANA_BIN_PATH" ]; then
    echo "❌ Could not find Solana binaries. Please ensure setup completed successfully."
    exit 1
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_info() { echo -e "${BLUE}ℹ️  $1${NC}"; }
print_success() { echo -e "${GREEN}✅ $1${NC}"; }
print_warning() { echo -e "${YELLOW}⚠️  $1${NC}"; }
print_error() { echo -e "${RED}❌ $1${NC}"; }

# Check dependencies
check_deps() {
    for cmd in tmux lsof curl; do
        if ! command -v "$cmd" &> /dev/null; then
            print_error "$cmd not found. Please run setup script first."
            exit 1
        fi
    done
}

# Verify required files
check_requirements() {
    for dir in "$LEDGER_DIR" "$KEYS_DIR" "$FIXTURES_DIR"; do
        mkdir -p "$dir"
    done

    IDENTITY_KEY="$KEYS_DIR/identity-keypair.json"
    VOTE_KEY="$KEYS_DIR/vote-account-keypair.json"
    STAKE_KEY="$KEYS_DIR/stake-account-keypair.json"
    FAUCET_KEY="$KEYS_DIR/faucet-keypair.json"

    for key in "$IDENTITY_KEY" "$VOTE_KEY" "$STAKE_KEY" "$FAUCET_KEY"; do
        if [ ! -f "$key" ]; then
            print_error "Key file $key not found. Please run setup script first."
            exit 1
        fi
    done
}

# Get public keys
get_public_keys() {
    ID_PUB="$("$SOLANA_BIN_PATH/solana-keygen" pubkey "$IDENTITY_KEY")"
    VOTE_PUB="$("$SOLANA_BIN_PATH/solana-keygen" pubkey "$VOTE_KEY")"
    STAKE_PUB="$("$SOLANA_BIN_PATH/solana-keygen" pubkey "$STAKE_KEY")"
    FAUCET_PUB="$("$SOLANA_BIN_PATH/solana-keygen" pubkey "$FAUCET_KEY")"
    
    print_info "Identity: $ID_PUB"
    print_info "Vote: $VOTE_PUB"
    print_info "Stake: $STAKE_PUB"
    print_info "Faucet: $FAUCET_PUB"
}

# Cleanup existing processes
cleanup_processes() {
    print_info "Cleaning up existing validator processes..."
    
    pkill -f "solana-validator" 2>/dev/null || true
    sleep 2
    
    if tmux has-session -t "$TMUX_SESSION_NAME" 2>/dev/null; then
        tmux kill-session -t "$TMUX_SESSION_NAME" || true
        sleep 1
    fi
    
    if lsof -i :$RPC_PORT > /dev/null 2>&1; then
        print_warning "Port $RPC_PORT in use, attempting to free..."
        sudo lsof -t -i :$RPC_PORT | xargs sudo kill -9 2>/dev/null || true
        sleep 2
    fi
}

# Create genesis
create_genesis() {
    print_info "Creating genesis configuration..."
    rm -rf "$LEDGER_DIR"/*
    
    "$SOLANA_BIN_PATH/solana-genesis" \
      --ledger "$LEDGER_DIR" \
      --inflation pico \
      --bootstrap-validator "$ID_PUB" "$VOTE_PUB" "$STAKE_PUB" \
      --bootstrap-validator-lamports 500000000000000000 \
      --bootstrap-validator-stake-lamports $INITIAL_STAKE \
      --faucet-pubkey "$FAUCET_PUB" \
      --faucet-lamports 10000000 \
      --hashes-per-tick 100 \
      --cluster-type development
}

# Start validator
start_validator() {
    print_info "Starting validator in tmux session: $TMUX_SESSION_NAME"
    
    tmux new-session -d -s "$TMUX_SESSION_NAME" -c "$SCRIPT_DIR"
    tmux send-keys -t "$TMUX_SESSION_NAME" "export RUST_LOG=solana=info,solana_core=debug" C-m
    tmux send-keys -t "$TMUX_SESSION_NAME" "export PATH=\"$SOLANA_BIN_PATH:\$PATH\"" C-m
    
    tmux send-keys -t "$TMUX_SESSION_NAME" "\"$SOLANA_BIN_PATH/solana-validator\" \\
  --ledger                        \"$LEDGER_DIR\" \\
  --identity                      \"$IDENTITY_KEY\" \\
  --vote-account                  \"$VOTE_KEY\" \\
  --no-port-check \\
  --no-wait-for-vote-to-start-leader \\
  --limit-ledger-size             10000000000 \\
  --full-rpc-api \\
  --enable-rpc-transaction-history \\
  --account-index                 program-id \\
  --account-index                 spl-token-owner \\
  --account-index                 spl-token-mint \\
  --rpc-bind-address              $RPC_HOST \\
  --rpc-port                      $RPC_PORT \\
  --snapshot-interval-slots       100 \\
  --use-snapshot-archives-at-startup always \\
  --log -" C-m
}

# Check status
check_status() {
    if ! tmux has-session -t "$TMUX_SESSION_NAME" 2>/dev/null; then
        print_error "Tmux session not found"
        return 1
    fi
    
    if ! pgrep -f "solana-validator" > /dev/null; then
        print_error "Validator process not running"
        return 1
    fi
    
    if curl -s -m 5 -X POST -H "Content-Type: application/json" \
       -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
       "http://127.0.0.1:$RPC_PORT" > /dev/null 2>&1; then
        print_success "RPC is responding on port $RPC_PORT"
    else
        print_warning "RPC not responding yet"
    fi
    
    print_success "Validator running in session: $TMUX_SESSION_NAME"
    return 0
}

# Test RPC
test_rpc() {
    print_info "Testing RPC endpoints..."
    for endpoint in getHealth getSlot getVersion; do
        print_info "Testing $endpoint..."
        curl -s -m 10 -X POST -H "Content-Type: application/json" \
             -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$endpoint\"}" \
             "http://127.0.0.1:$RPC_PORT" | jq . 2>/dev/null || echo "Response received"
    done
}

# Show logs
show_logs() {
    if tmux has-session -t "$TMUX_SESSION_NAME" 2>/dev/null; then
        tmux capture-pane -t "$TMUX_SESSION_NAME" -p | tail -50
    else
        print_error "Session not found"
    fi
}

# Main command handling
case "${1:-start}" in
    "start")
        check_deps
        check_requirements
        get_public_keys
        cleanup_processes
        create_genesis
        start_validator
        sleep 10
        if check_status; then
            print_success "🎉 Validator started successfully!"
            print_info "🌐 External RPC: http://$(curl -s -m 5 ifconfig.me 2>/dev/null || echo "YOUR_IP"):$RPC_PORT"
            print_info "🏠 Local RPC: http://127.0.0.1:$RPC_PORT"
            print_info "📺 Session: $TMUX_SESSION_NAME"
        fi
        ;;
    "stop") cleanup_processes; print_success "Validator stopped" ;;
    "restart") $0 stop; sleep 3; $0 start ;;
    "status") check_status ;;
    "logs") show_logs ;;
    "attach") tmux attach-session -t "$TMUX_SESSION_NAME" 2>/dev/null || print_error "Session not found" ;;
    "test") test_rpc ;;
    *) 
        echo "Usage: $0 {start|stop|restart|status|logs|attach|test}"
        echo "Commands:"
        echo "  start    - Start validator (default)"
        echo "  stop     - Stop validator"
        echo "  restart  - Restart validator"
        echo "  status   - Check status"
        echo "  logs     - Show logs"
        echo "  attach   - Attach to session"
        echo "  test     - Test RPC"
        ;;
esac
EOF

# Make script executable and set ownership
chmod +x "$USER_HOME/run-validator.sh"
chown "$SETUP_USER:$SETUP_USER" "$USER_HOME/run-validator.sh"
chown -R "$SETUP_USER:$SETUP_USER" "$USER_HOME/ledger-data" "$USER_HOME/keys" "$USER_HOME/fixtures"

# Verify installation
print_info "Verifying installation..."
if command -v solana-keygen &> /dev/null; then
    print_success "✅ solana-keygen is accessible globally"
else
    print_warning "⚠️  solana-keygen not in PATH, but available via symlink"
fi

print_success "🎉 Complete Solana validator setup finished!"
print_info ""
print_info "🔄 To load environment variables in new sessions, run:"
print_info "   source ~/.bashrc"
print_info ""
print_info "🚀 To start the validator:"
print_info "   cd $USER_HOME"
print_info "   ./run-validator.sh start"
print_info ""
print_info "📋 Available commands:"
print_info "   ./run-validator.sh start    - Start validator"
print_info "   ./run-validator.sh stop     - Stop validator"
print_info "   ./run-validator.sh status   - Check status"
print_info "   ./run-validator.sh logs     - View logs"
print_info "   ./run-validator.sh test     - Test RPC"
print_info ""
print_info "🔧 Solana binaries are available:"
print_info "   solana-keygen --version"
print_info "   solana-validator --version"
print_info "   solana-genesis --version"
print_info ""
print_info "🌐 After starting, your RPC will be available at:"
print_info "   External: http://$(curl -s -m 5 ifconfig.me 2>/dev/null || echo "YOUR_IP"):8890"
print_info "   Local: http://127.0.0.1:8890"
print_info ""
print_warning "⚠️  Don't forget to configure your firewall for external access!"