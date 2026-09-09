#![no_std]
//! # StelloBookingContract
//!
//! Booking lifecycle on Soroban (book → USDC escrow → check-in → complete → settle).
//!
//! ## Roles
//! - **Stello wallet:** `book`, `update_booking`, `lock_escrow`, `check_in`, `complete`,
//!   `execute_split`.
//! - Cancellation and dispute are deferred to later steps.

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

use events::{
    BookingCheckedIn, BookingCompleted, BookingCreated, BookingUpdated, EscrowLocked,
    PayoutClaimed, SettlementExecuted,
};
use storage::{
    get_claimable, get_config, next_booking_id, require_booking, require_config, set_booking,
    set_claimable, set_config,
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

    pub fn get_claimable_balance(env: Env, who: Address) -> i128 {
        get_claimable(&env, &who)
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
    /// Does not transfer escrow — call [`Self::execute_split`] to settle.
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

    /// Settle a `Completed` booking: 80/10/5/3/2 BPS split of escrow.
    ///
    /// All shares (Host, Ops, Review, QA, O2O) are pushed atomically from escrow.
    ///
    /// **Auth:** Stello wallet. Prevents double settlement.
    pub fn execute_split(env: Env, booking_id: u64) -> Result<SplitAmounts, Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;

        if booking.state != BookingState::Completed {
            return Err(Error::InvalidStateTransition);
        }
        if booking.settled {
            return Err(Error::AlreadySettled);
        }
        if booking.escrow_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let escrow = booking.escrow_amount;
        let split = compute_completion_split(escrow)?;
        let total = split
            .host_amount
            .checked_add(split.ops_amount)
            .and_then(|v| v.checked_add(split.review_amount))
            .and_then(|v| v.checked_add(split.qa_amount))
            .and_then(|v| v.checked_add(split.o2o_amount))
            .ok_or(Error::MathError)?;
        if total != escrow {
            return Err(Error::MathError);
        }

        // Effects before interactions (CEI): mark settled and zero escrow.
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        transfer_from_contract(&token_client, &contract, &booking.host, split.host_amount);
        transfer_from_contract(&token_client, &contract, &config.ops_pool, split.ops_amount);
        transfer_from_contract(
            &token_client,
            &contract,
            &config.review_pool,
            split.review_amount,
        );
        transfer_from_contract(&token_client, &contract, &config.qa_pool, split.qa_amount);
        transfer_from_contract(&token_client, &contract, &config.o2o_pool, split.o2o_amount);

        SettlementExecuted::from_split(booking_id, &split).publish(&env);
        Ok(split)
    }

    /// Withdraw claimable USDC (Host / O2O pull-pay). **Auth:** claimant.
    pub fn claim_payout(env: Env, who: Address) -> Result<i128, Error> {
        who.require_auth();
        let config = require_config(&env)?;
        let amount = get_claimable(&env, &who);
        if amount <= 0 {
            return Err(Error::NothingToClaim);
        }

        // CEI: zero balance before transfer.
        set_claimable(&env, &who, 0);

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        transfer_from_contract(&token_client, &contract, &who, amount);

        PayoutClaimed {
            claimant: who,
            amount,
            token: config.token,
        }
        .publish(&env);

        Ok(amount)
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

fn transfer_from_contract(
    token_client: &token::TokenClient,
    contract: &Address,
    to: &Address,
    amount: i128,
) {
    if amount > 0 {
        token_client.transfer(contract, to, &amount);
    }
}
