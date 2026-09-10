//! Contract events for off-chain indexers.
//!
//! # Settlement accounting for indexers
//! - [`SettlementExecuted`] is the **canonical** escrow settlement event. Sum
//!   its amount fields for on-escrow financial accounting.
//! - [`BookingCancelled`] is lifecycle / cancellation metadata. It may repeat
//!   escrow allocation figures for UX, but **MUST NOT** be independently summed
//!   as a second financial settlement (that would double-count vs
//!   [`SettlementExecuted`]).
//! - [`HostCancellationFeePaid`] is a separate Host → Operations payment
//!   **outside** booking escrow and is not escrow settlement.
use soroban_sdk::{Address, BytesN, contractevent};

use crate::types::SettlementType;

#[contractevent(topics = ["booking", "created"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingCreated {
    #[topic]
    pub booking_id: u64,
    /// Opaque Stello backend Booking id (data, not topic — size/indexing).
    pub booking_ref: BytesN<32>,
    /// Opaque Stello backend Service id (data, not topic).
    pub service_ref: BytesN<32>,
    pub traveller: Address,
    pub host: Address,
    pub amount: i128,
    pub token: Address,
    pub start_time: u64,
}

#[contractevent(topics = ["booking", "updated"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingUpdated {
    #[topic]
    pub booking_id: u64,
    pub amount: i128,
    pub start_time: u64,
    pub host: Address,
}

#[contractevent(topics = ["escrow", "locked"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowLocked {
    #[topic]
    pub booking_id: u64,
    pub amount: i128,
    pub token: Address,
}

#[contractevent(topics = ["booking", "checked_in"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingCheckedIn {
    #[topic]
    pub booking_id: u64,
    pub timestamp: u64,
}

#[contractevent(topics = ["booking", "completed"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingCompleted {
    #[topic]
    pub booking_id: u64,
}

/// Canonical **escrow** settlement event (completion, traveller/host cancel, dispute).
///
/// For `SettlementType::HostCancel`, amounts reflect booking escrow only
/// (typically full escrow → traveller; `ops_amount = 0`). Do **not** add the
/// Host→Ops `$5` fee into these fields — that fee is external to escrow and is
/// emitted separately as [`HostCancellationFeePaid`].
#[contractevent(topics = ["settlement", "executed"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementExecuted {
    #[topic]
    pub booking_id: u64,
    pub settlement_type: SettlementType,
    pub traveller_amount: i128,
    pub host_amount: i128,
    pub ops_amount: i128,
    pub review_amount: i128,
    pub qa_amount: i128,
    pub o2o_amount: i128,
    pub timestamp: u64,
}

/// Lifecycle / cancellation signal. Amount fields mirror escrow allocation for
/// convenience but **MUST NOT** be summed independently by indexers as a second
/// settlement — use [`SettlementExecuted`] for escrow accounting.
#[contractevent(topics = ["booking", "cancelled"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingCancelled {
    #[topic]
    pub booking_id: u64,
    pub by_host: bool,
    pub traveller_amount: i128,
    pub host_amount: i128,
    pub ops_amount: i128,
}

/// Host wallet → Operations fee on host cancel. **Not** part of booking escrow
/// and **not** escrow settlement. Indexers must not fold this into
/// [`SettlementExecuted`] totals.
#[contractevent(topics = ["host_cancel", "fee_paid"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostCancellationFeePaid {
    #[topic]
    pub booking_id: u64,
    pub host: Address,
    pub ops: Address,
    pub amount: i128,
}

#[contractevent(topics = ["dispute", "opened"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeOpened {
    #[topic]
    pub booking_id: u64,
}

#[contractevent(topics = ["dispute", "resolved"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisputeResolved {
    #[topic]
    pub booking_id: u64,
    pub traveller_bps: u32,
    pub host_bps: u32,
    pub ops_bps: u32,
    pub review_bps: u32,
    pub qa_bps: u32,
    pub o2o_bps: u32,
}
