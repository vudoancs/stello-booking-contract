#![cfg(test)]
//! Step 2 lifecycle tests: book → lock_escrow → check_in → complete.
use soroban_sdk::{Address, Env, testutils::Address as _, token};

use crate::{
    BookingState, Config, DEFAULT_HOST_CANCEL_FEE, Error, StelloBookingContract,
    StelloBookingContractClient,
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

    ctx.client().lock_escrow(&booking_id);
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
fn reject_lock_escrow_from_non_created() {
    let ctx = setup();
    let booking_id = ctx.fund_and_book(100, 1_000_000);
    ctx.client().lock_escrow(&booking_id);
    assert_eq!(
        ctx.client().try_lock_escrow(&booking_id),
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
    let booking_id = ctx.fund_and_book(100, 1_000_000);
    ctx.client().lock_escrow(&booking_id);
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
    let booking_id = ctx.fund_and_book(100, 1_000_000);
    ctx.client().lock_escrow(&booking_id);
    assert_eq!(
        ctx.client().try_complete(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn reject_complete_twice() {
    let ctx = setup();
    let booking_id = ctx.fund_and_book(100, 1_000_000);
    ctx.client().lock_escrow(&booking_id);
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
    ctx.client().lock_escrow(&booking_id);
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
