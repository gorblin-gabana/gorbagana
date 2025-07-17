#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Solana Build Script
# =============================================================================
# This script handles the complete Solana build process:
# 1. System dependencies installation
# 2. Rust installation and configuration
# 3. System tuning for Solana
# 4. Solana compilation with CPU compatibility
# =============================================================================

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m' # No Color

print_header() {
    echo -e "${BOLD}${BLUE}🔨 =============================================="
    echo -e "   Solana Build Script"
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

# Extract user info
SETUP_USER=$(logname 2>/dev/null || echo "${SUDO_USER:-gorblin}")
USER_HOME=$(eval echo "~$SETUP_USER")

# Get repository path
REPO_PATH="${1:-$(pwd)}"
if [ ! -f "$REPO_PATH/Cargo.toml" ]; then
    print_error "Invalid repository path: $REPO_PATH/Cargo.toml not found"
    print_info "Usage: sudo $0 [repository_path]"
    exit 1
fi

BINARIES_PATH="$REPO_PATH/target/release"

print_header
print_info "Repository: $REPO_PATH"
print_info "User: $SETUP_USER"
print_info "Binaries will be built in: $BINARIES_PATH"

# Apply comprehensive system tuning
print_info "Setting up comprehensive system tuning for Solana..."

# Backup existing sysctl.conf
cp /etc/sysctl.conf /etc/sysctl.conf.backup-$(date +%Y%m%d-%H%M%S) 2>/dev/null || true

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

# Configure systemd limits (Ubuntu uses system.conf.d directory)
mkdir -p /etc/systemd/system.conf.d
cat > /etc/systemd/system.conf.d/solana-limits.conf << 'EOF'
[Manager]
DefaultLimitNOFILE=1000000
DefaultLimitNPROC=1000000
DefaultLimitMEMLOCK=infinity
EOF

# Also configure user limits
cat > /etc/security/limits.d/solana.conf << 'EOF'
# Solana validator limits
* soft nofile 1000000
* hard nofile 1000000
* soft nproc 1000000
* hard nproc 1000000
* soft memlock unlimited
* hard memlock unlimited
EOF

# Reload systemd to apply changes
systemctl daemon-reload

print_success "System tuning applied successfully"

# Update system packages
print_info "Updating system and installing build dependencies..."
apt-get update -qq && apt-get upgrade -y -qq

# Install build dependencies
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
    ca-certificates \
    htop \
    iotop

print_success "Build dependencies installed successfully"

# Install/Update Rust for root
print_info "Installing Rust for root..."
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

print_success "Solana binaries are now globally accessible"

# Set proper ownership of repository
chown -R "$SETUP_USER:$SETUP_USER" "$REPO_PATH"

print_success "🎉 Solana build completed successfully!"
print_info ""
print_info "🔧 Solana binaries are available:"
print_info "   solana-keygen --version"
print_info "   solana-validator --version"
print_info "   solana-genesis --version"
print_info ""
print_info "📁 Repository owned by: $SETUP_USER"
print_info "💾 Binaries location: $BINARIES_PATH"
print_info ""
print_warning "⚠️  Next steps:"
print_warning "   1. Log out and back in to apply system limits"
print_warning "   2. Run validator setup scripts as user: $SETUP_USER"
