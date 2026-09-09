//! Storage keys, TTL policy, and helpers.
//!
//! # TTL strategy (ledger counts — not wall-clock)
//!
//! All constants below are **ledgers**. Do not convert them to “days/years” for
//! protocol decisions: Stellar ledger close time varies by network and era.
//!
//! ## Persistent `Booking(id)`
//! Extended on every write and on successful active reads (`require_booking`).
//! `PERSISTENT_BOOKING_TTL_EXTEND_TO` is sized to comfortably exceed Stello’s
//! maximum expected open booking lifecycle (far-future `start_time`, check-in,
//! completion/dispute) before the entry risks archival.
//!
//! ## Persistent `CancelSettlement(id)`
//! Written once at traveller/host cancel settlement. Retention is intentionally
//! **shorter** than active bookings: the record is terminal audit metadata, not
//! live escrow state. Readers still bump TTL. If archived on-network, Protocol 23+
//! auto-restore (via simulation restore list) can bring it back on access.
//!
//! ## Instance storage
//! `Config`, `TotalEscrowed`, and `NextBookingId` share **one** contract-instance
//! TTL. Call [`bump_instance_ttl`] after instance writes, on active config/
//! accounting reads, and on meaningful booking activity (persistent writes/reads)
//! so instance rent stays aligned with active protocol use — never treat those
//! instance keys as independently expiring.
//!
//! Per current Stellar docs, `env.storage().instance().extend_ttl(...)` extends
//! **both** the contract instance entry **and** its contract code (WASM) entry.
//! A separate WASM-only TTL cron is therefore **not** required for this contract’s
//! instance-bump strategy. Threshold checks for instance vs code are applied
//! independently by the host, but one `instance().extend_ttl` call covers both.
//!
//! Persistent `Booking` / `CancelSettlement` entries still have **independent**
//! TTLs and must continue to be extended individually (they do not share the
//! instance/code TTL).
//!
//! Note: TTL extensions only persist when performed inside a successful
//! contract invocation that commits; pure off-chain simulation does not.

use soroban_sdk::{Env, contracttype};

use crate::errors::Error;
use crate::types::{Booking, CancelSettlement, Config};

/// If remaining TTL is below this, bump booking entries toward
/// [`PERSISTENT_BOOKING_TTL_EXTEND_TO`].
pub const PERSISTENT_BOOKING_TTL_THRESHOLD: u32 = 100_000;

/// Target remaining TTL (ledgers) for `Booking(id)` after an extension.
pub const PERSISTENT_BOOKING_TTL_EXTEND_TO: u32 = 500_000;

/// Threshold for `CancelSettlement(id)` (terminal history; see module docs).
pub const PERSISTENT_SETTLEMENT_TTL_THRESHOLD: u32 = 50_000;

/// Target remaining TTL (ledgers) for cancel-settlement history entries.
pub const PERSISTENT_SETTLEMENT_TTL_EXTEND_TO: u32 = 200_000;

/// If instance TTL is below this, bump toward [`INSTANCE_TTL_EXTEND_TO`].
pub const INSTANCE_TTL_THRESHOLD: u32 = 100_000;

/// Target remaining TTL (ledgers) for the shared contract instance entry
/// (`Config` / `TotalEscrowed` / `NextBookingId`). Calling
/// [`bump_instance_ttl`] also extends the contract code entry (see module docs).
pub const INSTANCE_TTL_EXTEND_TO: u32 = 500_000;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    NextBookingId,
    TotalEscrowed,
    Booking(u64),
    CancelSettlement(u64),
}

/// Extend the shared instance TTL (and, per Stellar docs, the linked contract
/// code entry). Does **not** extend persistent `Booking` / `CancelSettlement`
/// keys — those remain independently TTL-managed.
pub fn bump_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

pub fn set_config(env: &Env, config: &Config) {
    env.storage().instance().set(&DataKey::Config, config);
    bump_instance_ttl(env);
}

pub fn get_config(env: &Env) -> Option<Config> {
    env.storage().instance().get(&DataKey::Config)
}

pub fn require_config(env: &Env) -> Result<Config, Error> {
    let config = get_config(env).ok_or(Error::NotInitialized)?;
    bump_instance_ttl(env);
    Ok(config)
}

pub fn next_booking_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::NextBookingId)
        .unwrap_or(1);
    let next = id.checked_add(1).expect("booking id overflow");
    env.storage().instance().set(&DataKey::NextBookingId, &next);
    bump_instance_ttl(env);
    id
}

pub fn get_total_escrowed(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalEscrowed)
        .unwrap_or(0)
}

pub fn set_total_escrowed(env: &Env, amount: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalEscrowed, &amount);
    bump_instance_ttl(env);
}

pub fn increase_total_escrowed(env: &Env, amount: i128) -> Result<(), Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }
    let current = get_total_escrowed(env);
    let next = current.checked_add(amount).ok_or(Error::MathError)?;
    set_total_escrowed(env, next);
    Ok(())
}

pub fn decrease_total_escrowed(env: &Env, amount: i128) -> Result<(), Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }
    let current = get_total_escrowed(env);
    let next = current.checked_sub(amount).ok_or(Error::MathError)?;
    set_total_escrowed(env, next);
    Ok(())
}

fn extend_booking_ttl(env: &Env, booking_id: u64) {
    env.storage().persistent().extend_ttl(
        &DataKey::Booking(booking_id),
        PERSISTENT_BOOKING_TTL_THRESHOLD,
        PERSISTENT_BOOKING_TTL_EXTEND_TO,
    );
}

fn extend_settlement_ttl(env: &Env, booking_id: u64) {
    env.storage().persistent().extend_ttl(
        &DataKey::CancelSettlement(booking_id),
        PERSISTENT_SETTLEMENT_TTL_THRESHOLD,
        PERSISTENT_SETTLEMENT_TTL_EXTEND_TO,
    );
}

pub fn set_booking(env: &Env, booking: &Booking) {
    let key = DataKey::Booking(booking.booking_id);
    env.storage().persistent().set(&key, booking);
    extend_booking_ttl(env, booking.booking_id);
    // Keep shared instance/code TTL warm on booking writes.
    bump_instance_ttl(env);
}

pub fn get_booking(env: &Env, booking_id: u64) -> Option<Booking> {
    env.storage()
        .persistent()
        .get(&DataKey::Booking(booking_id))
}

pub fn require_booking(env: &Env, booking_id: u64) -> Result<Booking, Error> {
    let booking = get_booking(env, booking_id).ok_or(Error::BookingNotFound)?;
    extend_booking_ttl(env, booking_id);
    bump_instance_ttl(env);
    Ok(booking)
}

pub fn set_cancel_settlement(env: &Env, booking_id: u64, settlement: &CancelSettlement) {
    let key = DataKey::CancelSettlement(booking_id);
    env.storage().persistent().set(&key, settlement);
    extend_settlement_ttl(env, booking_id);
}

pub fn get_cancel_settlement(env: &Env, booking_id: u64) -> Option<CancelSettlement> {
    let settlement = env
        .storage()
        .persistent()
        .get(&DataKey::CancelSettlement(booking_id))?;
    extend_settlement_ttl(env, booking_id);
    Some(settlement)
}
