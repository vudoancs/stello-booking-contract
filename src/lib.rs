#![no_std]
//! # StelloBookingContract
//!
//! Booking lifecycle, USDC escrow, and atomic settlement on Soroban.
//!
//! ## Roles
//! - **Stello wallet:** create / update booking, lock escrow, traveller cancel,
//!   complete booking, open & resolve disputes.
//! - **Host wallet:** host cancellation (authorizes the $5 cancellation fee).
//! - **Contract:** validates authorization, owns booking + escrow state, settles atomically.

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
    BookingCancelled, BookingCompleted, BookingCreated, BookingUpdated, DisputeOpened,
    DisputeResolved, EscrowLocked,
};
use refund::compute_refund as refund_compute;
use storage::{
    get_config, next_booking_id, require_booking, require_config, set_booking, set_config,
};

use soroban_sdk::{contract, contractimpl, token, Address, Env, Vec};

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

    /// Quote traveller/host cancel escrow split (does not include host $5 fee).
    pub fn quote_refund(
        env: Env,
        booking_id: u64,
        cancelled_by: CancelledBy,
    ) -> Result<CancelSettlement, Error> {
        let booking = require_booking(&env, booking_id)?;
        let now = env.ledger().timestamp();
        refund_compute(booking.amount, booking.start_time, now, cancelled_by)
    }

    /// Create booking in `Created`. **Auth:** Stello wallet.
    pub fn create_booking(
        env: Env,
        traveller: Address,
        host: Address,
        amount: i128,
        start_time: u64,
    ) -> Result<u64, Error> {
        let config = require_config(&env)?;
        require_stello(&env, &config)?;
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
        require_stello(&env, &config)?;
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

    /// Pull USDC from traveller into contract escrow. **Auth:** Stello + traveller (token).
    pub fn lock_escrow(env: Env, booking_id: u64) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&env, &config)?;
        let mut booking = require_booking(&env, booking_id)?;
        validate_transition(booking.state, BookingState::Escrowed)?;
        if booking.escrow_locked {
            return Err(Error::EscrowAlreadyLocked);
        }
        if booking.token != config.token {
            return Err(Error::InvalidToken);
        }

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        // Traveller must authorize the transfer into escrow.
        booking.traveller.require_auth();
        token_client.transfer(&booking.traveller, &contract, &booking.amount);

        booking.state = BookingState::Escrowed;
        booking.escrow_locked = true;
        set_booking(&env, &booking);

        EscrowLocked {
            booking_id,
            amount: booking.amount,
            token: config.token,
        }
        .publish(&env);

        Ok(())
    }

    /// Complete booking and push 80/10/5/3/2 split. **Auth:** Stello.
    pub fn complete_booking(env: Env, booking_id: u64) -> Result<SplitAmounts, Error> {
        let config = require_config(&env)?;
        require_stello(&env, &config)?;
        let mut booking = require_booking(&env, booking_id)?;
        validate_transition(booking.state, BookingState::Completed)?;
        require_escrow_unlocked_funds(&booking)?;

        let split = compute_completion_split(booking.amount)?;
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

        booking.state = BookingState::Completed;
        booking.settled = true;
        set_booking(&env, &booking);

        BookingCompleted::from_split(booking_id, &split).publish(&env);
        Ok(split)
    }

    /// Traveller cancellation with tiered escrow split. **Auth:** Stello.
    pub fn cancel_by_traveller(env: Env, booking_id: u64) -> Result<CancelSettlement, Error> {
        let config = require_config(&env)?;
        require_stello(&env, &config)?;
        let mut booking = require_booking(&env, booking_id)?;

        if booking.state == BookingState::Created {
            validate_transition(booking.state, BookingState::Cancelled)?;
            booking.state = BookingState::Cancelled;
            booking.was_cancelled = true;
            booking.cancelled_by = CancelledBy::Traveller;
            booking.settled = true;
            set_booking(&env, &booking);
            let empty = CancelSettlement {
                traveller_amount: 0,
                host_amount: 0,
                ops_amount: 0,
            };
            BookingCancelled::from_settlement(booking_id, false, &empty, 0).publish(&env);
            return Ok(empty);
        }

        validate_transition(booking.state, BookingState::Cancelled)?;
        require_escrow_unlocked_funds(&booking)?;

        let now = env.ledger().timestamp();
        let settlement =
            refund_compute(booking.amount, booking.start_time, now, CancelledBy::Traveller)?;

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        transfer_from_contract(
            &token_client,
            &contract,
            &booking.traveller,
            settlement.traveller_amount,
        );
        transfer_from_contract(
            &token_client,
            &contract,
            &booking.host,
            settlement.host_amount,
        );
        transfer_from_contract(
            &token_client,
            &contract,
            &config.ops_pool,
            settlement.ops_amount,
        );

        booking.state = BookingState::Cancelled;
        booking.was_cancelled = true;
        booking.cancelled_by = CancelledBy::Traveller;
        booking.settled = true;
        set_booking(&env, &booking);

        BookingCancelled::from_settlement(booking_id, false, &settlement, 0).publish(&env);
        Ok(settlement)
    }

    /// Host cancellation: 100% escrow → traveller; $5 USDC from host → ops.
    /// **Auth:** Host (covers fee transfer).
    pub fn cancel_by_host(env: Env, booking_id: u64) -> Result<CancelSettlement, Error> {
        let config = require_config(&env)?;
        let mut booking = require_booking(&env, booking_id)?;
        booking.host.require_auth();

        if booking.state == BookingState::Created {
            validate_transition(booking.state, BookingState::Cancelled)?;
            // Still charge host fee to ops (not from escrow).
            let fee = effective_host_fee(&config);
            if fee > 0 {
                let token_client = token::TokenClient::new(&env, &config.token);
                token_client.transfer(&booking.host, &config.ops_pool, &fee);
            }
            booking.state = BookingState::Cancelled;
            booking.was_cancelled = true;
            booking.cancelled_by = CancelledBy::Host;
            booking.settled = true;
            set_booking(&env, &booking);
            let empty = CancelSettlement {
                traveller_amount: 0,
                host_amount: 0,
                ops_amount: 0,
            };
            BookingCancelled::from_settlement(booking_id, true, &empty, fee).publish(&env);
            return Ok(empty);
        }

        validate_transition(booking.state, BookingState::Cancelled)?;
        require_escrow_unlocked_funds(&booking)?;

        let settlement =
            refund_compute(booking.amount, booking.start_time, 0, CancelledBy::Host)?;
        let fee = effective_host_fee(&config);

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);

        // Escrow → traveller (100%).
        transfer_from_contract(
            &token_client,
            &contract,
            &booking.traveller,
            settlement.traveller_amount,
        );
        // Host wallet → ops ($5), NOT from escrow.
        if fee > 0 {
            token_client.transfer(&booking.host, &config.ops_pool, &fee);
        }

        booking.state = BookingState::Cancelled;
        booking.was_cancelled = true;
        booking.cancelled_by = CancelledBy::Host;
        booking.settled = true;
        set_booking(&env, &booking);

        BookingCancelled::from_settlement(booking_id, true, &settlement, fee).publish(&env);
        Ok(settlement)
    }

    /// Freeze escrow for dispute. **Auth:** Stello.
    pub fn open_dispute(env: Env, booking_id: u64) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&env, &config)?;
        let mut booking = require_booking(&env, booking_id)?;
        validate_transition(booking.state, BookingState::Disputed)?;
        require_escrow_unlocked_funds(&booking)?;

        booking.state = BookingState::Disputed;
        set_booking(&env, &booking);

        DisputeOpened { booking_id }.publish(&env);
        Ok(())
    }

    /// Resolve dispute with custom BPS allocation (must sum to 10_000).
    /// Pushes escrow atomically to recipients. **Auth:** Stello.
    pub fn resolve_dispute(
        env: Env,
        booking_id: u64,
        shares: Vec<DisputeShare>,
    ) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&env, &config)?;
        let mut booking = require_booking(&env, booking_id)?;
        if booking.state != BookingState::Disputed {
            return Err(Error::InvalidDispute);
        }
        validate_transition(booking.state, BookingState::Completed)?;
        require_escrow_unlocked_funds(&booking)?;
        if shares.is_empty() {
            return Err(Error::InvalidBpsAllocation);
        }

        let n = shares.len() as usize;
        if n > 16 {
            return Err(Error::InvalidBpsAllocation);
        }

        let mut bps_buf = [0u32; 16];
        for (i, slot) in bps_buf.iter_mut().enumerate().take(n) {
            let share = shares.get(i as u32).unwrap();
            *slot = share.bps;
        }
        let mut amount_buf = [0i128; 16];
        let written = compute_bps_amounts(booking.amount, &bps_buf[..n], &mut amount_buf)?;

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        for (i, &payout) in amount_buf.iter().enumerate().take(written) {
            let share = shares.get(i as u32).unwrap();
            transfer_from_contract(&token_client, &contract, &share.recipient, payout);
        }

        booking.state = BookingState::Completed;
        booking.settled = true;
        set_booking(&env, &booking);

        DisputeResolved {
            booking_id,
            amount: booking.amount,
        }
        .publish(&env);

        Ok(())
    }
}

fn require_stello(env: &Env, config: &Config) -> Result<(), Error> {
    config.stello_wallet.require_auth();
    let _ = env;
    Ok(())
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

fn require_escrow_unlocked_funds(booking: &Booking) -> Result<(), Error> {
    if booking.settled {
        return Err(Error::AlreadySettled);
    }
    if !booking.escrow_locked {
        return Err(Error::EscrowNotLocked);
    }
    Ok(())
}

fn effective_host_fee(config: &Config) -> i128 {
    config.host_cancel_fee
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
