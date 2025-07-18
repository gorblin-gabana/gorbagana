# solana-account-decoder-client-types

## Overview

This crate provides core types for representing Solana account data in a UI-friendly, serializable format. It is primarily used by the `solana-account-decoder` crate and any RPC or client code that needs to encode/decode account data for display, transport, or inspection.

## Structure

```
src/
├── lib.rs      # Main library entry point, defines UiAccount, UiAccountData, UiAccountEncoding, etc.
├── token.rs    # Types and helpers for SPL token account representations (UiTokenAmount, UiTokenAccount, etc.)
```

## What This Crate Does
- Defines serializable types for Solana account data (UiAccount, UiAccountData, UiTokenAmount, etc.)
- Provides encoding/decoding helpers for account data in various formats (Base58, Base64, JSON, etc.)
- Supports token-specific account representations for SPL Token and Token-2022
- Used as the data contract for RPC responses, CLI output, and explorer UIs

## Where This Crate Is Imported
- `account-decoder` (as a core dependency for all UI/account parsing)
- Any crate that needs to serialize/deserialize Solana account data for RPC, CLI, or UI
- Used transitively by RPC, CLI, and explorer tools

## What This Crate Imports
- `serde`, `serde_derive`, `serde_json` (serialization)
- `base64`, `bs58` (encoding)
- `solana-account` (account data structures)
- `solana-pubkey` (public key handling)
- `core::str::FromStr` (string parsing)

## Example Usage

```rust
use solana_account_decoder_client_types::{UiAccount, UiAccountEncoding};

// Deserialize a UiAccount from JSON
let ui_account: UiAccount = serde_json::from_str(json_str)?;

// Access fields
println!("Owner: {}", ui_account.owner);
println!("Lamports: {}", ui_account.lamports);
```

## File Descriptions

- **lib.rs**: Main entry point. Defines UiAccount, UiAccountData, UiAccountEncoding, UiDataSliceConfig, ParsedAccount, and related types. Implements encoding/decoding logic and helpers.
- **token.rs**: Defines UiTokenAmount, UiTokenAccount, UiMint, UiMultisig, UiExtension, and related types for SPL token and Token-2022 account representations. Provides helpers for formatting token amounts and handling token-specific extensions.

## Integration Points
- Used by `solana-account-decoder` for all account parsing and encoding
- Used by RPC servers to serialize account data in API responses
- Used by CLI tools and explorer UIs to display account information

## Related Crates
- `solana-account-decoder`: Main account parsing and decoding logic
- `solana-account`: Raw account data structures
- `solana-pubkey`: Public key utilities 