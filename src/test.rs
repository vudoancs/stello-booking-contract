#![cfg(test)]
//! Integration tests with mock USDC (Stellar Asset Contract).
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, Vec,
};

use crate::{
    BookingState, CancelledBy, Config, DisputeShare, Error, StelloBookingContract,
    StelloBookingContractClient, DEFAULT_HOST_CANCEL_FEE, FOUR_WEEKS, TWO_WEEKS,
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
            .create_booking(&self.traveller, &self.host, &amount, &start_time)
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
    assert_eq!(got.host_cancel_fee, DEFAULT_HOST_CANCEL_FEE);

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
fn create_update_lock_complete_split() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);

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

    let split = ctx.client().complete_booking(&booking_id);
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
    assert_eq!(
        ctx.client().get_booking_state(&booking_id),
        BookingState::Completed
    );
}

#[test]
fn traveller_cancel_four_weeks() {
    let ctx = setup();
    let amount = 100i128;
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let booking_id = ctx.fund_and_book(amount, FOUR_WEEKS);
    ctx.client().lock_escrow(&booking_id);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 70);
    assert_eq!(s.host_amount, 15);
    assert_eq!(s.ops_amount, 15);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), 70);
    assert_eq!(ctx.token_client().balance(&ctx.host), 15);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 15);
}

#[test]
fn traveller_cancel_two_to_four_weeks() {
    let ctx = setup();
    let amount = 100i128;
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let booking_id = ctx.fund_and_book(amount, TWO_WEEKS);
    ctx.client().lock_escrow(&booking_id);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 50);
    assert_eq!(s.host_amount, 35);
    assert_eq!(s.ops_amount, 15);
}

#[test]
fn traveller_cancel_under_two_weeks() {
    let ctx = setup();
    let amount = 100i128;
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let booking_id = ctx.fund_and_book(amount, TWO_WEEKS - 1);
    ctx.client().lock_escrow(&booking_id);

    let s = ctx.client().cancel_by_traveller(&booking_id);
    assert_eq!(s.traveller_amount, 0);
    assert_eq!(s.host_amount, 80);
    assert_eq!(s.ops_amount, 20);
}

#[test]
fn host_cancel_refunds_escrow_and_charges_fee_from_host() {
    let ctx = setup();
    let amount = 100_000_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id);

    ctx.mint(&ctx.host, DEFAULT_HOST_CANCEL_FEE);
    assert_eq!(
        ctx.token_client().balance(&ctx.host),
        DEFAULT_HOST_CANCEL_FEE
    );

    let s = ctx.client().cancel_by_host(&booking_id);
    assert_eq!(s.traveller_amount, amount);
    assert_eq!(s.host_amount, 0);
    assert_eq!(s.ops_amount, 0);

    assert_eq!(ctx.token_client().balance(&ctx.traveller), amount);
    assert_eq!(ctx.token_client().balance(&ctx.contract_id), 0);
    assert_eq!(ctx.token_client().balance(&ctx.host), 0);
    assert_eq!(
        ctx.token_client().balance(&ctx.ops),
        DEFAULT_HOST_CANCEL_FEE
    );
    assert_eq!(
        ctx.client().get_booking(&booking_id).cancelled_by,
        CancelledBy::Host
    );
}

#[test]
fn dispute_custom_bps_allocation() {
    let ctx = setup();
    let amount = 10_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id);
    ctx.client().open_dispute(&booking_id);
    assert_eq!(
        ctx.client().get_booking_state(&booking_id),
        BookingState::Disputed
    );

    let mut shares = Vec::new(&ctx.env);
    shares.push_back(DisputeShare {
        recipient: ctx.traveller.clone(),
        bps: 4000,
    });
    shares.push_back(DisputeShare {
        recipient: ctx.host.clone(),
        bps: 4000,
    });
    shares.push_back(DisputeShare {
        recipient: ctx.ops.clone(),
        bps: 2000,
    });

    ctx.client().resolve_dispute(&booking_id, &shares);
    assert_eq!(ctx.token_client().balance(&ctx.traveller), 4000);
    assert_eq!(ctx.token_client().balance(&ctx.host), 4000);
    assert_eq!(ctx.token_client().balance(&ctx.ops), 2000);
    assert_eq!(
        ctx.client().get_booking_state(&booking_id),
        BookingState::Completed
    );
}

#[test]
fn dispute_rejects_bps_not_10000() {
    let ctx = setup();
    let amount = 10_000i128;
    let booking_id = ctx.fund_and_book(amount, 1_000_000);
    ctx.client().lock_escrow(&booking_id);
    ctx.client().open_dispute(&booking_id);

    let mut shares = Vec::new(&ctx.env);
    shares.push_back(DisputeShare {
        recipient: ctx.traveller.clone(),
        bps: 5000,
    });
    shares.push_back(DisputeShare {
        recipient: ctx.host.clone(),
        bps: 4000,
    });

    assert_eq!(
        ctx.client().try_resolve_dispute(&booking_id, &shares),
        Err(Ok(Error::InvalidBpsAllocation))
    );
}

#[test]
fn cannot_complete_without_escrow() {
    let ctx = setup();
    let booking_id = ctx
        .client()
        .create_booking(&ctx.traveller, &ctx.host, &100i128, &1_000_000);
    assert_eq!(
        ctx.client().try_complete_booking(&booking_id),
        Err(Ok(Error::InvalidStateTransition))
    );
}

#[test]
fn quote_refund_matches_windows() {
    let ctx = setup();
    ctx.env.ledger().with_mut(|l| l.timestamp = 0);
    let booking_id = ctx.fund_and_book(100, FOUR_WEEKS);
    let s = ctx
        .client()
        .quote_refund(&booking_id, &CancelledBy::Traveller);
    assert_eq!(s.traveller_amount, 70);
}
