//! Contract events for off-chain indexers.
use soroban_sdk::{Address, contractevent};

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
