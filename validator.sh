#!/usr/bin/env bash
set -euo pipefail

LEDGER_DIR="./ledger"
KEYS_DIR="./keys"
FIXTURES_DIR="./fixtures"
INITIAL_STAKE=500000000000000000         # 500M SOL

IDENTITY_KEY="$KEYS_DIR/identity-keypair.json"
VOTE_KEY="$KEYS_DIR/vote-account-keypair.json"
STAKE_KEY="$KEYS_DIR/stake-account-keypair.json"
ID_PUB="$(solana-keygen pubkey "$IDENTITY_KEY")"
VOTE_PUB="$(solana-keygen pubkey "$VOTE_KEY")"
STAKE_PUB="$(solana-keygen pubkey "$STAKE_KEY")"
FAUCET_PUB="$(solana-keygen pubkey "$KEYS_DIR/faucet-keypair.json")"

LOG_FILE="$LEDGER_DIR/solana-validator-$(basename "$IDENTITY_KEY" .json).log"
PID_FILE="$LEDGER_DIR/production-validator.pid"
ROTATE_CONF="/etc/logrotate.d/solana-validator"

echo "⏳ Creating genesis (initial stake: 1 SOL, supply:1B+)..."
solana-genesis \
  --ledger "$LEDGER_DIR" \
  --inflation pico \
  --bootstrap-validator "$ID_PUB" "$VOTE_PUB" "$STAKE_PUB" \
  --bootstrap-validator-lamports 500000000000000000 \
  --bootstrap-validator-stake-lamports $INITIAL_STAKE \
  --faucet-pubkey "$FAUCET_PUB" \
  --faucet-lamports 10000000 \
  --hashes-per-tick 100 \
  --cluster-type development

echo "✅ Genesis created."

echo "🚀 Starting production solana-validator from $LEDGER_DIR…"

nohup solana-validator \
  --ledger                        "$LEDGER_DIR" \
  --identity                      "$IDENTITY_KEY" \
  --vote-account                  "$VOTE_KEY" \
  --no-port-check \
  --no-wait-for-vote-to-start-leader \
  --limit-ledger-size             10000000000 \
  --full-rpc-api \
  --enable-rpc-transaction-history \
  --account-index                 program-id \
  --account-index                 spl-token-owner \
  --account-index                 spl-token-mint \
  --rpc-bind-address              0.0.0.0 \
  --rpc-port                      8899 \
  --snapshot-interval-slots       100 \
  --use-snapshot-archives-at-startup always \
  --log - \
  >> "$LOG_FILE" 2>&1 &

echo $! > "$PID_FILE"
echo "✅ Production validator launched; PID=$(<"$PID_FILE")"
echo "   Logs → $LOG_FILE"

# Wait for RPC to be ready
echo "⏳ Waiting for validator RPC to be ready..."
for i in {1..30}; do
  if solana cluster-version > /dev/null 2>&1; then
    echo "✅ Validator RPC is ready."
    break
  fi
  sleep 2
done

# Logrotate config (if running as root)
if [[ $EUID -eq 0 && ! -f "$ROTATE_CONF" ]]; then
  cat > "$ROTATE_CONF" <<EOF
$LEDGER_DIR/solana-validator-*.log {
    daily
    rotate 1
    compress
    missingok
    notifempty
    copytruncate
}
EOF
  echo "✅ logrotate installed at $ROTATE_CONF"
fi

echo "🎉 All done. Validator and stake accounts are set up and running."