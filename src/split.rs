//! Completion / dispute revenue split (basis points, integer math).
//!
//! # Rounding policy (MVP)
//! - **Completion (`compute_completion_split`):** each share is
//!   `floor(amount * bps / 10_000)`; any remainder after flooring is assigned
//!   to the **Host** so the five shares sum exactly to `amount`.
//! - **Dispute / custom BPS (`compute_bps_amounts`):** same floor-per-share
//!   rule; remainder is assigned to **`out[0]`** (Traveller in
//!   `resolve_dispute`). Keep this for MVP unless a stronger invariant requires
//!   a different remainder recipient.
//!
//! # Overflow-safe floor
//! `bps_floor` uses quotient/remainder decomposition so `amount * bps` is never
//! formed directly, supporting the full non-negative `i128` range while preserving
//! exact `floor(amount * bps / 10_000)` semantics.
use crate::errors::Error;
use crate::types::{BPS_DENOM, HOST_BPS, O2O_BPS, OPS_BPS, QA_BPS, REVIEW_BPS, SplitAmounts};

/// `floor(amount * bps / BPS_DENOM)` without multiplying `amount * bps` directly.
///
/// Decompose `amount = q * DENOM + r`, then:
/// `floor(amount * bps / DENOM) = q * bps + floor(r * bps / DENOM)`.
pub(crate) fn bps_floor(amount: i128, bps: u32) -> Result<i128, Error> {
    if amount < 0 {
        return Err(Error::InvalidAmount);
    }
    let bps_i = i128::from(bps);
    let denom = i128::from(BPS_DENOM);
    let q = amount / denom;
    let r = amount % denom;
    let high = q.checked_mul(bps_i).ok_or(Error::MathError)?;
    // r < DENOM and bps <= DENOM in our call sites → r * bps fits comfortably.
    let low = r
        .checked_mul(bps_i)
        .ok_or(Error::MathError)?
        .checked_div(denom)
        .ok_or(Error::MathError)?;
    high.checked_add(low).ok_or(Error::MathError)
}

/// Split `amount` into completion shares.
pub fn compute_completion_split(amount: i128) -> Result<SplitAmounts, Error> {
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }

    let mut host_amount = bps_floor(amount, HOST_BPS)?;
    let ops_amount = bps_floor(amount, OPS_BPS)?;
    let review_amount = bps_floor(amount, REVIEW_BPS)?;
    let qa_amount = bps_floor(amount, QA_BPS)?;
    let o2o_amount = bps_floor(amount, O2O_BPS)?;

    let sum = host_amount
        .checked_add(ops_amount)
        .and_then(|v| v.checked_add(review_amount))
        .and_then(|v| v.checked_add(qa_amount))
        .and_then(|v| v.checked_add(o2o_amount))
        .ok_or(Error::MathError)?;

    let remainder = amount.checked_sub(sum).ok_or(Error::MathError)?;
    host_amount = host_amount.checked_add(remainder).ok_or(Error::MathError)?;

    let total = host_amount
        .checked_add(ops_amount)
        .and_then(|v| v.checked_add(review_amount))
        .and_then(|v| v.checked_add(qa_amount))
        .and_then(|v| v.checked_add(o2o_amount))
        .ok_or(Error::MathError)?;

    if total != amount {
        return Err(Error::MathError);
    }

    Ok(SplitAmounts {
        host_amount,
        ops_amount,
        review_amount,
        qa_amount,
        o2o_amount,
    })
}

/// Fill `out` with floor(amount * bps / 10_000) for each entry in `bps_list`.
/// Remainder after flooring is added to `out[0]`. Returns number of shares written.
///
/// Requires `out.len() >= bps_list.len()` and `bps_list` summing to [`BPS_DENOM`].
pub fn compute_bps_amounts(
    amount: i128,
    bps_list: &[u32],
    out: &mut [i128],
) -> Result<usize, Error> {
    if amount <= 0 {
        return Err(Error::InvalidAmount);
    }
    if bps_list.is_empty() || out.len() < bps_list.len() {
        return Err(Error::InvalidBpsAllocation);
    }

    let mut total_bps: u32 = 0;
    for &bps in bps_list {
        total_bps = total_bps
            .checked_add(bps)
            .ok_or(Error::InvalidBpsAllocation)?;
    }
    if total_bps != BPS_DENOM {
        return Err(Error::InvalidBpsAllocation);
    }

    let n = bps_list.len();
    let mut sum: i128 = 0;
    for (slot, &bps) in out.iter_mut().zip(bps_list.iter()).take(n) {
        let share = bps_floor(amount, bps)?;
        *slot = share;
        sum = sum.checked_add(share).ok_or(Error::MathError)?;
    }

    let remainder = amount.checked_sub(sum).ok_or(Error::MathError)?;
    out[0] = out[0].checked_add(remainder).ok_or(Error::MathError)?;

    let mut check: i128 = 0;
    for &v in out.iter().take(n) {
        check = check.checked_add(v).ok_or(Error::MathError)?;
    }
    if check != amount {
        return Err(Error::MathError);
    }

    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sums_to(amount: i128) {
        let s = compute_completion_split(amount).unwrap();
        assert_eq!(
            s.host_amount + s.ops_amount + s.review_amount + s.qa_amount + s.o2o_amount,
            amount
        );
    }

    #[test]
    fn split_100_units() {
        let s = compute_completion_split(100).unwrap();
        assert_eq!(s.host_amount, 80);
        assert_eq!(s.ops_amount, 10);
        assert_eq!(s.review_amount, 5);
        assert_eq!(s.qa_amount, 3);
        assert_eq!(s.o2o_amount, 2);
    }

    #[test]
    fn split_50_remainder_to_host() {
        let s = compute_completion_split(50).unwrap();
        assert_eq!(s.host_amount, 41);
        assert_eq!(s.ops_amount, 5);
        assert_eq!(s.review_amount, 2);
        assert_eq!(s.qa_amount, 1);
        assert_eq!(s.o2o_amount, 1);
        assert_sums_to(50);
    }

    #[test]
    fn split_dust() {
        assert_sums_to(1);
        assert_sums_to(3);
        assert_sums_to(7);
        assert_sums_to(99);
    }

    #[test]
    fn rejects_non_positive() {
        assert_eq!(compute_completion_split(0), Err(Error::InvalidAmount));
        assert_eq!(compute_completion_split(-1), Err(Error::InvalidAmount));
    }

    #[test]
    fn dispute_bps_must_sum_10000() {
        let mut out = [0i128; 4];
        let n = compute_bps_amounts(100, &[5000, 5000], &mut out).unwrap();
        assert_eq!(n, 2);
        assert_eq!(&out[..2], &[50, 50]);

        let n = compute_bps_amounts(100, &[8000, 1000, 1000], &mut out).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&out[..3], &[80, 10, 10]);

        assert_eq!(
            compute_bps_amounts(100, &[5000, 4000], &mut out),
            Err(Error::InvalidBpsAllocation)
        );
    }

    #[test]
    fn extreme_i128_amounts_sum_exactly() {
        // Near i128::MAX — old amount*bps multiply would overflow.
        let amounts = [
            i128::MAX,
            i128::MAX - 1,
            i128::MAX / 2,
            10_000_000_000_000_000_000i128, // 1e19
            9_007_199_254_740_991i128,      // 2^53-1
        ];
        for amount in amounts {
            assert_sums_to(amount);
            let s = compute_completion_split(amount).unwrap();
            assert!(s.host_amount >= 0);
            assert!(s.ops_amount >= 0);

            let mut out = [0i128; 6];
            compute_bps_amounts(amount, &[4000, 3000, 1000, 1000, 500, 500], &mut out).unwrap();
            assert_eq!(out.iter().sum::<i128>(), amount);
        }
    }

    #[test]
    fn bps_floor_matches_naive_for_safe_range() {
        // Where amount * 8000 fits in i128, compare to naive.
        let amount = 1_000_000_000_000i128;
        let naive = amount * 8000 / 10_000;
        assert_eq!(bps_floor(amount, 8000).unwrap(), naive);
    }
}
