# Stello Booking Contract

Soroban smart contract for Stello booking lifecycle, USDC escrow, and atomic **push** settlement.

Configuration is set **atomically at deploy** via `__constructor`. There is no post-deploy `initialize` call.

## Roles

| Actor | Actions |
|-------|---------|
| Stello wallet | `book`, `update_booking`, `check_in`, `complete`, `execute_split`, `cancel_by_traveller`, `open_dispute`, `resolve_dispute` |
| Traveller | `lock_escrow` only (`booking.traveller.require_auth()`; USDC transfer from Traveller into escrow) |
| Host wallet | `cancel_by_host` only (`booking.host.require_auth()`; `$5` fee from host wallet, not escrow) |

## Settlement

- **Completed:** Host 80% / Ops 10% / Review 5% / QA 3% / O2O 2% (push in `execute_split`)
- **Traveller cancel:** ≥4w 70/15/15; 2–4w 50/35/15; &lt;2w 0/80/20 (traveller/host/ops)
- **Host cancel:** 100% escrow → traveller; + `host_cancel_fee` USDC from host → ops (default `50_000_000` = $5 @ 7 decimals)
- **Dispute:** custom BPS shares summing to `10_000`
- **Recovery:** if `execute_split` fails, booking stays `Completed` + unsettled; Stello may `open_dispute` then `resolve_dispute`

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

Build first (`make build`), then deploy **with constructor arguments** (no separate initialize):

```bash
# Create identity once
stellar keys generate stello-admin --network testnet --fund

# Build WASM
make build

# Deploy — pass __constructor args after `--`
# Order: stello_wallet, token, ops_pool, review_pool, qa_pool, o2o_pool, host_cancel_fee
stellar contract deploy \
  --wasm target/wasm32v1-none/release/stello_booking_contract.wasm \
  --source-account stello-admin \
  --network testnet \
  -- \
  <stello_wallet_address> \
  <usdc_sac_address> \
  <ops_pool_address> \
  <review_pool_address> \
  <qa_pool_address> \
  <o2o_pool_address> \
  <host_cancel_fee_i128>
```

Do **not** call `initialize` after deployment — that entrypoint no longer exists.

Use testnet USDC SAC for `token`. Do not commit real production addresses into docs or scripts.

> Note: SDK was bumped from 23.x → 27.x because `soroban-env-host` 23 resolves `ed25519-dalek >=2` to 3.x, which breaks `cargo test` with rand 0.8.
