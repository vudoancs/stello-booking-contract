//! Storage keys and helpers.
use soroban_sdk::{contracttype, Env};

use crate::errors::Error;
use crate::types::{Booking, Config};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    NextBookingId,
    Booking(u64),
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
    env.storage()
        .instance()
        .set(&DataKey::NextBookingId, &(id + 1));
    id
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
