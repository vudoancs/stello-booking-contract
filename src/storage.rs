//! Storage keys, TTL policy, and helpers.
//!
//! # TTL strategy (ledger counts — not wall-clock)
//!
//! All constants below are **ledgers**. Do not convert them to “days/years” for
//! protocol decisions: Stellar ledger close time varies by network and era.
//!
//! ## Persistent `Booking(id)` and `BookingRef(ref)`
//! Extended on every write and on successful active reads (`require_booking` /
//! ref lookups). Both use the same [`PERSISTENT_BOOKING_TTL_*`] policy so the
//! reverse-lookup index cannot expire significantly earlier than the canonical
//! booking entry.
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
//! Persistent `Booking` / `BookingRef` / `CancelSettlement` entries still have
//! **independent** TTLs and must continue to be extended individually (they do
//! not share the instance/code TTL).
//!
//! Note: TTL extensions only persist when performed inside a successful
//! contract invocation that commits; pure off-chain simulation does not.

use soroban_sdk::{BytesN, Env, contracttype};

use crate::errors::Error;
use crate::types::{Booking, CancelSettlement, Config};

/// If remaining TTL is below this, bump booking entries toward
/// [`PERSISTENT_BOOKING_TTL_EXTEND_TO`].
pub const PERSISTENT_BOOKING_TTL_THRESHOLD: u32 = 100_000;

/// Target remaining TTL (ledgers) for `Booking(id)` / `BookingRef(ref)` after
/// an extension.
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
    /// `booking_ref` → `booking_id` (canonical Booking remains under `Booking(id)`).
    BookingRef(BytesN<32>),
    CancelSettlement(u64),
}

/// Extend the shared instance TTL (and, per Stellar docs, the linked contract
/// code entry). Does **not** extend persistent `Booking` / `BookingRef` /
/// `CancelSettlement` keys — those remain independently TTL-managed.
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

/// Read the next booking id that would be allocated **without** mutating state.
#[cfg(test)]
pub fn peek_next_booking_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::NextBookingId)
        .unwrap_or(1)
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

fn extend_booking_ref_ttl(env: &Env, booking_ref: &BytesN<32>) {
    env.storage().persistent().extend_ttl(
        &DataKey::BookingRef(booking_ref.clone()),
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

/// Keep `Booking(id)` and `BookingRef(ref)` TTLs aligned under the same policy.
fn bump_booking_and_ref_ttl(env: &Env, booking: &Booking) {
    extend_booking_ttl(env, booking.booking_id);
    extend_booking_ref_ttl(env, &booking.booking_ref);
}

pub fn set_booking(env: &Env, booking: &Booking) {
    let key = DataKey::Booking(booking.booking_id);
    env.storage().persistent().set(&key, booking);
    // BookingRef index is written separately at create; subsequent writes bump
    // both entries when the index already exists.
    if booking_ref_exists(env, &booking.booking_ref) {
        bump_booking_and_ref_ttl(env, booking);
    } else {
        extend_booking_ttl(env, booking.booking_id);
    }
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
    bump_booking_and_ref_ttl(env, &booking);
    bump_instance_ttl(env);
    Ok(booking)
}

pub fn booking_ref_exists(env: &Env, booking_ref: &BytesN<32>) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::BookingRef(booking_ref.clone()))
}

pub fn get_booking_id_by_ref(env: &Env, booking_ref: &BytesN<32>) -> Option<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::BookingRef(booking_ref.clone()))
}

/// Persist `booking_ref → booking_id` and apply the booking TTL policy.
pub fn set_booking_ref_index(env: &Env, booking_ref: &BytesN<32>, booking_id: u64) {
    let key = DataKey::BookingRef(booking_ref.clone());
    env.storage().persistent().set(&key, &booking_id);
    extend_booking_ref_ttl(env, booking_ref);
    bump_instance_ttl(env);
}

pub fn require_booking_id_by_ref(env: &Env, booking_ref: &BytesN<32>) -> Result<u64, Error> {
    let booking_id = get_booking_id_by_ref(env, booking_ref).ok_or(Error::BookingNotFound)?;
    // Align TTLs via the canonical booking path.
    let _ = require_booking(env, booking_id)?;
    Ok(booking_id)
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
