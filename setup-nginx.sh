#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m' # No Color

print_header() {
    echo -e "${BOLD}${BLUE}🌐 =============================================="
    echo -e "   Nginx & SSL Setup for Solana RPC"
    echo -e "======================================================== ${NC}"
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

# Configuration variables
SOLANA_RPC_PORT=8899
SOLANA_WS_PORT=8900
NGINX_RATE_LIMIT="100r/m"
NGINX_BURST_LIMIT=20

# Interactive configuration
configure_setup() {
    print_header
    print_info "This script will set up nginx reverse proxy for your Solana RPC endpoint"
    print_info ""
    
    # Get domain name
    while true; do
        read -p "🌐 Enter your domain name (e.g., rpc.mydomain.com) or 'skip' for IP-only setup: " DOMAIN
        
        if [ "$DOMAIN" = "skip" ]; then
            DOMAIN=""
            USE_SSL=false
            break
        elif [[ "$DOMAIN" =~ ^[a-zA-Z0-9][a-zA-Z0-9.-]*[a-zA-Z0-9]\.[a-zA-Z]{2,}$ ]]; then
            USE_SSL=true
            break
        else
            print_error "Invalid domain format. Please try again."
        fi
    done
    
    # Get email for SSL if domain is provided
    if [ -n "$DOMAIN" ]; then
        read -p "📧 Enter email for SSL certificate (default: admin@$DOMAIN): " EMAIL
        EMAIL="${EMAIL:-admin@$DOMAIN}"
    fi
    
    # Get Solana RPC configuration
    read -p "🔌 Solana RPC port (default: $SOLANA_RPC_PORT): " input_rpc_port
    SOLANA_RPC_PORT="${input_rpc_port:-$SOLANA_RPC_PORT}"
    
    read -p "🔌 Solana WebSocket port (default: $SOLANA_WS_PORT): " input_ws_port
    SOLANA_WS_PORT="${input_ws_port:-$SOLANA_WS_PORT}"
    
    # Rate limiting configuration
    read -p "🚦 Rate limit (requests per minute, default: ${NGINX_RATE_LIMIT%r/m}): " input_rate
    if [ -n "$input_rate" ]; then
        NGINX_RATE_LIMIT="${input_rate}r/m"
    fi
    
    read -p "💥 Burst limit (default: $NGINX_BURST_LIMIT): " input_burst
    NGINX_BURST_LIMIT="${input_burst:-$NGINX_BURST_LIMIT}"
    
    # Confirm configuration
    print_info ""
    print_info "📋 Configuration Summary:"
    if [ -n "$DOMAIN" ]; then
        print_info "   Domain: $DOMAIN"
        print_info "   Email: $EMAIL"
        print_info "   SSL: Enabled"
    else
        print_info "   Domain: IP-only setup"
        print_info "   SSL: Disabled"
    fi
    print_info "   Solana RPC Port: $SOLANA_RPC_PORT"
    print_info "   Solana WebSocket Port: $SOLANA_WS_PORT"
    print_info "   Rate Limit: $NGINX_RATE_LIMIT"
    print_info "   Burst Limit: $NGINX_BURST_LIMIT"
    print_info ""
    
    read -p "Continue with this configuration? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        print_info "Setup cancelled."
        exit 0
    fi
}

# Install nginx and dependencies
install_dependencies() {
    print_info "Installing nginx and dependencies..."
    
    apt-get update -qq
    apt-get install -y -qq \
        nginx \
        ufw \
        curl \
        jq
    
    if [ "$USE_SSL" = true ]; then
        apt-get install -y -qq \
            certbot \
            python3-certbot-nginx
    fi
    
    print_success "Dependencies installed"
}

# Configure UFW firewall
configure_firewall() {
    print_info "Configuring UFW firewall..."
    
    # Enable UFW
    ufw --force enable
    
    # Set default policies
    ufw default deny incoming
    ufw default allow outgoing
    
    # Allow SSH
    ufw allow ssh
    
    # Allow HTTP/HTTPS
    ufw allow 80/tcp comment 'HTTP'
    if [ "$USE_SSL" = true ]; then
        ufw allow 443/tcp comment 'HTTPS'
    fi
    
    # Allow Solana ports
    ufw allow "$SOLANA_RPC_PORT/tcp" comment 'Solana RPC'
    ufw allow "$SOLANA_WS_PORT/tcp" comment 'Solana WebSocket'
    
    # Allow Solana gossip (UDP range for validator communication)
    ufw allow 1024:65535/udp comment 'Solana Gossip'
    
    # Show status
    print_success "Firewall configured"
    ufw status numbered
}

# Create nginx configuration
create_nginx_config() {
    print_info "Creating nginx configuration..."
    
    local server_name
    if [ -n "$DOMAIN" ]; then
        server_name="$DOMAIN"
    else
        server_name="_"
    fi
    
    # Remove default nginx site
    rm -f /etc/nginx/sites-enabled/default
    
    # Create Solana RPC configuration
    cat > /etc/nginx/sites-available/solana-rpc << EOF
# Rate limiting zones
limit_req_zone \$binary_remote_addr zone=rpc:10m rate=$NGINX_RATE_LIMIT;
limit_req_zone \$binary_remote_addr zone=ws:10m rate=200r/m;

# Upstream for Solana RPC
upstream solana_rpc {
    server 127.0.0.1:$SOLANA_RPC_PORT;
    keepalive 32;
}

# Upstream for Solana WebSocket
upstream solana_ws {
    server 127.0.0.1:$SOLANA_WS_PORT;
    keepalive 32;
}

server {
    listen 80;
    server_name $server_name;
    
    # Security headers
    add_header X-Frame-Options DENY always;
    add_header X-Content-Type-Options nosniff always;
    add_header X-XSS-Protection "1; mode=block" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    
    # WebSocket location (must be before main location)
    location /ws {
        limit_req zone=ws burst=50 nodelay;
        
        # WebSocket proxy configuration
        proxy_pass http://solana_ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        
        # WebSocket specific timeouts
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 3600s; # 1 hour for long-lived connections
        
        # Disable buffering for WebSocket
        proxy_buffering off;
        
        # CORS for WebSocket
        add_header Access-Control-Allow-Origin * always;
        add_header Access-Control-Allow-Methods "GET, POST, OPTIONS" always;
        add_header Access-Control-Allow-Headers "Content-Type, Authorization, Upgrade, Connection" always;
    }
    
    # Main RPC location
    location / {
        limit_req zone=rpc burst=$NGINX_BURST_LIMIT nodelay;
        
        # CORS headers for web3 applications
        add_header Access-Control-Allow-Origin * always;
        add_header Access-Control-Allow-Methods "GET, POST, OPTIONS" always;
        add_header Access-Control-Allow-Headers "Content-Type, Authorization" always;
        
        # Handle preflight requests
        if (\$request_method = 'OPTIONS') {
            add_header Access-Control-Allow-Origin * always;
            add_header Access-Control-Allow-Methods "GET, POST, OPTIONS" always;
            add_header Access-Control-Allow-Headers "Content-Type, Authorization" always;
            add_header Access-Control-Max-Age 3600;
            add_header Content-Type text/plain;
            add_header Content-Length 0;
            return 204;
        }
        
        # Proxy to local Solana RPC
        proxy_pass http://solana_rpc;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        
        # Timeout settings for blockchain operations
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 60s;
        
        # Buffer settings
        proxy_buffering on;
        proxy_buffer_size 8k;
        proxy_buffers 8 8k;
        proxy_busy_buffers_size 16k;
    }
    
    # Health check endpoint
    location /health {
        access_log off;
        return 200 "healthy\\n";
        add_header Content-Type text/plain always;
    }
    
    # Nginx status (local only)
    location /nginx_status {
        stub_status on;
        access_log off;
        allow 127.0.0.1;
        deny all;
    }
    
    # Block access to sensitive paths
    location ~ /\\. {
        deny all;
    }
}
EOF

    # Enable the site
    ln -sf /etc/nginx/sites-available/solana-rpc /etc/nginx/sites-enabled/
    
    print_success "Nginx configuration created"
}

# Test and start nginx
start_nginx() {
    print_info "Testing nginx configuration..."
    
    if nginx -t; then
        print_success "Nginx configuration is valid"
    else
        print_error "Nginx configuration has errors"
        exit 1
    fi
    
    # Start and enable nginx
    systemctl enable nginx
    systemctl restart nginx
    
    print_success "Nginx started and enabled"
}


# Setup SSL with Let's Encrypt
setup_ssl() {
    if [ "$USE_SSL" != true ] || [ -z "$DOMAIN" ]; then
        return 0
    fi
    
    print_info "Setting up SSL certificate for $DOMAIN..."
    
    # Update nginx configuration with domain
    sed -i "s/server_name _;/server_name $DOMAIN;/" /etc/nginx/sites-available/solana-rpc
    systemctl reload nginx
    
    # Get SSL certificate
    if certbot --nginx \
        --non-interactive \
        --agree-tos \
        --email "$EMAIL" \
        --domains "$DOMAIN" \
        --redirect; then
        
        print_success "SSL certificate obtained and configured"
        
        # Setup auto-renewal
        print_info "Setting up automatic certificate renewal..."
        
        # Create renewal hook
        mkdir -p /etc/letsencrypt/renewal-hooks/deploy
        cat > /etc/letsencrypt/renewal-hooks/deploy/nginx-reload.sh << 'EOF'
#!/bin/bash
systemctl reload nginx
EOF
        chmod +x /etc/letsencrypt/renewal-hooks/deploy/nginx-reload.sh
        
        # Test automatic renewal
        if certbot renew --dry-run; then
            print_success "Automatic renewal test passed"
        else
            print_warning "Automatic renewal test failed"
        fi
        
    else
        print_error "Failed to obtain SSL certificate"
        return 1
    fi
}

# Create management script
create_management_script() {
    print_info "Creating nginx management script..."
    
    cat > /usr/local/bin/solana-nginx << 'EOF'
#!/usr/bin/env bash
set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_info() { echo -e "${BLUE}ℹ️  $1${NC}"; }
print_success() { echo -e "${GREEN}✅ $1${NC}"; }
print_warning() { echo -e "${YELLOW}⚠️  $1${NC}"; }
print_error() { echo -e "${RED}❌ $1${NC}"; }

show_status() {
    print_info "Nginx Status:"
    systemctl is-active nginx && print_success "Service: Active" || print_error "Service: Inactive"
    
    print_info "Listening Ports:"
    ss -tlnp | grep nginx || echo "No nginx ports found"
    
    print_info "SSL Certificates:"
    if [ -d /etc/letsencrypt/live ]; then
        ls -la /etc/letsencrypt/live/
    else
        echo "No SSL certificates found"
    fi
    
    print_info "Recent Access Logs:"
    tail -5 /var/log/nginx/access.log 2>/dev/null || echo "No access logs found"
}

test_endpoints() {
    print_info "Testing endpoints..."
    
    # Test health endpoint
    if curl -s -m 5 http://localhost/health > /dev/null; then
        print_success "Health endpoint: OK"
    else
        print_error "Health endpoint: Failed"
    fi
    
    # Test RPC endpoint
    if curl -s -m 5 -X POST -H "Content-Type: application/json" \
       -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
       http://localhost/ > /dev/null; then
        print_success "RPC endpoint: OK"
    else
        print_warning "RPC endpoint: Failed (validator may not be running)"
    fi
}

reload_config() {
    print_info "Reloading nginx configuration..."
    if nginx -t; then
        systemctl reload nginx
        print_success "Configuration reloaded"
    else
        print_error "Configuration test failed"
    fi
}

case "${1:-status}" in
    "status") show_status ;;
    "test") test_endpoints ;;
    "reload") reload_config ;;
    "restart") systemctl restart nginx; print_success "Nginx restarted" ;;
    "logs") tail -f /var/log/nginx/access.log ;;
    "error-logs") tail -f /var/log/nginx/error.log ;;
    *)
        echo "Usage: $0 {status|test|reload|restart|logs|error-logs}"
        echo "Commands:"
        echo "  status      - Show nginx status"
        echo "  test        - Test endpoints"
        echo "  reload      - Reload configuration"
        echo "  restart     - Restart nginx"
        echo "  logs        - Follow access logs"
        echo "  error-logs  - Follow error logs"
        ;;
esac
EOF

    chmod +x /usr/local/bin/solana-nginx
    print_success "Management script created at /usr/local/bin/solana-nginx"
}

# Show final summary
show_summary() {
    print_success "🎉 Nginx setup completed successfully!"
    print_info ""
    
    if [ -n "$DOMAIN" ]; then
        if [ "$USE_SSL" = true ]; then
            print_info "🌐 Your Solana RPC endpoints:"
            print_info "   HTTPS: https://$DOMAIN"
            print_info "   WebSocket: wss://$DOMAIN/ws"
            print_info "   HTTP (redirects): http://$DOMAIN"
        else
            print_info "🌐 Your Solana RPC endpoints:"
            print_info "   HTTP: http://$DOMAIN"
            print_info "   WebSocket: ws://$DOMAIN/ws"
        fi
    else
        SERVER_IP=$(curl -s -m 5 ifconfig.me 2>/dev/null || echo "YOUR_IP")
        print_info "🌐 Your Solana RPC endpoints:"
        print_info "   HTTP: http://$SERVER_IP"
        print_info "   WebSocket: ws://$SERVER_IP/ws"
    fi
    
    print_info "   Direct RPC: http://127.0.0.1:$SOLANA_RPC_PORT"
    print_info "   Direct WebSocket: ws://127.0.0.1:$SOLANA_WS_PORT"
    print_info ""
    
    print_info "🛡️  Security features:"
    print_info "   ✅ Rate limiting: $NGINX_RATE_LIMIT (burst: $NGINX_BURST_LIMIT)"
    print_info "   ✅ CORS headers for web3 apps"
    print_info "   ✅ WebSocket proxy support"
    if [ "$USE_SSL" = true ]; then
        print_info "   ✅ SSL/TLS encryption"
        print_info "   ✅ Auto-renewal configured"
    fi
    print_info "   ✅ UFW firewall configured"
    print_info ""
    
    print_info "📝 Test your endpoints:"
    print_info "   solana-nginx test"
    print_info "   curl -X POST -H 'Content-Type: application/json' \\"
    if [ -n "$DOMAIN" ] && [ "$USE_SSL" = true ]; then
        print_info "        -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}' \\"
        print_info "        https://$DOMAIN"
    else
        print_info "        -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}' \\"
        print_info "        http://$(curl -s -m 5 ifconfig.me 2>/dev/null || echo "YOUR_IP")"
    fi
    print_info ""
    
    print_info "🔧 Management commands:"
    print_info "   solana-nginx status     - Show status"
    print_info "   solana-nginx test       - Test endpoints"
    print_info "   solana-nginx logs       - Follow logs"
    print_info "   solana-nginx reload     - Reload config"
}

# Main execution
main() {
    configure_setup
    install_dependencies
    configure_firewall
    create_nginx_config
    start_nginx
    
    if ! setup_ssl; then
        print_error "SSL setup failed"
        exit 1
    fi
    
    create_management_script
    show_summary
}

# Run main function
main "$@"
