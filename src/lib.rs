#![no_std]
//! # StelloBookingContract
//!
//! Booking lifecycle on Soroban (book → USDC escrow → check-in → complete → settle).
//!
//! ## Push payments only
//! All settlement outcomes transfer escrow **immediately** in the same transaction.
//! There is no claimable / pull-payment model.
//!
//! ## Roles
//! - **Stello wallet:** `book`, `update_booking`, `lock_escrow`, `check_in`, `complete`,
//!   `execute_split`, `cancel_by_traveller`, `open_dispute`, `resolve_dispute`.
//! - **Host wallet:** `cancel_by_host` only (`booking.host.require_auth()`).
//! - Stello Wallet is the sole dispute-resolution authority (MVP; no DAO/multisig).

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
    BookingCancelled, BookingCheckedIn, BookingCompleted, BookingCreated, BookingUpdated,
    DisputeOpened, DisputeResolved, EscrowLocked, HostCancellationFeePaid, SettlementExecuted,
};
use storage::{
    decrease_total_escrowed, get_cancel_settlement as load_cancel_settlement, get_config,
    get_total_escrowed, increase_total_escrowed, next_booking_id, require_booking, require_config,
    set_booking, set_cancel_settlement, set_config,
};

use soroban_sdk::{Address, Env, contract, contractimpl, token};

#[contract]
pub struct StelloBookingContract;

#[contractimpl]
impl StelloBookingContract {
    /// Initialize protocol config. Caller must be `config.stello_wallet`. One-time only.
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

    /// Global accounted escrow (excludes unsolicited token transfers).
    pub fn get_total_escrowed(env: Env) -> i128 {
        get_total_escrowed(&env)
    }

    pub fn get_cancel_settlement(env: Env, booking_id: u64) -> Result<CancelSettlement, Error> {
        load_cancel_settlement(&env, booking_id).ok_or(Error::SettlementNotFound)
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
        validate_start_time(&env, start_time)?;

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
    ///
    /// `start_time` cannot be changed once the booking is `Escrowed`.
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
        validate_start_time(&env, start_time)?;

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
    /// Increases `total_escrowed` by `amount` (checked).
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
        increase_total_escrowed(&env, amount)?;

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

    /// Settle a `Completed` booking: 80/10/5/3/2 push split of escrow.
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

        require_solvency(&env, &config.token)?;

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

        // Effects before interactions (CEI).
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);
        decrease_total_escrowed(&env, escrow)?;

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

        SettlementExecuted {
            booking_id,
            settlement_type: SettlementType::Completed,
            traveller_amount: 0,
            host_amount: split.host_amount,
            ops_amount: split.ops_amount,
            review_amount: split.review_amount,
            qa_amount: split.qa_amount,
            o2o_amount: split.o2o_amount,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(split)
    }

    /// Traveller cancellation while `Escrowed`. **Auth:** Stello wallet only.
    ///
    /// Tiered refund from `start_time` vs ledger timestamp:
    /// - `>= 4 weeks`: Traveller 70% / Host 15% / Ops 15%
    /// - `2–4 weeks`: Traveller 50% / Host 35% / Ops 15%
    /// - `< 2 weeks`: Traveller 0% / Host 80% / Ops 20%
    pub fn cancel_by_traveller(env: Env, booking_id: u64) -> Result<CancelSettlement, Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;

        if booking.was_cancelled || booking.state == BookingState::Cancelled {
            return Err(Error::AlreadyCancelled);
        }
        if booking.settled {
            return Err(Error::AlreadySettled);
        }
        if booking.state != BookingState::Escrowed {
            return Err(Error::InvalidStateTransition);
        }
        validate_transition(booking.state, BookingState::Cancelled)?;
        if booking.escrow_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        require_solvency(&env, &config.token)?;

        let now = env.ledger().timestamp();
        let escrow = booking.escrow_amount;
        let settlement = compute_refund(escrow, booking.start_time, now, CancelledBy::Traveller)?;
        let total = settlement
            .traveller_amount
            .checked_add(settlement.host_amount)
            .and_then(|v| v.checked_add(settlement.ops_amount))
            .ok_or(Error::MathError)?;
        if total != escrow {
            return Err(Error::MathError);
        }

        booking.state = BookingState::Cancelled;
        booking.was_cancelled = true;
        booking.cancelled_by = CancelledBy::Traveller;
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);
        set_cancel_settlement(&env, booking_id, &settlement);
        decrease_total_escrowed(&env, escrow)?;

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

        BookingCancelled {
            booking_id,
            by_host: false,
            traveller_amount: settlement.traveller_amount,
            host_amount: settlement.host_amount,
            ops_amount: settlement.ops_amount,
        }
        .publish(&env);

        SettlementExecuted {
            booking_id,
            settlement_type: SettlementType::TravellerCancel,
            traveller_amount: settlement.traveller_amount,
            host_amount: settlement.host_amount,
            ops_amount: settlement.ops_amount,
            review_amount: 0,
            qa_amount: 0,
            o2o_amount: 0,
            timestamp: now,
        }
        .publish(&env);

        Ok(settlement)
    }

    /// Host cancellation while `Escrowed`. **Auth:** booking host only (not Stello).
    ///
    /// - 100% escrow is pushed to Traveller (not reduced by the fee).
    /// - Host pays `config.host_cancel_fee` USDC from Host wallet → Operations.
    /// - If the fee transfer fails, the whole call reverts atomically.
    pub fn cancel_by_host(env: Env, booking_id: u64) -> Result<CancelSettlement, Error> {
        let config = require_config(&env)?;
        let mut booking = require_booking(&env, booking_id)?;

        // Host must authorize — Stello cannot impersonate the host.
        booking.host.require_auth();

        if booking.was_cancelled || booking.state == BookingState::Cancelled {
            return Err(Error::AlreadyCancelled);
        }
        if booking.settled {
            return Err(Error::AlreadySettled);
        }
        if booking.state != BookingState::Escrowed {
            return Err(Error::InvalidStateTransition);
        }
        validate_transition(booking.state, BookingState::Cancelled)?;
        if booking.escrow_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let fee = config.host_cancel_fee;
        if fee <= 0 {
            return Err(Error::InvalidAmount);
        }

        require_solvency(&env, &config.token)?;

        let escrow = booking.escrow_amount;
        let settlement = CancelSettlement {
            traveller_amount: escrow,
            host_amount: 0,
            ops_amount: 0,
        };

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);

        // Fee from Host wallet → Ops first (NOT from escrow). If this fails, nothing settles.
        token_client.transfer(&booking.host, &config.ops_pool, &fee);

        // Effects after fee succeeds; escrow push follows (full tx still atomic).
        booking.state = BookingState::Cancelled;
        booking.was_cancelled = true;
        booking.cancelled_by = CancelledBy::Host;
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);
        set_cancel_settlement(&env, booking_id, &settlement);
        decrease_total_escrowed(&env, escrow)?;

        HostCancellationFeePaid {
            booking_id,
            host: booking.host.clone(),
            ops: config.ops_pool.clone(),
            amount: fee,
        }
        .publish(&env);

        transfer_from_contract(
            &token_client,
            &contract,
            &booking.traveller,
            settlement.traveller_amount,
        );

        BookingCancelled {
            booking_id,
            by_host: true,
            traveller_amount: settlement.traveller_amount,
            host_amount: settlement.host_amount,
            ops_amount: settlement.ops_amount,
        }
        .publish(&env);

        SettlementExecuted {
            booking_id,
            settlement_type: SettlementType::HostCancel,
            traveller_amount: settlement.traveller_amount,
            host_amount: 0,
            ops_amount: 0,
            review_amount: 0,
            qa_amount: 0,
            o2o_amount: 0,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(settlement)
    }

    /// Open a dispute while `Escrowed`. Freezes escrow (no fund release). **Auth:** Stello.
    pub fn open_dispute(env: Env, booking_id: u64) -> Result<(), Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;

        if booking.settled {
            return Err(Error::AlreadySettled);
        }
        if booking.state != BookingState::Escrowed {
            return Err(Error::InvalidStateTransition);
        }
        validate_transition(booking.state, BookingState::Disputed)?;
        if booking.escrow_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        booking.state = BookingState::Disputed;
        set_booking(&env, &booking);

        DisputeOpened { booking_id }.publish(&env);
        Ok(())
    }

    /// Resolve a dispute with custom BPS allocation (must sum to 10_000).
    /// Pushes escrow atomically to traveller/host/ops/review/qa/o2o. **Auth:** Stello.
    pub fn resolve_dispute(
        env: Env,
        booking_id: u64,
        traveller_bps: u32,
        host_bps: u32,
        ops_bps: u32,
        review_bps: u32,
        qa_bps: u32,
        o2o_bps: u32,
    ) -> Result<SettlementAmounts, Error> {
        let config = require_config(&env)?;
        require_stello(&config);
        let mut booking = require_booking(&env, booking_id)?;

        if booking.state != BookingState::Disputed {
            return Err(Error::InvalidDispute);
        }
        if booking.settled {
            return Err(Error::AlreadySettled);
        }
        validate_transition(booking.state, BookingState::Completed)?;
        if booking.escrow_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        require_solvency(&env, &config.token)?;

        let bps_sum = traveller_bps
            .checked_add(host_bps)
            .and_then(|v| v.checked_add(ops_bps))
            .and_then(|v| v.checked_add(review_bps))
            .and_then(|v| v.checked_add(qa_bps))
            .and_then(|v| v.checked_add(o2o_bps))
            .ok_or(Error::InvalidBpsAllocation)?;
        if bps_sum != BPS_DENOM {
            return Err(Error::InvalidBpsAllocation);
        }

        let escrow = booking.escrow_amount;
        let bps_list = [
            traveller_bps,
            host_bps,
            ops_bps,
            review_bps,
            qa_bps,
            o2o_bps,
        ];
        let mut amounts = [0i128; 6];
        compute_bps_amounts(escrow, &bps_list, &mut amounts)?;

        let traveller_amount = amounts[0];
        let host_amount = amounts[1];
        let ops_amount = amounts[2];
        let review_amount = amounts[3];
        let qa_amount = amounts[4];
        let o2o_amount = amounts[5];

        let total = traveller_amount
            .checked_add(host_amount)
            .and_then(|v| v.checked_add(ops_amount))
            .and_then(|v| v.checked_add(review_amount))
            .and_then(|v| v.checked_add(qa_amount))
            .and_then(|v| v.checked_add(o2o_amount))
            .ok_or(Error::MathError)?;
        if total != escrow {
            return Err(Error::MathError);
        }
        if traveller_amount > escrow
            || host_amount > escrow
            || ops_amount > escrow
            || review_amount > escrow
            || qa_amount > escrow
            || o2o_amount > escrow
        {
            return Err(Error::EscrowExceedsAmount);
        }

        booking.state = BookingState::Completed;
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);
        decrease_total_escrowed(&env, escrow)?;

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);
        transfer_from_contract(
            &token_client,
            &contract,
            &booking.traveller,
            traveller_amount,
        );
        transfer_from_contract(&token_client, &contract, &booking.host, host_amount);
        transfer_from_contract(&token_client, &contract, &config.ops_pool, ops_amount);
        transfer_from_contract(&token_client, &contract, &config.review_pool, review_amount);
        transfer_from_contract(&token_client, &contract, &config.qa_pool, qa_amount);
        transfer_from_contract(&token_client, &contract, &config.o2o_pool, o2o_amount);

        DisputeResolved {
            booking_id,
            traveller_bps,
            host_bps,
            ops_bps,
            review_bps,
            qa_bps,
            o2o_bps,
        }
        .publish(&env);

        SettlementExecuted {
            booking_id,
            settlement_type: SettlementType::Dispute,
            traveller_amount,
            host_amount,
            ops_amount,
            review_amount,
            qa_amount,
            o2o_amount,
            timestamp: env.ledger().timestamp(),
        }
        .publish(&env);

        Ok(SettlementAmounts {
            traveller_amount,
            host_amount,
            ops_amount,
            review_amount,
            qa_amount,
            o2o_amount,
        })
    }
}

fn require_stello(config: &Config) {
    config.stello_wallet.require_auth();
}

fn validate_config(config: &Config) -> Result<(), Error> {
    if config.host_cancel_fee <= 0 {
        return Err(Error::InvalidAmount);
    }
    // Addresses are always "present" in Soroban; require pairwise distinctness so
    // pools / wallets / token cannot silently collapse into one destination.
    if config.stello_wallet == config.token
        || config.ops_pool == config.token
        || config.review_pool == config.token
        || config.qa_pool == config.token
        || config.o2o_pool == config.token
    {
        return Err(Error::InvalidAddress);
    }
    if config.stello_wallet == config.ops_pool
        || config.stello_wallet == config.review_pool
        || config.stello_wallet == config.qa_pool
        || config.stello_wallet == config.o2o_pool
    {
        return Err(Error::InvalidAddress);
    }
    if config.ops_pool == config.review_pool
        || config.ops_pool == config.qa_pool
        || config.ops_pool == config.o2o_pool
        || config.review_pool == config.qa_pool
        || config.review_pool == config.o2o_pool
        || config.qa_pool == config.o2o_pool
    {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}

fn validate_start_time(env: &Env, start_time: u64) -> Result<(), Error> {
    if start_time <= env.ledger().timestamp() {
        return Err(Error::InvalidStartTime);
    }
    Ok(())
}

fn validate_parties(traveller: &Address, host: &Address) -> Result<(), Error> {
    if traveller == host {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}

/// Contract token balance must never be below accounted `total_escrowed`.
fn require_solvency(env: &Env, token_addr: &Address) -> Result<(), Error> {
    let contract = env.current_contract_address();
    let token_client = token::TokenClient::new(env, token_addr);
    let balance = token_client.balance(&contract);
    let total = get_total_escrowed(env);
    if balance < total {
        return Err(Error::Insolvent);
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
