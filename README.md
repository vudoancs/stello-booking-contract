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

## CI/CD & Testnet Deployment

GitHub Actions automates checks and Testnet releases. **Mainnet is not automated.**

### What runs on push / PR

On `pull_request` and pushes to `dev` / `main` (and on Testnet tags), the **CI** workflow runs:

1. `cargo fmt --check`
2. `cargo test`
3. `cargo clippy -- -D warnings`
4. Optimized WASM build with provenance meta
5. Interface inspection (rejects deprecated `initialize` / `claim_*` / `release_refund`)
6. Structured meta check (`source_repo`, `commit_sha`) + `sha256sum` == `stellar contract info hash --wasm`
7. WASM artifact upload

A normal push to `dev` **never deploys**.

### How to create a Testnet release

From the commit you want deployed:

```bash
git tag -a v0.1.0-testnet.X -m "Testnet Release X"
git push origin v0.1.0-testnet.X
```

Tag pattern required: `v*-testnet.*` (example: `v0.1.0-testnet.3`).

### What the Deploy Testnet workflow does

1. Checks out the **exact tagged commit**
2. Re-runs CI checks on that commit
3. Builds optimized WASM with provenance meta (`source_repo`, `commit_sha`) and **fails before deploy** if meta does not match `github.repository` / `github.sha`
4. Records `sha256sum` of that exact file (must equal `stellar contract info hash --wasm`), then deploys **that same file** with `--optimize=false` (no second optimize/rebuild)
5. Uses GitHub Environment **`dev`** for constructor config vars and the Testnet deployer secret
6. Verifies deployed interface; `get_config` via structured JSON exact field match; `get_total_escrowed == 0`; three-way Wasm hash (`sha256sum` == `--wasm` == `--contract-id`)
7. Publishes artifacts + a **prerelease** on GitHub

### Where to find the Contract ID

- GitHub Actions run **Summary**
- GitHub Release notes for the tag
- Artifact / release file `deployment/testnet.json` (`contract_id`) — generated only; **not committed** (`/deployment/` is gitignored)

### Notes

- Constructor args come from Environment **variables** (not committed): `STELLO_WALLET`, `USDC_TOKEN`, `OPS_POOL`, `REVIEW_POOL`, `QA_POOL`, `O2O_POOL`, optional `HOST_CANCEL_FEE` (default `50000000`).
- Deployer credential is Environment/repo **secret** `STELLAR_TESTNET_SECRET_KEY` (never printed or written into manifests). Referenced only in the tag-triggered deploy job.
- `pull_request` / push to `dev` / `main` never deploy. Only `refs/tags/v*-testnet.*` deploy.
- `deploy_tx_hash` is omitted from the manifest unless/until Stellar CLI exposes it reliably (Contract ID is mandatory).
- Testnet releases do **not** deploy Mainnet.

## Manual Deploy (testnet)

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
