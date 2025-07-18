# solana-accounts-bench

## Overview

This crate provides a benchmarking tool for Solana's accounts database. It is used to measure the performance of account creation, storage, and hash calculation in the Solana validator's core storage engine.

## Structure

```
src/
└── main.rs   # Main binary for running accounts database benchmarks
```

## What This Crate Does
- Benchmarks account creation, storage, and hash calculation
- Supports configurable number of slots, accounts, and iterations
- Can run in 'clean' mode to test account cleaning performance
- Outputs timing and performance statistics for each operation

## Where This Crate Is Imported
- This is a standalone benchmarking tool and is not imported as a library by other crates. It is invoked as a binary for performance testing.

## What This Crate Imports
- `solana-accounts-db` (core accounts database and utilities)
- `solana-epoch-schedule`, `solana-measure`, `solana-pubkey` (core Solana types)
- `clap` (argument parsing)
- `rayon` (parallelism)
- `log` (logging)
- `std` (standard library)

## Example Usage

```sh
cargo run --release -p accounts-bench -- --num_slots 8 --num_accounts 100000 --iterations 50
```

## File Descriptions

- **main.rs**: Implements the CLI, argument parsing, and all benchmarking logic. Runs account creation, storage, hash calculation, and cleaning benchmarks, printing results to stdout.

## Integration Points
- Used by Solana core developers and performance engineers to benchmark and tune the accounts database.
- Not used in production or by other crates directly.

## Related Crates
- `accounts-db`: The core storage engine being benchmarked
- `solana-measure`: Timing utilities
- `solana-pubkey`: Public key utilities 