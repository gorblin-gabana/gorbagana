# solana-banking-bench

## Overview

This crate provides a benchmarking tool for Solana's banking stage and transaction processing pipeline. It is used to measure the throughput, latency, and contention characteristics of the banking stage, including transaction batching, account locking, and compute unit pricing.

## Structure

```
src/
└── main.rs   # Main binary for running banking stage and transaction pipeline benchmarks
```

## What This Crate Does
- Benchmarks the Solana banking stage (transaction processing pipeline)
- Simulates transfer and mint transactions with configurable contention
- Measures throughput, latency, and lock contention
- Supports different write lock contention scenarios (none, same batch, full)
- Outputs timing and performance statistics for each operation

## Where This Crate Is Imported
- This is a standalone benchmarking tool and is not imported as a library by other crates. It is invoked as a binary for performance testing.

## What This Crate Imports
- `solana-core`, `solana-runtime`, `solana-transaction`, `solana-transaction-status` (core transaction and runtime types)
- `solana-gossip`, `solana-ledger`, `solana-pubkey`, `solana-keypair` (network, ledger, and key utilities)
- `clap` (argument parsing)
- `rayon`, `rand`, `log` (parallelism, randomness, logging)
- `assert_matches` (test assertions)
- `std` (standard library)

## Example Usage

```sh
cargo run --release -p banking-bench -- --packets-per-batch 64 --batches-per-iteration 100
```

## File Descriptions

- **main.rs**: Implements the CLI, argument parsing, and all benchmarking logic. Simulates transaction batches, lock contention, and measures banking stage performance.

## Integration Points
- Used by Solana core developers and performance engineers to benchmark and tune the banking stage and transaction pipeline.
- Not used in production or by other crates directly.

## Related Crates
- `solana-core`: Core validator and banking stage logic
- `solana-runtime`: Transaction execution engine
- `solana-transaction`: Transaction types
- `solana-pubkey`: Public key utilities 