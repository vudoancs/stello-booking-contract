//! Contract events for off-chain indexers.
use soroban_sdk::{contractevent, Address};

use crate::types::{CancelSettlement, SplitAmounts};

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

#[contractevent(topics = ["booking", "completed"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookingCompleted {
    #[topic]
    pub booking_id: u64,
    pub host_amount: i128,
    pub ops_amount: i128,
    pub review_amount: i128,
    pub qa_amount: i128,
    pub o2o_amount: i128,
}

impl BookingCompleted {
    pub fn from_split(booking_id: u64, split: &SplitAmounts) -> Self {
        Self {
            booking_id,
            host_amount: split.host_amount,
            ops_amount: split.ops_amount,
            review_amount: split.review_amount,
            qa_amount: split.qa_amount,
            o2o_amount: split.o2o_amount,
        }
    }
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
    pub host_fee: i128,
}

impl BookingCancelled {
    pub fn from_settlement(
        booking_id: u64,
        by_host: bool,
        settlement: &CancelSettlement,
        host_fee: i128,
    ) -> Self {
        Self {
            booking_id,
            by_host,
            traveller_amount: settlement.traveller_amount,
            host_amount: settlement.host_amount,
            ops_amount: settlement.ops_amount,
            host_fee,
        }
    }
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
    pub amount: i128,
}
