# solana-accounts-cluster-bench

## Overview

This crate provides a benchmarking tool for Solana's accounts and RPC cluster performance. It is used to measure the throughput, latency, and correctness of various RPC endpoints and account operations in a simulated or real Solana cluster.

## Structure

```
src/
└── main.rs   # Main binary for running cluster and RPC benchmarks
```

## What This Crate Does
- Benchmarks RPC endpoints (getMultipleAccounts, getTransaction, etc.)
- Simulates account creation, transfers, and token operations
- Supports multi-threaded, multi-client benchmarking
- Measures latency, throughput, and error rates for cluster operations
- Provides utilities for airdrops, polling, and transaction tracking

## Where This Crate Is Imported
- This is a standalone benchmarking tool and is not imported as a library by other crates. It is invoked as a binary for performance and integration testing.

## What This Crate Imports
- `solana-client`, `solana-rpc-client`, `solana-transaction-status` (RPC and transaction APIs)
- `solana-clap-utils`, `solana-cli-config` (CLI and config helpers)
- `solana-pubkey`, `solana-keypair`, `solana-signature` (key and signature utilities)
- `clap`, `rayon`, `rand`, `log` (CLI, parallelism, randomness, logging)
- `spl-token` (token account operations)
- `std` (standard library)

## Example Usage

```sh
cargo run --release -p accounts-cluster-bench -- --threads 8 --iterations 1000 --rpc-url http://localhost:8899
```

## File Descriptions

- **main.rs**: Implements the CLI, argument parsing, and all benchmarking logic. Contains utilities for polling, airdrops, transaction tracking, and running various RPC and account operation benchmarks.

## Integration Points
- Used by Solana core developers and performance engineers to benchmark and tune the cluster and RPC endpoints.
- Not used in production or by other crates directly.

## Related Crates
- `solana-client`, `solana-rpc-client`: RPC and transaction APIs
- `solana-accounts-db`: Core storage engine
- `spl-token`: Token operations 