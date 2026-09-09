//! Contract errors for StelloBookingContract.
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    InvalidStateTransition = 4,
    BookingNotFound = 5,
    InvalidAmount = 6,
    InvalidAddress = 7,
    InvalidToken = 8,
    EscrowAlreadyLocked = 9,
    EscrowNotLocked = 10,
    AlreadySettled = 11,
    AlreadyCancelled = 12,
    InvalidDispute = 13,
    InvalidBpsAllocation = 14,
    MathError = 15,
    InvalidUpdate = 16,
    EscrowExceedsAmount = 17,
    /// Contract token balance is below accounted total_escrowed.
    Insolvent = 18,
    /// start_time must be strictly after current ledger timestamp.
    InvalidStartTime = 19,
    /// Cancel settlement record missing.
    SettlementNotFound = 20,
}
