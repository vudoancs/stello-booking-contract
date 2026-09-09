//! Traveller / host cancellation refund policy.
//!
//! Boundaries use `start_time.saturating_sub(now)` as time-until-start.
//! Inclusive lower bounds for traveller cancel:
//! - `>= FOUR_WEEKS` → 70/15/15 (traveller/host/ops)
//! - `>= TWO_WEEKS` (and < 4 weeks) → 50/35/15
//! - `< TWO_WEEKS` → 0/80/20
//!
//! Host cancellation → 100% traveller refund of escrow (fee handled separately).
//!
//! Remainder after flooring goes to Operations when ops share > 0; otherwise host;
//! otherwise traveller.
use crate::errors::Error;
use crate::split::bps_floor;
use crate::types::{CancelSettlement, CancelledBy, FOUR_WEEKS, TWO_WEEKS};

/// Pure refund computation for escrow — does not include host $5 fee.
pub fn compute_refund(
    amount: i128,
    start_time: u64,
    now: u64,
    cancelled_by: CancelledBy,
) -> Result<CancelSettlement, Error> {
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    if cancelled_by == CancelledBy::Host {
        return Ok(CancelSettlement {
            traveller_amount: amount,
            host_amount: 0,
            ops_amount: 0,
        });
    }

    if cancelled_by != CancelledBy::Traveller {
        return Err(Error::InvalidAmount);
    }

    let until_start = start_time.saturating_sub(now);

    let (traveller_bps, host_bps, ops_bps) = if until_start >= FOUR_WEEKS {
        (7000u32, 1500u32, 1500u32)
    } else if until_start >= TWO_WEEKS {
        (5000, 3500, 1500)
    } else {
        (0, 8000, 2000)
    };

    let mut traveller_amount = bps_floor(amount, traveller_bps)?;
    let mut host_amount = bps_floor(amount, host_bps)?;
    let mut ops_amount = bps_floor(amount, ops_bps)?;

    let sum = traveller_amount
        .checked_add(host_amount)
        .and_then(|v| v.checked_add(ops_amount))
        .ok_or(Error::MathError)?;
    let remainder = amount.checked_sub(sum).ok_or(Error::MathError)?;

    if ops_bps > 0 {
        ops_amount = ops_amount.checked_add(remainder).ok_or(Error::MathError)?;
    } else if host_bps > 0 {
        host_amount = host_amount.checked_add(remainder).ok_or(Error::MathError)?;
    } else {
        traveller_amount = traveller_amount
            .checked_add(remainder)
            .ok_or(Error::MathError)?;
    }

    let total = traveller_amount
        .checked_add(host_amount)
        .and_then(|v| v.checked_add(ops_amount))
        .ok_or(Error::MathError)?;
    if total != amount {
        return Err(Error::MathError);
    }

    Ok(CancelSettlement {
        traveller_amount,
        host_amount,
        ops_amount,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cancel_full_refund() {
        let s = compute_refund(100, 1_000_000, 0, CancelledBy::Host).unwrap();
        assert_eq!(s.traveller_amount, 100);
        assert_eq!(s.host_amount, 0);
        assert_eq!(s.ops_amount, 0);
    }

    #[test]
    fn ge_four_weeks() {
        let s = compute_refund(100, FOUR_WEEKS + 100, 0, CancelledBy::Traveller).unwrap();
        assert_eq!(s.traveller_amount, 70);
        assert_eq!(s.host_amount, 15);
        assert_eq!(s.ops_amount, 15);
    }

    #[test]
    fn exactly_four_weeks() {
        let s = compute_refund(100, FOUR_WEEKS, 0, CancelledBy::Traveller).unwrap();
        assert_eq!(s.traveller_amount, 70);
    }

    #[test]
    fn just_below_four_weeks() {
        let s = compute_refund(100, FOUR_WEEKS - 1, 0, CancelledBy::Traveller).unwrap();
        assert_eq!(s.traveller_amount, 50);
        assert_eq!(s.host_amount, 35);
        assert_eq!(s.ops_amount, 15);
    }

    #[test]
    fn exactly_two_weeks() {
        let s = compute_refund(100, TWO_WEEKS, 0, CancelledBy::Traveller).unwrap();
        assert_eq!(s.traveller_amount, 50);
    }

    #[test]
    fn just_below_two_weeks() {
        let s = compute_refund(100, TWO_WEEKS - 1, 0, CancelledBy::Traveller).unwrap();
        assert_eq!(s.traveller_amount, 0);
        assert_eq!(s.host_amount, 80);
        assert_eq!(s.ops_amount, 20);
    }

    #[test]
    fn amounts_sum() {
        for amount in [1i128, 3, 7, 50, 99, 100, 10_000] {
            for &delta in &[0u64, TWO_WEEKS - 1, TWO_WEEKS, FOUR_WEEKS - 1, FOUR_WEEKS] {
                let s = compute_refund(amount, delta, 0, CancelledBy::Traveller).unwrap();
                assert_eq!(
                    s.traveller_amount + s.host_amount + s.ops_amount,
                    amount
                );
            }
        }
    }
}
