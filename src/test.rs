#![cfg(test)]
//! Lifecycle, escrow, settlement, TTL, constructor, auth, and atomicity tests.
use soroban_sdk::{
    Address, Env, IntoVal,
    testutils::{
        Address as _, Ledger, MockAuth, MockAuthInvoke,
        storage::{Instance as _, Persistent as _},
    },
    token,
};

use crate::{
    BookingState, DEFAULT_HOST_CANCEL_FEE, Error, FOUR_WEEKS, INSTANCE_TTL_EXTEND_TO,
    PERSISTENT_BOOKING_TTL_EXTEND_TO, PERSISTENT_SETTLEMENT_TTL_EXTEND_TO, StelloBookingContract,
    StelloBookingContractClient, TWO_WEEKS,
    storage::DataKey,
    test_token::{ScriptedToken, ScriptedTokenClient},
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

fn register_contract(
    env: &Env,
    stello: &Address,
    token: &Address,
    ops: &Address,
    review: &Address,
    qa: &Address,
    o2o: &Address,
    host_cancel_fee: i128,
) -> Address {
    env.register(
        StelloBookingContract,
        (
            stello.clone(),
            token.clone(),
            ops.clone(),
            review.clone(),
            qa.clone(),
            o2o.clone(),
            host_cancel_fee,
        ),
    )
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

    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );

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
fn constructor_sets_config_atomically() {
    let ctx = setup();
    let got = ctx.client().get_config();
    assert_eq!(got.stello_wallet, ctx.stello);
    assert_eq!(got.token, ctx.token);
    assert_eq!(got.ops_pool, ctx.ops);
    assert_eq!(got.review_pool, ctx.review);
    assert_eq!(got.qa_pool, ctx.qa);
    assert_eq!(got.o2o_pool, ctx.o2o);
    assert_eq!(got.host_cancel_fee, DEFAULT_HOST_CANCEL_FEE);
    assert_eq!(ctx.client().get_total_escrowed(), 0);
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
    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);

    let amount = 100i128;
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
    assert_eq!(ctx.client().get_total_escrowed(), 0);
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
fn full_lifecycle_push_settlement() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.reach_completed(amount);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

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
    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.client().get_booking(&booking_id).escrow_amount, 0);
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
    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);

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
    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);

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
        ctx.client().try_cancel_by_host(&booking_id),
        Err(Ok(Error::AlreadyCancelled))
    );
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

    // CheckedIn (not Escrowed/Completed) cannot open dispute.
    ctx.client().lock_escrow(&booking_id, &amount);
    ctx.client().check_in(&booking_id);
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
    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);

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

// --- Push-only accounting & hardening ---

#[test]
fn total_escrowed_increases_on_lock_decreases_on_settlement() {
    let ctx = setup();
    let amount = 100i128;
    assert_eq!(ctx.client().get_total_escrowed(), 0);

    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    assert_eq!(ctx.client().get_total_escrowed(), 0);

    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    ctx.client().check_in(&booking_id);
    ctx.client().complete(&booking_id);
    ctx.client().execute_split(&booking_id);

    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.client().get_booking(&booking_id).escrow_amount, 0);
    assert!(ctx.client().get_booking(&booking_id).settled);
}

#[test]
fn traveller_cancel_decreases_total_escrowed() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, FOUR_WEEKS);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.client().get_booking(&booking_id).escrow_amount, 0);
}

#[test]
fn host_cancel_decreases_total_escrowed_and_pushes_refund() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    ctx.client().cancel_by_host(&booking_id);
    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), amount);
    assert_eq!(
        ctx.token_client().balance(&ctx.ops),
        DEFAULT_HOST_CANCEL_FEE
    );
}

#[test]
fn dispute_settlement_decreases_total_escrowed() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.client().open_dispute(&booking_id);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    ctx.client()
        .resolve_dispute(&booking_id, &10000, &0, &0, &0, &0, &0);
    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), amount);
}

#[test]
fn multiple_bookings_escrow_isolation() {
    let ctx = setup();
    let amount_a = 100i128;
    let amount_b = 200i128;

    let id_a = ctx.fund_and_book(amount_a, 1_000_000);
    let id_b = ctx.fund_and_book(amount_b, 2_000_000);
    ctx.client().lock_escrow(&id_a, &amount_a);
    ctx.client().lock_escrow(&id_b, &amount_b);

    assert_eq!(ctx.client().get_total_escrowed(), amount_a + amount_b);
    assert_eq!(
        ctx.token_client().balance(&ctx.contract_id),
        amount_a + amount_b
    );

    ctx.client().check_in(&id_a);
    ctx.client().complete(&id_a);
    ctx.client().execute_split(&id_a);

    // Settling A must not touch B's escrow.
    assert_eq!(ctx.client().get_total_escrowed(), amount_b);
    assert_eq!(ctx.client().get_booking(&id_b).escrow_amount, amount_b);
    assert!(!ctx.client().get_booking(&id_b).settled);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount_b);

    ctx.client().check_in(&id_b);
    ctx.client().complete(&id_b);
    ctx.client().execute_split(&id_b);
    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
}

#[test]
fn unsolicited_usdc_does_not_modify_total_escrowed() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    // Extra tokens sent to the contract outside lock_escrow.
    let extra = 50i128;
    ctx.mint(&ctx.stello, extra);
    ctx.token_client()
        .transfer(&ctx.stello, &ctx.contract_id, &extra);

    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount + extra);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    ctx.client().check_in(&booking_id);
    ctx.client().complete(&booking_id);
    ctx.client().execute_split(&booking_id);

    // Settlement only spends booking escrow; leftover unsolicited remains.
    assert_eq!(ctx.client().get_total_escrowed(), 0);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), extra);
}

#[test]
fn insufficient_contract_balance_causes_settlement_failure() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_completed(amount);
    assert_eq!(ctx.client().get_total_escrowed(), amount);

    // Inflate accounted escrow above token balance to trip solvency check.
    ctx.env.as_contract(&ctx.contract_id, || {
        crate::storage::set_total_escrowed(&ctx.env, amount + 1);
    });

    assert_eq!(
        ctx.client().try_execute_split(&booking_id),
        Err(Ok(Error::Insolvent))
    );

    let b = ctx.client().get_booking(&booking_id);
    assert!(!b.settled);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), amount);
}

#[test]
fn host_cancel_insufficient_fee_leaves_total_escrowed_unchanged() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE - 1);

    assert!(ctx.client().try_cancel_by_host(&booking_id).is_err());
    assert_eq!(ctx.client().get_total_escrowed(), amount);
    assert!(!ctx.client().get_booking(&booking_id).settled);
}

#[test]
fn book_rejects_start_time_in_the_past() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 1_000);
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.host, &100i128, &1_000u64),
        Err(Ok(Error::InvalidStartTime))
    );
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.host, &100i128, &999u64),
        Err(Ok(Error::InvalidStartTime))
    );
}

#[test]
fn update_booking_rejects_invalid_start_time() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 500);
    let booking_id = ctx
        .client()
        .book(&ctx.traveller, &ctx.host, &100i128, &1_000u64);

    assert_eq!(
        ctx.client()
            .try_update_booking(&booking_id, &ctx.host, &100i128, &500u64),
        Err(Ok(Error::InvalidStartTime))
    );
}

#[test]
fn start_time_cannot_change_after_escrowed() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id, &amount);

    assert_eq!(
        ctx.client()
            .try_update_booking(&booking_id, &ctx.host, &amount, &9_000_000),
        Err(Ok(Error::InvalidUpdate))
    );
    assert_eq!(ctx.client().get_booking(&booking_id).start_time, 1_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn constructor_rejects_non_positive_host_cancel_fee() {
    let env = Env::default();
    env.mock_all_auths();
    let stello = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let _ = register_contract(
        &env,
        &stello,
        &token,
        &Address::generate(&env),
        &Address::generate(&env),
        &Address::generate(&env),
        &Address::generate(&env),
        0,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn constructor_rejects_duplicate_pool_addresses() {
    let env = Env::default();
    env.mock_all_auths();
    let stello = Address::generate(&env);
    let shared = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let _ = register_contract(
        &env,
        &stello,
        &token,
        &shared,
        &shared,
        &Address::generate(&env),
        &Address::generate(&env),
        DEFAULT_HOST_CANCEL_FEE,
    );
}

// --- TTL (testutils get_ttl; not a full network archival simulation) ---
//
// Limitation: Protocol 23+ test envs auto-restore archived persistent/instance
// entries on access, so we do not assert hard archival failures. We verify
// extend_ttl targets via get_ttl and that reads succeed after advancing the
// ledger sequence within the extended TTL window.

#[test]
fn ttl_extended_on_booking_and_instance() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);

    ctx.env.as_contract(&ctx.contract_id, || {
        let booking_ttl = ctx
            .env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Booking(booking_id));
        assert_eq!(booking_ttl, PERSISTENT_BOOKING_TTL_EXTEND_TO);

        let instance_ttl = ctx.env.storage().instance().get_ttl();
        assert_eq!(instance_ttl, INSTANCE_TTL_EXTEND_TO);
    });

    // Advance within extended TTL — booking + config + accounting still readable.
    let seq = ctx.env.ledger().sequence();
    ctx.env.ledger().set_sequence_number(seq + 10_000);

    let b = ctx.client().get_booking(&booking_id);
    assert_eq!(b.booking_id, booking_id);
    assert_eq!(ctx.client().get_config().stello_wallet, ctx.stello);
    assert_eq!(ctx.client().get_total_escrowed(), 0);

    ctx.client().lock_escrow(&booking_id, &amount);
    assert_eq!(ctx.client().get_total_escrowed(), amount);
}

#[test]
fn ttl_extended_on_cancel_settlement() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, FOUR_WEEKS);
    ctx.client().cancel_by_traveller(&booking_id);

    ctx.env.as_contract(&ctx.contract_id, || {
        let ttl = ctx
            .env
            .storage()
            .persistent()
            .get_ttl(&DataKey::CancelSettlement(booking_id));
        assert_eq!(ttl, PERSISTENT_SETTLEMENT_TTL_EXTEND_TO);
    });

    let seq = ctx.env.ledger().sequence();
    ctx.env.ledger().set_sequence_number(seq + 5_000);
    let s = ctx.client().get_cancel_settlement(&booking_id);
    assert_eq!(s.traveller_amount, 70);
}

// --- State-machine regression ---

#[test]
fn cancel_by_host_while_disputed_fails() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE);
    ctx.client().open_dispute(&booking_id);

    assert_eq!(
        ctx.client().try_cancel_by_host(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
    assert_eq!(
        ctx.client().get_booking(&booking_id).state,
        BookingState::Disputed
    );
    assert_eq!(ctx.client().get_total_escrowed(), amount);
}

#[test]
fn open_dispute_after_cancellation_fails() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, FOUR_WEEKS);
    ctx.client().cancel_by_traveller(&booking_id);

    assert_eq!(
        ctx.client().try_open_dispute(&booking_id),
        Err(Ok(Error::AlreadySettled))
    );
}

#[test]
fn cancel_by_traveller_after_open_dispute_fails() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_escrowed(amount, 1_000_000);
    ctx.client().open_dispute(&booking_id);

    assert_eq!(
        ctx.client().try_cancel_by_traveller(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn update_booking_rejects_traveller_equals_host() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    assert_eq!(
        ctx.client()
            .try_update_booking(&booking_id, &ctx.traveller, &amount, &2_000_000u64),
        Err(Ok(Error::InvalidAddress))
    );
}

// --- Failed push recovery (Completed → Disputed) ---

#[test]
fn failed_execute_split_leaves_completed_unsettled_then_dispute_recovers() {
    // Limitation: SAC testutils cannot model classic trustline rejects.
    // ScriptedToken refuses the last completion recipient (o2o) to force late failure.
    let env = Env::default();
    env.mock_all_auths();

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let token = env.register(ScriptedToken, ());
    let tok = ScriptedTokenClient::new(&env, &token);

    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);

    let amount = 100i128;
    tok.mint(&traveller, &amount);
    let booking_id = client.book(&traveller, &host, &amount, &1_000_000u64);
    client.lock_escrow(&booking_id, &amount);
    client.check_in(&booking_id);
    client.complete(&booking_id);

    assert_eq!(client.get_total_escrowed(), amount);
    assert_eq!(tok.balance(&contract_id), amount);

    // Fail on the last push recipient (o2o) after earlier shares would succeed.
    tok.arm_fail_to(&o2o);
    assert!(
        client.try_execute_split(&booking_id).is_err(),
        "expected late o2o transfer failure"
    );

    let b = client.get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Completed);
    assert!(!b.settled);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(client.get_total_escrowed(), amount);
    // No partial payouts retained.
    assert_eq!(tok.balance(&contract_id), amount);
    assert_eq!(tok.balance(&host), 0);
    assert_eq!(tok.balance(&ops), 0);
    assert_eq!(tok.balance(&review), 0);
    assert_eq!(tok.balance(&qa), 0);
    assert_eq!(tok.balance(&o2o), 0);

    // Recovery: open dispute from Completed + unsettled.
    client.open_dispute(&booking_id);
    assert_eq!(
        client.get_booking(&booking_id).state,
        BookingState::Disputed
    );

    tok.disarm();
    let settlement = client.resolve_dispute(&booking_id, &10000, &0, &0, &0, &0, &0);
    assert_eq!(settlement.traveller_amount, amount);
    assert!(client.get_booking(&booking_id).settled);
    assert_eq!(client.get_booking(&booking_id).escrow_amount, 0);
    assert_eq!(client.get_total_escrowed(), 0);
    assert_eq!(tok.balance(&traveller), amount);
}

#[test]
fn open_dispute_rejected_after_successful_settlement() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_completed(amount);
    ctx.client().execute_split(&booking_id);
    assert_eq!(
        ctx.client().try_open_dispute(&booking_id),
        Err(Ok(Error::AlreadySettled))
    );
}

#[test]
fn open_dispute_allowed_for_completed_unsettled() {
    let ctx = setup();
    let amount = 100i128;
    let booking_id = ctx.reach_completed(amount);
    assert!(!ctx.client().get_booking(&booking_id).settled);
    ctx.client().open_dispute(&booking_id);
    assert_eq!(
        ctx.client().get_booking(&booking_id).state,
        BookingState::Disputed
    );
}

// --- Address collision hardening ---

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn constructor_rejects_ops_pool_equals_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = Address::generate(&env);
    let stello = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    env.register_at(
        &contract_id,
        StelloBookingContract,
        (
            stello,
            token,
            contract_id.clone(), // ops_pool == contract
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
            DEFAULT_HOST_CANCEL_FEE,
        ),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn constructor_rejects_stello_equals_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    env.register_at(
        &contract_id,
        StelloBookingContract,
        (
            contract_id.clone(),
            token,
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
            DEFAULT_HOST_CANCEL_FEE,
        ),
    );
}

#[test]
fn book_rejects_traveller_or_host_equals_contract() {
    let ctx = setup();
    assert_eq!(
        ctx.client()
            .try_book(&ctx.contract_id, &ctx.host, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.contract_id, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn book_rejects_host_equals_ops_pool() {
    let ctx = setup();
    // Host == ops would make host-cancel fee an economic no-op.
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.ops, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn book_rejects_party_equals_payout_sink() {
    let ctx = setup();
    assert_eq!(
        ctx.client()
            .try_book(&ctx.review, &ctx.host, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.qa, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.o2o, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.stello, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn book_rejects_traveller_or_host_equals_token() {
    let ctx = setup();
    assert_eq!(
        ctx.client()
            .try_book(&ctx.token, &ctx.host, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
    assert_eq!(
        ctx.client()
            .try_book(&ctx.traveller, &ctx.token, &100i128, &1_000_000),
        Err(Ok(Error::InvalidAddress))
    );
}

#[test]
fn update_booking_rejects_host_equals_ops() {
    let ctx = setup();
    let booking_id = ctx.fund_and_book(100, 1_000_000);
    assert_eq!(
        ctx.client()
            .try_update_booking(&booking_id, &ctx.ops, &100i128, &2_000_000),
        Err(Ok(Error::InvalidAddress))
    );
}

// --- Auth without exclusive reliance on silent success ---

#[test]
fn stello_auth_required_for_book_recorded() {
    let ctx = setup();
    let _ = ctx
        .client()
        .book(&ctx.traveller, &ctx.host, &100i128, &1_000_000u64);
    let auths = ctx.env.auths();
    assert!(
        auths.iter().any(|(addr, _)| *addr == ctx.stello),
        "expected stello auth on book; got {:?}",
        auths.len()
    );
}

#[test]
fn stello_cannot_impersonate_host_for_cancel() {
    let env = Env::default();
    // Authorize only Stello — not the host — for cancel_by_host.
    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    env.mock_all_auths();
    let issuer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(issuer);
    let token = sac.address();
    let sac_admin = token::StellarAssetClient::new(&env, &token);
    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);
    let amount = 100i128;
    sac_admin.mint(&traveller, &amount);
    sac_admin.mint(&host, &DEFAULT_HOST_CANCEL_FEE);
    let booking_id = client.book(&traveller, &host, &amount, &1_000_000u64);
    client.lock_escrow(&booking_id, &amount);

    // Only mock Stello for cancel_by_host — host.require_auth must still fail.
    env.set_auths(&[]);
    env.mock_auths(&[MockAuth {
        address: &stello,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "cancel_by_host",
            args: (booking_id,).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(
        client.try_cancel_by_host(&booking_id).is_err(),
        "Stello must not impersonate Host"
    );
    let b = client.get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Escrowed);
    assert!(!b.settled);
}

#[test]
fn host_cancel_late_escrow_refund_failure_rolls_back_fee() {
    // Fee transfer (host→ops) would succeed; traveller escrow refund fails afterward.
    let env = Env::default();
    env.mock_all_auths();

    let stello = Address::generate(&env);
    let traveller = Address::generate(&env);
    let host = Address::generate(&env);
    let ops = Address::generate(&env);
    let review = Address::generate(&env);
    let qa = Address::generate(&env);
    let o2o = Address::generate(&env);

    let token = env.register(ScriptedToken, ());
    let tok = ScriptedTokenClient::new(&env, &token);
    let contract_id = register_contract(
        &env,
        &stello,
        &token,
        &ops,
        &review,
        &qa,
        &o2o,
        DEFAULT_HOST_CANCEL_FEE,
    );
    let client = StelloBookingContractClient::new(&env, &contract_id);

    let amount = 100i128;
    tok.mint(&traveller, &amount);
    tok.mint(&host, &DEFAULT_HOST_CANCEL_FEE);
    let booking_id = client.book(&traveller, &host, &amount, &1_000_000u64);
    client.lock_escrow(&booking_id, &amount);

    tok.arm_fail_to(&traveller);
    assert!(client.try_cancel_by_host(&booking_id).is_err());

    let b = client.get_booking(&booking_id);
    assert_eq!(b.state, BookingState::Escrowed);
    assert!(!b.settled);
    assert_eq!(b.escrow_amount, amount);
    assert_eq!(client.get_total_escrowed(), amount);
    // Host fee not lost; escrow intact.
    assert_eq!(tok.balance(&host), DEFAULT_HOST_CANCEL_FEE);
    assert_eq!(tok.balance(&ops), 0);
    assert_eq!(tok.balance(&contract_id), amount);
    assert_eq!(tok.balance(&traveller), 0);
}

#[test]
fn instance_ttl_bumped_on_booking_activity() {
    let ctx = setup();
    let booking_id = ctx.fund_and_book(100, 1_000_000);

    // Drop remaining TTL below INSTANCE_TTL_THRESHOLD so the next active read extends.
    let seq = ctx.env.ledger().sequence();
    ctx.env
        .ledger()
        .set_sequence_number(seq + INSTANCE_TTL_EXTEND_TO - 10_000);

    let _ = ctx.client().get_booking(&booking_id);
    ctx.env.as_contract(&ctx.contract_id, || {
        let instance_ttl = ctx.env.storage().instance().get_ttl();
        assert_eq!(instance_ttl, INSTANCE_TTL_EXTEND_TO);
        let booking_ttl = ctx
            .env
            .storage()
            .persistent()
            .get_ttl(&DataKey::Booking(booking_id));
        assert_eq!(booking_ttl, PERSISTENT_BOOKING_TTL_EXTEND_TO);
    });
    // Limitation: testutils auto-restore archived entries (Protocol 23+); we assert
    // extend_ttl targets via get_ttl, not irreversible network archival.
}
