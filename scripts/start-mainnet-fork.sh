#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

DEV_LOG="ledger/dev-validator.log"
mkdir -p ledger

echo "⏳  Cloning fixtures…"
mucho clone

echo "🚀  Capturing snapshot at slot 500 and exiting…"
solana-test-validator \
  --ledger ./ledger \
  --limit-ledger-size 100000000 \
  --account-index program-id \
  --account-index spl-token-mint \
  --account-index spl-token-owner \
  --snapshot-interval-slots 500 \
  --halt-at-slot 500 \
  >> "$DEV_LOG" 2>&1

echo "✅  First snapshot written under ledger/snapshots/500/"

echo "🚀  Relaunching long-running dev-fork…"
mucho validator --ledger ./ledger \
  --limit-ledger-size 100000000 \
  --account-index program-id \
  --account-index spl-token-mint \
  --account-index spl-token-owner \
  --snapshot-interval-slots 500 \
  >> "$DEV_LOG" 2>&1 &

echo "✅  Dev fork ready; logs → $DEV_LOG"
