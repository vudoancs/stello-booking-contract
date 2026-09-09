# Stello Booking Contract

Soroban smart contract for Stello booking lifecycle, USDC escrow, and atomic settlement.

## Roles

| Actor | Actions |
|-------|---------|
| Stello wallet | `initialize`, `create_booking`, `update_booking`, `lock_escrow`, `cancel_by_traveller`, `complete_booking`, `open_dispute`, `resolve_dispute` |
| Host wallet | `cancel_by_host` (authorizes $5 USDC fee from host, not escrow) |

## Settlement

- **Completed:** Host 80% / Ops 10% / Review 5% / QA 3% / O2O 2%
- **Traveller cancel:** ≥4w 70/15/15; 2–4w 50/35/15; &lt;2w 0/80/20 (traveller/host/ops)
- **Host cancel:** 100% escrow → traveller; + `host_cancel_fee` USDC from host → ops (default `50_000_000` = $5 @ 7 decimals)
- **Dispute:** custom BPS shares summing to `10_000`

## Requirements

- Rust with `wasm32v1-none` (`rustup target add wasm32v1-none`)
- [Stellar CLI](https://developers.stellar.org/docs/tools/cli) (`stellar`)
- Soroban SDK 27.x

## Commands

```bash
make test      # cargo test
make build     # stellar contract build
make fmt
make clippy
make check     # fmt + clippy + test + build
```

## Deploy (testnet)

```bash
# Create identity once
stellar keys generate stello-admin --network testnet --fund

# Build WASM
make build

# Deploy
stellar contract deploy \
  --wasm target/wasm32v1-none/release/stello_booking_contract.wasm \
  --source stello-admin \
  --network testnet
```

Use testnet USDC SAC address for `token` when initializing on testnet.

> Note: SDK was bumped from 23.x → 27.x because `soroban-env-host` 23 resolves `ed25519-dalek >=2` to 3.x, which breaks `cargo test` with rand 0.8.
