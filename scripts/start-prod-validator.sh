#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

LEDGER_DIR="./ledger"
KEYS_DIR="./keys"

IDENTITY_KEY="$KEYS_DIR/identity-keypair.json"
VOTE_KEY="$KEYS_DIR/vote-account-keypair.json"
STAKE_KEY="$KEYS_DIR/stake-account-keypair.json"

LOG_FILE="$LEDGER_DIR/solana-validator-$(basename "$IDENTITY_KEY" .json).log"
PID_FILE="$LEDGER_DIR/production-validator.pid"
ROTATE_CONF="/etc/logrotate.d/solana-validator"

echo "🚀 Starting production solana-validator from $LEDGER_DIR…"

nohup solana-validator \
  --ledger                        "$LEDGER_DIR" \
  --identity                      "$IDENTITY_KEY" \
  --vote-account                  "$VOTE_KEY" \
  --no-port-check \
  --no-wait-for-vote-to-start-leader \
  --limit-ledger-size             500000000 \
  --account-index                 program-id \
  --account-index                 spl-token-mint \
  --account-index                 spl-token-owner \
  --full-rpc-api \
  --rpc-bind-address              0.0.0.0 \
  --rpc-port                      8899 \
  --snapshot-interval-slots       500 \
  --use-snapshot-archives-at-startup always \
  >> "$LOG_FILE" 2>&1 &

echo $! > "$PID_FILE"
echo "✅ Production validator launched; PID=$(<"$PID_FILE")"
echo "   Logs → $LOG_FILE"

# logrotate stanza (only as root)
if [[ $EUID -eq 0 && ! -f "$ROTATE_CONF" ]]; then
  cat > "$ROTATE_CONF" <<EOF
$LEDGER_DIR/solana-validator-*.log {
    daily
    rotate 7
    compress
    missingok
    notifempty
    copytruncate
}
EOF
  echo "✅ logrotate installed at $ROTATE_CONF"
fi
