#![no_std]
//! # StelloBookingContract
//!
//! Booking lifecycle on Soroban (Step 2–3: book → USDC escrow → check-in → complete).
//!
//! ## Roles
//! - **Stello wallet:** `book`, `update_booking`, `lock_escrow`, `check_in`, `complete`.
//! - Settlement transfers, cancellation, and dispute are deferred to later steps.

mod errors;
mod events;
mod refund;
mod split;
mod state_machine;
mod storage;
mod types;

#[cfg(test)]
mod test;

pub use errors::Error;
pub use refund::compute_refund;
pub use split::{compute_bps_amounts, compute_completion_split};
pub use state_machine::{is_terminal, validate_transition};
pub use types::*;

use events::{BookingCheckedIn, BookingCompleted, BookingCreated, BookingUpdated, EscrowLocked};
use storage::{
    get_config, next_booking_id, require_booking, require_config, set_booking, set_config,
};

use soroban_sdk::{Address, Env, contract, contractimpl, token};

#[contract]
pub struct StelloBookingContract;

#[contractimpl]
impl StelloBookingContract {
    /// Initialize protocol config. Caller must be `config.stello_wallet`.
    pub fn initialize(env: Env, config: Config) -> Result<(), Error> {
        if get_config(&env).is_some() {
            return Err(Error::AlreadyInitialized);
        }
        validate_config(&config)?;
        config.stello_wallet.require_auth();
        set_config(&env, &config);
        Ok(())
    }

    pub fn get_config(env: Env) -> Result<Config, Error> {
        require_config(&env)
    }

    pub fn get_booking(env: Env, booking_id: u64) -> Result<Booking, Error> {
        require_booking(&env, booking_id)
    }

    pub fn get_booking_state(env: Env, booking_id: u64) -> Result<BookingState, Error> {
        Ok(require_booking(&env, booking_id)?.state)
    }

    /// Create booking in `Created`. **Auth:** Stello wallet.
    pub fn book(
        env: Env,
        traveller: Address,
        host: Address,
        amount: i128,
        start_time: u64,
    ) -> Result<u64, Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        validate_parties(&traveller, &host)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let booking_id = next_booking_id(&env);
        let booking = Booking {
            booking_id,
            traveller: traveller.clone(),
            host: host.clone(),
            amount,
            token: config.token.clone(),
            start_time,
            created_at: env.ledger().timestamp(),
            state: BookingState::Created,
            escrow_locked: false,
            escrow_amount: 0,
            checked_in: false,
            settled: false,
            was_cancelled: false,
            cancelled_by: CancelledBy::None,
        };
        set_booking(&env, &booking);

        BookingCreated {
            booking_id,
            traveller,
            host,
            amount,
            token: config.token,
            start_time,
        }
        .publish(&env);

        Ok(booking_id)
    }

    /// Update booking fields while still `Created` (before escrow). **Auth:** Stello.
    pub fn update_booking(
        env: Env,
        booking_id: u64,
        host: Address,
        amount: i128,
        start_time: u64,
    ) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;
        if booking.state != BookingState::Created || booking.escrow_locked {
            return Err(Error::InvalidUpdate);
        }
        validate_parties(&booking.traveller, &host)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        booking.host = host.clone();
        booking.amount = amount;
        booking.start_time = start_time;
        set_booking(&env, &booking);

        BookingUpdated {
            booking_id,
            amount,
            start_time,
            host,
        }
        .publish(&env);

        Ok(())
    }

    /// Pull USDC from traveller into contract escrow (`Created → Escrowed`).
    ///
    /// `amount` must equal `booking.amount` and be `> 0`.
    /// **Auth:** Stello wallet (traveller also signs the token transfer).
    pub fn lock_escrow(env: Env, booking_id: u64, amount: i128) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;

        if booking.state != BookingState::Created {
            return Err(Error::InvalidStateTransition);
        }
        validate_transition(booking.state, BookingState::Escrowed)?;

        if booking.escrow_locked || booking.escrow_amount != 0 {
            return Err(Error::EscrowAlreadyLocked);
        }
        if amount <= 0 || amount != booking.amount {
            return Err(Error::InvalidAmount);
        }

        let new_escrow = booking
            .escrow_amount
            .checked_add(amount)
            .ok_or(Error::MathError)?;
        if new_escrow > booking.amount {
            return Err(Error::EscrowExceedsAmount);
        }
        if booking.token != config.token {
            return Err(Error::InvalidToken);
        }

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        booking.traveller.require_auth();
        token_client.transfer(&booking.traveller, &contract, &amount);

        booking.state = BookingState::Escrowed;
        booking.escrow_locked = true;
        booking.escrow_amount = new_escrow;
        set_booking(&env, &booking);

        EscrowLocked {
            booking_id,
            amount,
            token: config.token,
        }
        .publish(&env);

        Ok(())
    }

    /// GPS / experience check-in (`Escrowed → CheckedIn`). **Auth:** Stello wallet.
    pub fn check_in(env: Env, booking_id: u64) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;
        validate_transition(booking.state, BookingState::CheckedIn)?;
        if !booking.escrow_locked {
            return Err(Error::EscrowNotLocked);
        }

        let timestamp = env.ledger().timestamp();
        booking.state = BookingState::CheckedIn;
        booking.checked_in = true;
        set_booking(&env, &booking);

        BookingCheckedIn {
            booking_id,
            timestamp,
        }
        .publish(&env);

        Ok(())
    }

    /// Mark booking completed (`CheckedIn → Completed`). **Auth:** Stello.
    /// Does not transfer escrow yet (settlement is a later step).
    pub fn complete(env: Env, booking_id: u64) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;
        validate_transition(booking.state, BookingState::Completed)?;
        if !booking.escrow_locked || !booking.checked_in {
            return Err(Error::InvalidStateTransition);
        }

        booking.state = BookingState::Completed;
        set_booking(&env, &booking);

        BookingCompleted { booking_id }.publish(&env);
        Ok(())
    }
}

fn require_stello(config: &Config) {
    config.stello_wallet.require_auth();
}

fn validate_config(config: &Config) -> Result<(), Error> {
    if config.host_cancel_fee < 0 {
        return Err(Error::InvalidAmount);
    }
    Ok(())
}

fn validate_parties(traveller: &Address, host: &Address) -> Result<(), Error> {
    if traveller == host {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}
