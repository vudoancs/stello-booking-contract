//! Core types for Stello booking / escrow / settlement contract.
use soroban_sdk::{Address, BytesN, contracttype};

/// Booking lifecycle states.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum BookingState {
    Created = 0,
    Escrowed = 1,
    CheckedIn = 2,
    Completed = 3,
    Cancelled = 4,
    Disputed = 5,
}

/// Who initiated cancellation (drives refund policy).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CancelledBy {
    None = 0,
    Traveller = 1,
    Host = 2,
}

/// Protocol configuration (instance storage).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// Privileged Stello wallet — book/update/check-in/complete/cancel/dispute.
    /// Does **not** authorize `lock_escrow` (Traveller funds escrow).
    pub stello_wallet: Address,
    /// Stellar Asset Contract for USDC (or test token).
    pub token: Address,
    pub ops_pool: Address,
    pub review_pool: Address,
    pub qa_pool: Address,
    pub o2o_pool: Address,
    /// Host cancellation fee in token base units ($5 USDC @ 7 decimals = 50_000_000).
    pub host_cancel_fee: i128,
}

/// On-chain booking record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Booking {
    /// Internal sequential ID for this contract deployment.
    pub booking_id: u64,
    /// Immutable opaque reference to the Stello backend Booking (globally unique here).
    pub booking_ref: BytesN<32>,
    /// Immutable opaque reference to the Stello backend Service (not unique).
    pub service_ref: BytesN<32>,
    pub traveller: Address,
    pub host: Address,
    pub amount: i128,
    pub token: Address,
    /// Unix seconds — experience start (cancellation windows).
    pub start_time: u64,
    pub created_at: u64,
    pub state: BookingState,
    pub escrow_locked: bool,
    /// USDC held in contract escrow for this booking (0 until lock; then == amount).
    pub escrow_amount: i128,
    pub checked_in: bool,
    pub settled: bool,
    pub was_cancelled: bool,
    pub cancelled_by: CancelledBy,
}

/// Traveller-cancel settlement breakdown.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelSettlement {
    pub traveller_amount: i128,
    pub host_amount: i128,
    pub ops_amount: i128,
}

/// Completion split breakdown (basis-point math).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitAmounts {
    pub host_amount: i128,
    pub ops_amount: i128,
    pub review_amount: i128,
    pub qa_amount: i128,
    pub o2o_amount: i128,
}

/// Full multi-party settlement amounts (completion or dispute).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementAmounts {
    pub traveller_amount: i128,
    pub host_amount: i128,
    pub ops_amount: i128,
    pub review_amount: i128,
    pub qa_amount: i128,
    pub o2o_amount: i128,
}

/// Distinguishes settlement outcomes in events.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SettlementType {
    Completed = 0,
    TravellerCancel = 1,
    /// Escrow-only settlement. The Host→Ops `$5` fee is **not** included here;
    /// see `HostCancellationFeePaid`.
    HostCancel = 2,
    Dispute = 3,
}

/// Revenue split basis points (must sum to BPS_DENOM).
pub const HOST_BPS: u32 = 8000;
pub const OPS_BPS: u32 = 1000;
pub const REVIEW_BPS: u32 = 500;
pub const QA_BPS: u32 = 300;
pub const O2O_BPS: u32 = 200;
pub const BPS_DENOM: u32 = 10_000;

/// Cancellation window thresholds (seconds).
pub const SECONDS_PER_WEEK: u64 = 7 * 24 * 60 * 60;
pub const FOUR_WEEKS: u64 = 4 * SECONDS_PER_WEEK;
pub const TWO_WEEKS: u64 = 2 * SECONDS_PER_WEEK;

/// Default host cancel fee: $5 USDC with 7 decimals.
pub const DEFAULT_HOST_CANCEL_FEE: i128 = 50_000_000;
