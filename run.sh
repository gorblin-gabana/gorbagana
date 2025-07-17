#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Solana Validator Complete Setup Script (Orchestrator)
# =============================================================================
# This script orchestrates the complete Solana validator setup by calling
# specialized scripts for different components:
# 1. Repository cloning/detection
# 2. Building Solana (build-solana.sh)
# 3. Setting up validator keys and directories
# 4. Optional nginx setup (setup-nginx.sh)
#
# Usage: sudo ./run.sh [optional_repo_path]
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
SETUP_USER=$(logname 2>/dev/null || echo "${SUDO_USER:-gorblin}")
USER_HOME=$(eval echo "~$SETUP_USER")

print_header
print_info "Starting complete Solana validator setup..."
print_info "User: $SETUP_USER"
print_info "Home: $USER_HOME"

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

print_success "Repository validated: $REPO_PATH"

# Ensure git ownership and configuration
print_info "Ensuring git repository ownership and configuration..."
git config --global --add safe.directory "$REPO_PATH"
chown -R "$SETUP_USER:$SETUP_USER" "$REPO_PATH"
print_success "Git ownership and configuration verified"

# Check and backup any existing validator data in ledger directory
if [ -d "$REPO_PATH/ledger" ] && [ -f "$REPO_PATH/ledger/genesis.bin" ]; then
    print_warning "Found validator data in ledger directory, backing up..."
    mv "$REPO_PATH/ledger" "$USER_HOME/ledger-backup-$(date +%Y%m%d-%H%M%S)"
    print_success "Validator data backed up"
fi

# Build Solana using the build script
print_info "Building Solana using build-solana.sh..."
if [ -f "$REPO_PATH/build-solana.sh" ]; then
    BUILD_SCRIPT="$REPO_PATH/build-solana.sh"
elif [ -f "./build-solana.sh" ]; then
    BUILD_SCRIPT="./build-solana.sh"
else
    print_error "build-solana.sh not found. Please ensure it's in the repository or current directory."
    exit 1
fi

chmod +x "$BUILD_SCRIPT"
if ! "$BUILD_SCRIPT" "$REPO_PATH"; then
    print_error "Solana build failed"
    exit 1
fi

print_success "Solana build completed successfully"

# Setup validator environment
print_info "Setting up validator environment..."

VALIDATOR_HOME="$REPO_PATH"
BINARIES_PATH="$REPO_PATH/target/release"

# Create validator directory structure
mkdir -p "$VALIDATOR_HOME/ledger" "$VALIDATOR_HOME/keys" "$VALIDATOR_HOME/fixtures"

# Generate validator keypairs
print_info "Generating validator keypairs..."
cd "$VALIDATOR_HOME"

# Ensure proper ownership of directories before key generation
chown -R "$SETUP_USER:$SETUP_USER" "$VALIDATOR_HOME/keys"

sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/identity-keypair.json --force"
sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/vote-account-keypair.json --force"
sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/stake-account-keypair.json --force"
sudo -u "$SETUP_USER" bash -c "cd '$VALIDATOR_HOME' && '$BINARIES_PATH/solana-keygen' new --no-bip39-passphrase -so keys/faucet-keypair.json --force"

print_success "Validator keypairs generated"

# Setup validator scripts
print_info "Configuring validator scripts..."

# Make validator.sh executable
chmod +x "$REPO_PATH/validator.sh"
chown "$SETUP_USER:$SETUP_USER" "$REPO_PATH/validator.sh"

# Make setup scripts executable
if [ -f "$REPO_PATH/setup-nginx.sh" ]; then
    chmod +x "$REPO_PATH/setup-nginx.sh"
fi

# Ensure proper ownership of validator directories
chown -R "$SETUP_USER:$SETUP_USER" "$REPO_PATH/ledger" "$REPO_PATH/keys" "$REPO_PATH/fixtures"

print_success "Validator environment setup completed"

# Ask about nginx setup
print_info ""
print_info "🌐 Nginx & SSL Setup (Optional)"
print_info "Would you like to set up nginx reverse proxy with SSL support?"
print_info "This provides:"
print_info "   • HTTPS access to your RPC endpoint"
print_info "   • WebSocket proxy support"
print_info "   • Rate limiting and security headers"
print_info "   • UFW firewall configuration"
print_info ""

read -p "Setup nginx now? (y/N): " -n 1 -r
echo

if [[ $REPLY =~ ^[Yy]$ ]]; then
    if [ -f "$REPO_PATH/setup-nginx.sh" ]; then
        print_info "Running nginx setup..."
        "$REPO_PATH/setup-nginx.sh"
    else
        print_warning "setup-nginx.sh not found, skipping nginx setup"
    fi
else
    print_info "Skipping nginx setup. You can run it later with:"
    print_info "   sudo $REPO_PATH/setup-nginx.sh"
fi

# Create quick start script
print_info "Creating quick start script..."
cat > "$REPO_PATH/start-validator.sh" << 'EOF'
#!/usr/bin/env bash
set -euo pipefail

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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VALIDATOR_SCRIPT="$SCRIPT_DIR/validator.sh"

# Check if running as the correct user (not root)
if [[ $EUID -eq 0 ]]; then
   print_error "Do not run validator as root. Run as regular user."
   exit 1
fi

# Check if validator script exists
if [ ! -f "$VALIDATOR_SCRIPT" ]; then
    print_error "Validator script not found: $VALIDATOR_SCRIPT"
    exit 1
fi

case "${1:-start}" in
    "start")
        print_info "Starting Solana validator..."
        "$VALIDATOR_SCRIPT"
        ;;
    "stop")
        print_info "Stopping validator..."
        if [ -f "$SCRIPT_DIR/ledger/production-validator.pid" ]; then
            pid=$(cat "$SCRIPT_DIR/ledger/production-validator.pid" 2>/dev/null || echo "")
            if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
                kill "$pid"
                print_success "Validator stopped"
            else
                print_warning "Validator not running"
            fi
        else
            print_warning "No PID file found"
        fi
        ;;
    "status")
        if [ -f "$SCRIPT_DIR/ledger/production-validator.pid" ]; then
            pid=$(cat "$SCRIPT_DIR/ledger/production-validator.pid" 2>/dev/null || echo "")
            if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
                print_success "Validator is running (PID: $pid)"
                # Test RPC
                if curl -s -m 5 -X POST -H "Content-Type: application/json" \
                   -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
                   "http://127.0.0.1:8899" > /dev/null 2>&1; then
                    print_success "RPC is responding"
                else
                    print_warning "RPC not responding"
                fi
            else
                print_error "Validator not running (stale PID)"
            fi
        else
            print_error "Validator not running"
        fi
        ;;
    "logs")
        LOG_FILE="$SCRIPT_DIR/ledger/solana-validator-identity.log"
        if [ -f "$LOG_FILE" ]; then
            tail -f "$LOG_FILE"
        else
            print_error "Log file not found: $LOG_FILE"
        fi
        ;;
    *)
        echo "Usage: $0 {start|stop|status|logs}"
        echo "Commands:"
        echo "  start   - Start validator"
        echo "  stop    - Stop validator"
        echo "  status  - Check status"
        echo "  logs    - Follow logs"
        ;;
esac
EOF

chmod +x "$REPO_PATH/start-validator.sh"
chown "$SETUP_USER:$SETUP_USER" "$REPO_PATH/start-validator.sh"

print_success "Quick start script created"

print_success "🎉 Complete Solana validator setup finished!"
print_info ""
print_info "📁 Repository: $REPO_PATH"
print_info "👤 Owner: $SETUP_USER"
print_info ""
print_info "🔄 To load environment variables in new sessions, run:"
print_info "   source ~/.bashrc"
print_info ""
print_info "🚀 To start the validator:"
print_info "   cd $REPO_PATH"
print_info "   ./start-validator.sh start"
print_info ""
print_info "📋 Available scripts:"
print_info "   ./validator.sh            - Direct validator start (original)"
print_info "   ./start-validator.sh      - Enhanced validator management"
print_info "   ./build-solana.sh         - Rebuild Solana binaries"
print_info "   ./setup-nginx.sh          - Setup nginx with SSL/domains"
print_info ""
print_info "🔧 Solana binaries are available globally:"
print_info "   solana-keygen --version"
print_info "   solana-validator --version"
print_info "   solana-genesis --version"
print_info ""
print_info "🌐 Your RPC endpoints:"
print_info "   Direct: http://127.0.0.1:8899"
if command -v nginx &> /dev/null && systemctl is-active nginx &> /dev/null; then
    print_info "   Nginx Proxy: http://$(curl -s -m 5 ifconfig.me 2>/dev/null || echo "YOUR_IP")"
fi
print_info ""
print_warning "⚠️  Next steps:"
print_warning "   1. Log out and back in to apply system limits"
print_warning "   2. Run './start-validator.sh start' as user: $SETUP_USER"
if [ -f "$REPO_PATH/setup-nginx.sh" ]; then
    print_warning "   3. Optionally setup domain: sudo ./setup-nginx.sh"
fi