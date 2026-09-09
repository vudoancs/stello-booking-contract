//! Contract events for off-chain indexers.
use soroban_sdk::{Address, contractevent};

use crate::types::SplitAmounts;

#[contractevent(topics = ["booking", "created"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingCreated {
    #[topic]
    pub booking_id: u64,
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

#[contractevent(topics = ["settlement", "executed"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementExecuted {
    #[topic]
    pub booking_id: u64,
    pub traveller_amount: i128,
    pub host_amount: i128,
    pub ops_amount: i128,
    pub review_amount: i128,
    pub qa_amount: i128,
    pub o2o_amount: i128,
}

impl SettlementExecuted {
    pub fn from_split(booking_id: u64, split: &SplitAmounts) -> Self {
        Self {
            booking_id,
            traveller_amount: 0,
            host_amount: split.host_amount,
            ops_amount: split.ops_amount,
            review_amount: split.review_amount,
            qa_amount: split.qa_amount,
            o2o_amount: split.o2o_amount,
        }
    }
}

#[contractevent(topics = ["payout", "claimed"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutClaimed {
    #[topic]
    pub claimant: Address,
    pub amount: i128,
    pub token: Address,
}

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

#[contractevent(topics = ["cancel", "settled"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelSettlementExecuted {
    #[topic]
    pub booking_id: u64,
    pub traveller_amount: i128,
    pub host_amount: i128,
    pub ops_amount: i128,
}

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
