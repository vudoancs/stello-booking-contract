//! Storage keys and helpers.
use soroban_sdk::{Env, contracttype};

use crate::errors::Error;
use crate::types::{Booking, CancelSettlement, Config};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    NextBookingId,
    TotalEscrowed,
    Booking(u64),
    CancelSettlement(u64),
}

pub fn set_config(env: &Env, config: &Config) {
    env.storage().instance().set(&DataKey::Config, config);
}

pub fn get_config(env: &Env) -> Option<Config> {
    env.storage().instance().get(&DataKey::Config)
}

pub fn require_config(env: &Env) -> Result<Config, Error> {
    get_config(env).ok_or(Error::NotInitialized)
}

pub fn next_booking_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::NextBookingId)
        .unwrap_or(1);
    let next = id.checked_add(1).expect("booking id overflow");
    env.storage().instance().set(&DataKey::NextBookingId, &next);
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

pub fn set_booking(env: &Env, booking: &Booking) {
    env.storage()
        .persistent()
        .set(&DataKey::Booking(booking.booking_id), booking);
}

pub fn get_booking(env: &Env, booking_id: u64) -> Option<Booking> {
    env.storage()
        .persistent()
        .get(&DataKey::Booking(booking_id))
}

pub fn require_booking(env: &Env, booking_id: u64) -> Result<Booking, Error> {
    get_booking(env, booking_id).ok_or(Error::BookingNotFound)
}

pub fn set_cancel_settlement(env: &Env, booking_id: u64, settlement: &CancelSettlement) {
    env.storage()
        .persistent()
        .set(&DataKey::CancelSettlement(booking_id), settlement);
}

pub fn get_cancel_settlement(env: &Env, booking_id: u64) -> Option<CancelSettlement> {
    env.storage()
        .persistent()
        .get(&DataKey::CancelSettlement(booking_id))
}
