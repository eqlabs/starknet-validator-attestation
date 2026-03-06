# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`starknet-validator-attestation` is a Rust binary tool for attesting Starknet validators per [SNIP 28](https://community.starknet.io/t/snip-28-staking-v2-proposal/115250) (Staking v2). It monitors the Starknet chain and submits an attestation transaction once per epoch at the validator's assigned block.

## Commands

```sh
# Build
cargo build

# Run tests
just test                    # or: cargo test --workspace --all-targets
just test <test_name>        # run a single test

# Lint
just clippy                  # clippy with -D warnings and -D rust_2018_idioms
just check                   # cargo check only

# Format
just fmt

# Run
cargo run -- --staker-operational-address <ADDR> --node-url <URL> --local-signer
```

The Rust toolchain is pinned to **1.91.0** via `rust-toolchain.toml`.

## Architecture

The program is a single async `tokio` binary. The main event loop in `main.rs` drives a state machine that coordinates two WebSocket background tasks.

### Concurrent tasks

Two tasks run in the background and send updates to the main loop via `mpsc` channels. Both tasks are automatically restarted (with a 5-second delay) if they exit.

- **`headers::fetch`** (`headers.rs`) — subscribes to `starknet_subscribeNewHeads` over WebSocket and forwards `BlockHeader` and `ReorgData` messages.
- **`events::fetch`** (`events.rs`) — subscribes to `starknet_subscribeEvents` filtering on `StakerAttestationSuccessful` events from the attestation contract, forwarding `AttestationEvent` and `ReorgData` messages.

### State machine (`state.rs`)

`State` is a `Clone`able enum with four variants that progress in order each epoch:

1. `BeforeBlockToAttest` — waiting for the chain to reach the block assigned for attestation.
2. `Attesting` — inside the attestation window; submits the transaction via `Client::attest`.
3. `AttestationSubmitted` — transaction sent; polls for confirmation via `Client::attestation_status`.
4. `WaitingForNextEpoch` — attestation confirmed; idle until the next epoch begins.

State transitions are triggered by `handle_new_block_header` (called on each new block) and `handle_new_event` (called on each `StakerAttestationSuccessful` event). On a chain reorg the state is reset to a fresh `State::from_attestation_info`.

### RPC client (`jsonrpc.rs`)

`Client` is a trait that abstracts all Starknet interactions; `StarknetRpcClient` is the production implementation wrapping `JsonRpcClient<HttpTransport>`. Tests mock this trait. The inner `ClearSigningAccount` implements the `starknet` crate's `Account` trait to build and sign INVOKE v3 transactions.

### Signing (`signer.rs`)

`AttestationSigner` is an enum with two variants:
- `Local` — signs with a `LocalWallet` (private key from `VALIDATOR_ATTESTATION_OPERATIONAL_PRIVATE_KEY`).
- `Remote` — POSTs transaction + chain_id to an external HTTP `/sign` endpoint; see `examples/signer.rs` for a reference implementation.

### Tip calculation (`tip.rs`)

`TipCalculationParams::calculate_tip` applies a scaling factor (`tip_boost`) to the median tip of the latest block, then takes the maximum of that and `minimum_tip`. Both parameters are CLI-configurable.

### Attestation block selection (`attestation_info.rs`)

The block to attest in a given epoch is deterministic: `Poseidon(stake, epoch_id, staker_address) mod (epoch_len - attestation_window)` offset from the epoch's starting block. The attestation window opens 11 blocks after that block (to ensure the block hash is available on-chain).

### Metrics (`metrics_exporter.rs`)

A Prometheus scrape endpoint is served via `axum` (default `127.0.0.1:9090/metrics`). Gauges and counters are updated throughout the state machine using the `metrics` crate macros.

### Contract addresses

Known mainnet and Sepolia addresses for the staking and attestation contracts are hardcoded in `main.rs`; the STRK token address is the same for both networks. For other networks, all three addresses must be passed explicitly via CLI flags.
