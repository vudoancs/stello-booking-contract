#![cfg(test)]
//! Lifecycle + Step 3 USDC escrow tests.
use soroban_sdk::{
    Address, Env, IntoVal,
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    token,
};

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

    /// book → lock → check_in → complete (not settled).
    fn reach_completed(&self, amount: i128) -> u64 {
        let booking_id = self.fund_and_book(amount, 1_000_000);
        self.client().lock_escrow(&booking_id, &amount);
        self.client().check_in(&booking_id);
        self.client().complete(&booking_id);
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
