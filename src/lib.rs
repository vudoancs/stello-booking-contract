#![no_std]
//! # StelloBookingContract
//!
//! Booking lifecycle on Soroban (book → USDC escrow → check-in → complete → settle).
//!
//! ## Roles
//! - **Stello wallet:** `book`, `update_booking`, `lock_escrow`, `check_in`, `complete`,
//!   `execute_split`, `cancel_by_traveller`, `open_dispute`, `resolve_dispute`.
//! - **Host wallet:** `cancel_by_host` (authorizes $5 fee from host, not escrow).
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
    CancelSettlementExecuted, DisputeOpened, DisputeResolved, EscrowLocked,
    HostCancellationFeePaid, PayoutClaimed, SettlementExecuted,
};
use storage::{
    get_cancel_settlement as load_cancel_settlement, get_claimable, get_config, next_booking_id,
    require_booking, require_config, set_booking, set_cancel_settlement, set_claimable, set_config,
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

    pub fn get_cancel_settlement(env: Env, booking_id: u64) -> Result<CancelSettlement, Error> {
        load_cancel_settlement(&env, booking_id).ok_or(Error::BookingNotFound)
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

    /// Traveller cancellation while `Escrowed`. **Auth:** Stello wallet only.
    ///
    /// Tiered refund from `start_time` vs ledger timestamp:
    /// - `>= 4 weeks`: Traveller 70% / Host 15% / Ops 15%
    /// - `2–4 weeks`: Traveller 50% / Host 35% / Ops 15%
    /// - `< 2 weeks`: Traveller 0% / Host 80% / Ops 20%
    ///
    /// Traveller, Host, and Ops shares are pushed atomically from escrow.
    /// Marks booking `Cancelled` + `settled`.
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

        let now = env.ledger().timestamp();
        let settlement = compute_refund(
            booking.escrow_amount,
            booking.start_time,
            now,
            CancelledBy::Traveller,
        )?;
        let total = settlement
            .traveller_amount
            .checked_add(settlement.host_amount)
            .and_then(|v| v.checked_add(settlement.ops_amount))
            .ok_or(Error::MathError)?;
        if total != booking.escrow_amount {
            return Err(Error::MathError);
        }

        // Effects (CEI): cancel + settle, store allocation, then push all shares.
        booking.state = BookingState::Cancelled;
        booking.was_cancelled = true;
        booking.cancelled_by = CancelledBy::Traveller;
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);
        set_cancel_settlement(&env, booking_id, &settlement);

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

        CancelSettlementExecuted {
            booking_id,
            traveller_amount: settlement.traveller_amount,
            host_amount: settlement.host_amount,
            ops_amount: settlement.ops_amount,
        }
        .publish(&env);

        Ok(settlement)
    }

    /// Host cancellation while `Escrowed`. **Auth:** booking host only (not Stello).
    ///
    /// - 100% escrow is pushed to Traveller (not reduced by the fee).
    /// - Host pays `config.host_cancel_fee` USDC from Host wallet → Operations.
    /// - If the fee transfer fails, the whole call reverts atomically (no host debt).
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

        let escrow = booking.escrow_amount;
        let settlement = CancelSettlement {
            traveller_amount: escrow,
            host_amount: 0,
            ops_amount: 0,
        };

        // Effects (CEI) before external transfers.
        booking.state = BookingState::Cancelled;
        booking.was_cancelled = true;
        booking.cancelled_by = CancelledBy::Host;
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);
        set_cancel_settlement(&env, booking_id, &settlement);

        let contract = env.current_contract_address();
        let token_client = token::TokenClient::new(&env, &config.token);

        // Fee from Host wallet → Ops (NOT from escrow). Fails atomically if insufficient.
        token_client.transfer(&booking.host, &config.ops_pool, &fee);
        HostCancellationFeePaid {
            booking_id,
            host: booking.host.clone(),
            ops: config.ops_pool.clone(),
            amount: fee,
        }
        .publish(&env);

        // 100% escrow → Traveller.
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

        CancelSettlementExecuted {
            booking_id,
            traveller_amount: settlement.traveller_amount,
            host_amount: settlement.host_amount,
            ops_amount: settlement.ops_amount,
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

        // Effects (CEI): settle + move to Completed, then push all shares.
        booking.state = BookingState::Completed;
        booking.settled = true;
        booking.escrow_amount = 0;
        set_booking(&env, &booking);

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

        let settlement_event = SettlementExecuted {
            booking_id,
            traveller_amount,
            host_amount,
            ops_amount,
            review_amount,
            qa_amount,
            o2o_amount,
        };
        settlement_event.publish(&env);

        Ok(SettlementAmounts {
            traveller_amount,
            host_amount,
            ops_amount,
            review_amount,
            qa_amount,
            o2o_amount,
        })
    }

    /// Withdraw claimable USDC (if any). **Auth:** claimant.
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
