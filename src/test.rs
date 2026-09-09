#![cfg(test)]
//! Lifecycle, escrow, settlement, and traveller-cancellation tests.
use soroban_sdk::{
    Address, Env, IntoVal,
    testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke},
    token,
};

use crate::{
    BookingState, Config, DEFAULT_HOST_CANCEL_FEE, Error, FOUR_WEEKS, StelloBookingContract,
    StelloBookingContractClient, TWO_WEEKS,
};

struct TestCtx {
    env: Env,
    contract_id: Address,
    token: Address,
    stello: Address,
    traveller: Address,
    host: Address,
    ops: Address,
    review: Address,
    qa: Address,
    o2o: Address,
}

impl TestCtx {
    fn client(&self) -> StelloBookingContractClient<'_> {
        StelloBookingContractClient::new(&self.env, &self.contract_id)
    }

    fn token_client(&self) -> token::TokenClient<'_> {
        token::TokenClient::new(&self.env, &self.token)
    }

    fn sac_admin(&self) -> token::StellarAssetClient<'_> {
        token::StellarAssetClient::new(&self.env, &self.token)
    }

    fn mint(&self, to: &Address, amount: i128) {
        self.sac_admin().mint(to, &amount);
    }

    fn fund_and_book(&self, amount: i128, start_time: u64) -> u64 {
        self.mint(&self.traveller, amount);
        self.client()
            .book(&self.traveller, &self.host, &amount, &start_time)
    }

    /// book → lock → check_in → complete (not settled).
    fn reach_completed(&self, amount: i128) -> u64 {
        let booking_id = self.fund_and_book(amount, 1_000_000);
        self.client().lock_escrow(&booking_id, &amount);
        self.client().check_in(&booking_id);
        self.client().complete(&booking_id);
        booking_id
    }

    /// book with start_time → lock escrow (Escrowed, ready to cancel).
    fn reach_escrowed(&self, amount: i128, start_time: u64) -> u64 {
        let booking_id = self.fund_and_book(amount, start_time);
        self.client().lock_escrow(&booking_id, &amount);
        booking_id
    }
}

fn setup() -> TestCtx {
    let env = Env::default();
    env.mock_all_auths();

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let token = sac.address();

    let contract_id = env.register(StelloBookingContract, ());
    let client = StelloBookingContractClient::new(&env, &contract_id);

    let cfg = Config {
        stello_wallet: stello.clone(),
        token: token.clone(),
        ops_pool: ops.clone(),
        review_pool: review.clone(),
        qa_pool: qa.clone(),
        o2o_pool: o2o.clone(),
        host_cancel_fee: DEFAULT_HOST_CANCEL_FEE,
    };
    client.initialize(&cfg);

    TestCtx {
        env,
        contract_id,
        token,
        stello,
        traveller,
        host,
        ops,
        review,
        qa,
        o2o,
    }
}

#[test]
fn initialize_once() {
    let ctx = setup();
    let got = ctx.client().get_config();
    assert_eq!(got.stello_wallet, ctx.stello);

    let cfg = Config {
        stello_wallet: ctx.stello.clone(),
        token: ctx.token.clone(),
        ops_pool: ctx.ops.clone(),
        review_pool: ctx.review.clone(),
        qa_pool: ctx.qa.clone(),
        o2o_pool: ctx.o2o.clone(),
        host_cancel_fee: DEFAULT_HOST_CANCEL_FEE,
    };
    assert_eq!(
        ctx.client().try_initialize(&cfg),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn valid_lifecycle_created_escrowed_checked_in_completed() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    assert_eq!(
        ctx.client().get_booking_state(&booking_id),
        BookingState::Created
    );

    ctx.client()
        .update_booking(&booking_id, &ctx.host, &amount, &2_000_000);
    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.start_time, 2_000_000);
    assert_eq!(b.state, BookingState::Created);

    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(
        ctx.client().get_booking_state(&booking_id),
        BookingState::Escrowed
    );
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), 0);

    ctx.client().check_in(&booking_id);
    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::CheckedIn);
    assert!(b.checked_in);

    // Escrow remains in the contract — settlement is deferred.
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);

    ctx.client().complete(&booking_id);
    assert_eq!(
        ctx.client().get_booking_state(&booking_id),
        BookingState::Completed
    );
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
}

#[test]
fn successful_escrow() {
    let ctx = setup();
    let amount = 50_000_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);

    ctx.client().lock_escrow(&booking_id, &amount);

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Escrowed);
    assert!(b.escrow_locked);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), 0);
}

#[test]
fn escrow_rejects_incorrect_amount() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);

    assert_eq!(
        ctx.client().try_lock_escrow(&booking_id, &(amount + 1)),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        ctx.client().try_lock_escrow(&booking_id, &(amount - 1)),
        Err(Ok(Error::InvalidAmount))
    );

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Created);
    assert_eq!(b.escrow_amount, 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), amount);
}

#[test]
fn escrow_rejects_double_lock() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);

    assert_eq!(
        ctx.client().try_lock_escrow(&booking_id, &amount),
        Err(Ok(Error::InvalidStateTransition))
    );

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
}

#[test]
fn escrow_rejects_zero_amount() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);

    assert_eq!(
        ctx.client().try_lock_escrow(&booking_id, &0i128),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(ctx.client().get_booking(&booking_id).escrow_amount, 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

#[test]
fn escrow_rejects_unauthorized_caller() {
    let env = Env::default();
    // Do not mock_all_auths — only authorize initialize + book + traveller path partially.

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer.clone());
    let token = sac.address();
    let sac_admin = token::StellarAssetClient::new(&env, &token);
    let contract_id = env.register(StelloBookingContract, ());
    let client = StelloBookingContractClient::new(&env, &contract_id);

    let cfg = Config {
        stello_wallet: stello.clone(),
        token: token.clone(),
        ops_pool: ops,
        review_pool: review,
        qa_pool: qa,
        o2o_pool: o2o,
        host_cancel_fee: DEFAULT_HOST_CANCEL_FEE,
    };

    env.mock_auths(&[MockAuth {
        address: &stello,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (cfg.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&cfg);

    let amount = 100i128;
    env.mock_auths(&[MockAuth {
        address: &stello,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "book",
            args: (traveller.clone(), host.clone(), amount, 1_000_000u64).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    // Mint needs issuer/admin auth — use mock_all briefly for mint only via StellarAssetClient.
    // StellarAssetClient mint with mock_all is easier:
    env.mock_all_auths();
    sac_admin.mint(&traveller, &amount);
    let booking_id = client.book(&traveller, &host, &amount, &1_000_000u64);

    // Clear auths: lock_escrow without Stello authorization must fail.
    env.set_auths(&[]);
    let result = client.try_lock_escrow(&booking_id, &amount);
    assert!(result.is_err(), "expected unauthorized lock_escrow to fail");
}

#[test]
fn escrow_rejects_insufficient_balance() {
    let ctx = setup();
    let amount = 100i128;
    // Book without funding traveller fully.
    let booking_id = ctx
        .client()
        .book(&ctx.traveller, &ctx.host, &amount, &1_000_000u64);
    ctx.mint(&ctx.traveller, amount - 1);

    let result = ctx.client().try_lock_escrow(&booking_id, &amount);
    assert!(result.is_err(), "expected insufficient balance to fail");
    assert_eq!(
        ctx.client().get_booking(&booking_id).state,
        BookingState::Created
    );
    assert_eq!(ctx.client().get_booking(&booking_id).escrow_amount, 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

#[test]
fn escrow_balance_tracking() {
    let ctx = setup();
    let amount = 25_000_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);

    assert_eq!(ctx.client().get_booking(&booking_id).escrow_amount, 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);

    ctx.client().lock_escrow(&booking_id, &amount);

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(b.escrow_amount, b.amount);
    assert_eq!(
        ctx.token_client().balance(&ctx.contract_id),
        b.escrow_amount
    );
    assert!(b.escrow_amount <= b.amount);
}

#[test]
fn escrow_rejects_invalid_booking() {
    let ctx = setup();
    assert_eq!(
        ctx.client().try_lock_escrow(&999u64, &100i128),
        Err(Ok(Error::BookingNotFound))
    );
}

#[test]
fn reject_lock_escrow_from_non_created() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(
        ctx.client().try_lock_escrow(&booking_id, &amount),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_check_in_from_created() {
    let ctx = setup();
    let booking_id = ctx.fund_and_book(100, 1_000_000);
    assert_eq!(
        ctx.client().try_check_in(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_check_in_from_checked_in_and_completed() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);
    ctx.client().check_in(&booking_id);
    assert_eq!(
        ctx.client().try_check_in(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );

    ctx.client().complete(&booking_id);
    assert_eq!(
        ctx.client().try_check_in(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_complete_from_created() {
    let ctx = setup();
    let booking_id = ctx
        .client()
        .book(&ctx.traveller, &ctx.host, &100i128, &1_000_000);
    assert_eq!(
        ctx.client().try_complete(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_complete_from_escrowed_without_check_in() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(
        ctx.client().try_complete(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_complete_twice() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);
    ctx.client().check_in(&booking_id);
    ctx.client().complete(&booking_id);
    assert_eq!(
        ctx.client().try_complete(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_update_after_escrow_locked() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(
        ctx.client()
            .try_update_booking(&booking_id, &ctx.host, &amount, &2_000_000),
        Err(Ok(Error::InvalidUpdate))
    );
}

#[test]
fn reject_book_identical_traveller_host() {
    let ctx = setup();
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.traveller, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn reject_book_non_positive_amount() {
    let ctx = setup();
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.host, &0i128, &1_000_000),
        Err(Ok(Error::InvalidAmount))
    );
}

// --- Step 4: Completed settlement ---

#[test]
fn execute_split_successful_100_units() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_completed(amount);

    let split = ctx.client().execute_split(&booking_id);
    assert_eq!(split.host_amount, 80);
    assert_eq!(split.ops_amount, 10);
    assert_eq!(split.review_amount, 5);
    assert_eq!(split.qa_amount, 3);
    assert_eq!(split.o2o_amount, 2);
    assert_eq!(
        split.host_amount
            + split.ops_amount
            + split.review_amount
            + split.qa_amount
            + split.o2o_amount,
        amount
    );

    let b = ctx.client().get_booking(&booking_id);
    assert!(b.settled);
    assert_eq!(b.escrow_amount, 0);
    assert_eq!(b.state, BookingState::Completed);

    // All five shares are pushed immediately.
    assert_eq!(ctx.token_client().balance(&ctx.host), 80);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 10);
    assert_eq!(ctx.token_client().balance(&ctx.review), 5);
    assert_eq!(ctx.token_client().balance(&ctx.qa), 3);
    assert_eq!(ctx.token_client().balance(&ctx.o2o), 2);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
    assert_eq!(ctx.client().get_claimable_balance(&ctx.host), 0);
    assert_eq!(ctx.client().get_claimable_balance(&ctx.o2o), 0);
}

#[test]
fn execute_split_dust_remainder_to_host() {
    let ctx = setup();
    let amount = 50i128;
    let booking_id = ctx.reach_completed(amount);

    let split = ctx.client().execute_split(&booking_id);
    // floors 40+5+2+1+1=49, remainder 1 → host 41
    assert_eq!(split.host_amount, 41);
    assert_eq!(split.ops_amount, 5);
    assert_eq!(split.review_amount, 2);
    assert_eq!(split.qa_amount, 1);
    assert_eq!(split.o2o_amount, 1);
    assert_eq!(
        split.host_amount
            + split.ops_amount
            + split.review_amount
            + split.qa_amount
            + split.o2o_amount,
        amount
    );
}

#[test]
fn execute_split_rejects_before_completed() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );

    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );

    ctx.client().check_in(&booking_id);
    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn execute_split_rejects_double_settlement() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_completed(amount);
    ctx.client().execute_split(&booking_id);

    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::AlreadySettled))
    );
}

#[test]
fn execute_split_rejects_unknown_booking() {
    let ctx = setup();
    assert_eq!(
        ctx.client().try_execute_split(&999u64),
        Err(Ok(Error::BookingNotFound))
    );
}

#[test]
fn claim_payout_rejects_nothing_to_claim_after_push_settlement() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_completed(amount);
    ctx.client().execute_split(&booking_id);

    // Host/O2O were pushed; nothing left to claim.
    assert_eq!(
        ctx.client().try_claim_payout(&ctx.host),
        Err(Ok(Error::NothingToClaim))
    );
    assert_eq!(
        ctx.client().try_claim_payout(&ctx.o2o),
        Err(Ok(Error::NothingToClaim))
    );
}

#[test]
fn claim_payout_rejects_nothing_to_claim() {
    let ctx = setup();
    assert_eq!(
        ctx.client().try_claim_payout(&ctx.host),
        Err(Ok(Error::NothingToClaim))
    );
}

#[test]
fn full_lifecycle_with_settlement_and_claims() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.reach_completed(amount);

    let split = ctx.client().execute_split(&booking_id);
    assert_eq!(split.host_amount, 80_000_000);
    assert_eq!(split.ops_amount, 10_000_000);
    assert_eq!(split.review_amount, 5_000_000);
    assert_eq!(split.qa_amount, 3_000_000);
    assert_eq!(split.o2o_amount, 2_000_000);

    assert_eq!(ctx.token_client().balance(&ctx.host), 80_000_000);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 10_000_000);
    assert_eq!(ctx.token_client().balance(&ctx.review), 5_000_000);
    assert_eq!(ctx.token_client().balance(&ctx.qa), 3_000_000);
    assert_eq!(ctx.token_client().balance(&ctx.o2o), 2_000_000);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

// --- Step 5: Traveller cancellation ---

#[test]
fn traveller_cancel_exactly_four_weeks() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, FOUR_WEEKS);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 70);
    assert_eq!(s.host_amount, 15);
    assert_eq!(s.ops_amount, 15);

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Cancelled);
    assert!(b.settled);
    assert!(b.was_cancelled);
    assert_eq!(b.escrow_amount, 0);

    let stored = ctx.client().get_cancel_settlement(&booking_id);
    assert_eq!(stored, s);

    assert_eq!(ctx.token_client().balance(&ctx.traveller), 70);
    assert_eq!(ctx.token_client().balance(&ctx.host), 15);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 15);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

#[test]
fn traveller_cancel_just_under_four_weeks() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, FOUR_WEEKS - 1);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 50);
    assert_eq!(s.host_amount, 35);
    assert_eq!(s.ops_amount, 15);
}

#[test]
fn traveller_cancel_exactly_two_weeks() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, TWO_WEEKS);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 50);
    assert_eq!(s.host_amount, 35);
    assert_eq!(s.ops_amount, 15);
}

#[test]
fn traveller_cancel_just_under_two_weeks() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, TWO_WEEKS - 1);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 0);
    assert_eq!(s.host_amount, 80);
    assert_eq!(s.ops_amount, 20);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), 0);
    assert_eq!(ctx.token_client().balance(&ctx.host), 80);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 20);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

#[test]
fn traveller_cancel_less_than_two_weeks() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, TWO_WEEKS / 2);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 0);
    assert_eq!(s.host_amount, 80);
    assert_eq!(s.ops_amount, 20);
}

#[test]
fn traveller_cancel_rejects_invalid_state() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, FOUR_WEEKS);
    assert_eq!(
        ctx.client().try_cancel_by_traveller(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );

    let booking_id = ctx.reach_completed(amount);
    assert_eq!(
        ctx.client().try_cancel_by_traveller(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn traveller_cancel_rejects_repeated_cancellation() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, FOUR_WEEKS);
    ctx.client().cancel_by_traveller(&booking_id);

    assert_eq!(
        ctx.client().try_cancel_by_traveller(&booking_id),
        Err(Ok(Error::AlreadyCancelled))
    );
    // Also blocked from completion settlement.
    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn traveller_cancel_rejects_unauthorized_caller() {
    let env = Env::default();
    env.mock_all_auths();

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let token = sac.address();
    let sac_admin = token::StellarAssetClient::new(&env, &token);
    let contract_id = env.register(StelloBookingContract, ());
    let client = StelloBookingContractClient::new(&env, &contract_id);

    let cfg = Config {
        stello_wallet: stello.clone(),
        token: token.clone(),
        ops_pool: ops,
        review_pool: review,
        qa_pool: qa,
        o2o_pool: o2o,
        host_cancel_fee: DEFAULT_HOST_CANCEL_FEE,
    };
    client.initialize(&cfg);

    let amount = 100i128;
    sac_admin.mint(&traveller, &amount);
    let booking_id = client.book(&traveller, &host, &amount, &FOUR_WEEKS);
    client.lock_escrow(&booking_id, &amount);

    env.set_auths(&[]);
    let result = client.try_cancel_by_traveller(&booking_id);
    assert!(
        result.is_err(),
        "expected unauthorized cancel_by_traveller to fail"
    );
}

// --- Step 6: Host cancellation ---

#[test]
fn host_cancel_successfully() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);

    // Fund host separately for $5 fee — not from escrow.
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE);
    let contract_before = ctx.token_client().balance(&ctx.contract_id);
    assert_eq!(contract_before, amount);

    let s = ctx.client().cancel_by_host(&booking_id);
    assert_eq!(s.traveller_amount, amount);
    assert_eq!(s.host_amount, 0);
    assert_eq!(s.ops_amount, 0);

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Cancelled);
    assert!(b.settled);
    assert_eq!(b.cancelled_by, crate::CancelledBy::Host);
    assert_eq!(b.escrow_amount, 0);

    // Traveller receives 100% escrow; ops receives exactly $5 from host wallet.
    assert_eq!(ctx.token_client().balance(&ctx.traveller), amount);
    assert_eq!(
        ctx.token_client().balance(&ctx.ops),
        DEFAULT_HOST_CANCEL_FEE
    );
    assert_eq!(ctx.token_client().balance(&ctx.host), 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);

    let stored = ctx.client().get_cancel_settlement(&booking_id);
    assert_eq!(stored.traveller_amount, amount);
}

#[test]
fn host_cancel_escrow_untouched_by_fee() {
    let ctx = setup();
    let amount = 80_000_000i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE * 2);

    let escrow_before = ctx.token_client().balance(&ctx.contract_id);
    let host_before = ctx.token_client().balance(&ctx.host);
    assert_eq!(escrow_before, amount);

    ctx.client().cancel_by_host(&booking_id);

    // Escrow amount went entirely to traveller; fee came only from host wallet.
    assert_eq!(ctx.token_client().balance(&ctx.traveller), escrow_before);
    assert_eq!(
        ctx.token_client().balance(&ctx.host),
        host_before - DEFAULT_HOST_CANCEL_FEE
    );
    assert_eq!(
        ctx.token_client().balance(&ctx.ops),
        DEFAULT_HOST_CANCEL_FEE
    );
}

#[test]
fn host_cancel_ops_receives_exactly_five_dollars() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE);

    ctx.client().cancel_by_host(&booking_id);
    assert_eq!(
        ctx.token_client().balance(&ctx.ops),
        DEFAULT_HOST_CANCEL_FEE
    );
}

#[test]
fn host_cancel_rejects_insufficient_fee() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    // Host has less than $5.
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE - 1);

    let result = ctx.client().try_cancel_by_host(&booking_id);
    assert!(result.is_err(), "expected insufficient $5 fee to fail");

    // Atomic: booking still Escrowed, escrow intact, no fee paid.
    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Escrowed);
    assert!(!b.settled);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 0);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), 0);
}

#[test]
fn host_cancel_rejects_non_host_without_auth() {
    let env = Env::default();
    env.mock_all_auths();

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let token = sac.address();
    let sac_admin = token::StellarAssetClient::new(&env, &token);
    let contract_id = env.register(StelloBookingContract, ());
    let client = StelloBookingContractClient::new(&env, &contract_id);

    client.initialize(&Config {
        stello_wallet: stello,
        token: token.clone(),
        ops_pool: ops.clone(),
        review_pool: review,
        qa_pool: qa,
        o2o_pool: o2o,
        host_cancel_fee: DEFAULT_HOST_CANCEL_FEE,
    });

    let amount = 100i128;
    sac_admin.mint(&traveller, &amount);
    sac_admin.mint(&host, &DEFAULT_HOST_CANCEL_FEE);
    let booking_id = client.book(&traveller, &host, &amount, &1_000_000u64);
    client.lock_escrow(&booking_id, &amount);

    // Clear auths: without host signature, cancel_by_host must fail.
    env.set_auths(&[]);
    let result = client.try_cancel_by_host(&booking_id);
    assert!(
        result.is_err(),
        "expected non-host / missing host auth to fail"
    );
}

#[test]
fn host_cancel_rejects_double_cancellation() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE * 2);
    ctx.client().cancel_by_host(&booking_id);

    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

// --- Step 7: Dispute resolution ---

#[test]
fn open_dispute_freezes_escrow() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);

    ctx.client().open_dispute(&booking_id);
    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Disputed);
    assert!(!b.settled);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);

    // Escrow frozen: cancel / check_in / complete / execute_split blocked.
    assert_eq!(
        ctx.client().try_cancel_by_traveller(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
    assert_eq!(
        ctx.client().try_check_in(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
    assert_eq!(
        ctx.client().try_complete(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn resolve_dispute_custom_bps_pushes_all_parties() {
    let ctx = setup();
    let amount = 10_000i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.client().open_dispute(&booking_id);

    let settlement = ctx.client().resolve_dispute(
        &booking_id,
        &4000u32, // traveller
        &3000u32, // host
        &1000u32, // ops
        &1000u32, // review
        &500u32,  // qa
        &500u32,  // o2o
    );

    assert_eq!(settlement.traveller_amount, 4000);
    assert_eq!(settlement.host_amount, 3000);
    assert_eq!(settlement.ops_amount, 1000);
    assert_eq!(settlement.review_amount, 1000);
    assert_eq!(settlement.qa_amount, 500);
    assert_eq!(settlement.o2o_amount, 500);
    assert_eq!(
        settlement.traveller_amount
            + settlement.host_amount
            + settlement.ops_amount
            + settlement.review_amount
            + settlement.qa_amount
            + settlement.o2o_amount,
        amount
    );

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Completed);
    assert!(b.settled);
    assert_eq!(b.escrow_amount, 0);

    assert_eq!(ctx.token_client().balance(&ctx.traveller), 4000);
    assert_eq!(ctx.token_client().balance(&ctx.host), 3000);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 1000);
    assert_eq!(ctx.token_client().balance(&ctx.review), 1000);
    assert_eq!(ctx.token_client().balance(&ctx.qa), 500);
    assert_eq!(ctx.token_client().balance(&ctx.o2o), 500);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

#[test]
fn resolve_dispute_remainder_goes_to_traveller() {
    let ctx = setup();
    let amount = 50i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.client().open_dispute(&booking_id);

    let settlement = ctx
        .client()
        .resolve_dispute(&booking_id, &8000, &1000, &500, &300, &200, &0);
    assert_eq!(
        settlement.traveller_amount
            + settlement.host_amount
            + settlement.ops_amount
            + settlement.review_amount
            + settlement.qa_amount
            + settlement.o2o_amount,
        amount
    );
}

#[test]
fn resolve_dispute_rejects_invalid_bps_sum() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.client().open_dispute(&booking_id);

    assert_eq!(
        ctx.client()
            .try_resolve_dispute(&booking_id, &5000, &4000, &0, &0, &0, &0),
        Err(Ok(Error::InvalidBpsAllocation))
    );
    assert_eq!(
        ctx.client().get_booking(&booking_id).state,
        BookingState::Disputed
    );
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
}

#[test]
fn resolve_dispute_rejects_non_disputed() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);

    assert_eq!(
        ctx.client()
            .try_resolve_dispute(&booking_id, &10000, &0, &0, &0, &0, &0),
        Err(Ok(Error::InvalidDispute))
    );
}

#[test]
fn resolve_dispute_rejects_twice() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.client().open_dispute(&booking_id);
    ctx.client()
        .resolve_dispute(&booking_id, &10000, &0, &0, &0, &0, &0);

    assert_eq!(
        ctx.client()
            .try_resolve_dispute(&booking_id, &10000, &0, &0, &0, &0, &0),
        Err(Ok(Error::InvalidDispute))
    );
    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::AlreadySettled))
    );
}

#[test]
fn open_dispute_rejects_invalid_state() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    assert_eq!(
        ctx.client().try_open_dispute(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );

    let booking_id = ctx.reach_completed(amount);
    assert_eq!(
        ctx.client().try_open_dispute(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn open_dispute_rejects_unauthorized() {
    let env = Env::default();
    env.mock_all_auths();

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let token = sac.address();
    let sac_admin = token::StellarAssetClient::new(&env, &token);
    let contract_id = env.register(StelloBookingContract, ());
    let client = StelloBookingContractClient::new(&env, &contract_id);

    client.initialize(&Config {
        stello_wallet: stello,
        token: token.clone(),
        ops_pool: ops,
        review_pool: review,
        qa_pool: qa,
        o2o_pool: o2o,
        host_cancel_fee: DEFAULT_HOST_CANCEL_FEE,
    });

    let amount = 100i128;
    sac_admin.mint(&traveller, &amount);
    let booking_id = client.book(&traveller, &host, &amount, &1_000_000u64);
    client.lock_escrow(&booking_id, &amount);

    env.set_auths(&[]);
    assert!(client.try_open_dispute(&booking_id).is_err());
    assert!(
        client
            .try_resolve_dispute(&booking_id, &10000, &0, &0, &0, &0, &0)
            .is_err()
    );
}
