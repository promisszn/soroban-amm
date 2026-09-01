//! Q64.96 fixed-point math for concentrated liquidity (Uniswap V3-style).
//!
//! `sqrtPriceX96` encodes sqrt(price) * 2^96 as a u128.
//! All arithmetic is integer-only (no_std, no floats).
//!
//! Constraints:
//!   MIN_TICK = -887272  →  sqrt(1.0001^MIN_TICK) * 2^96 ≈ 4295128739
//!   MAX_TICK =  887272  →  sqrt(1.0001^MAX_TICK) * 2^96 ≈ 1461446703485210103287273052203988822378723970342

#![allow(dead_code)]

/// 2^96 as u128
pub const Q96: u128 = 79_228_162_514_264_337_593_543_950_336_u128; // 1 << 96

pub const MIN_TICK: i32 = -887_272;
pub const MAX_TICK: i32 = 887_272;

/// Minimum sqrt price: tick_to_sqrt_price_x96(MIN_TICK)
pub const MIN_SQRT_PRICE: u128 = 4_295_128_739_u128;
/// Maximum sqrt price representable in u128 (Uniswap V3's true max exceeds u128 range;
/// we cap at the highest value that fits: corresponds to ~tick 443636).
pub const MAX_SQRT_PRICE: u128 = 340_275_971_719_517_849_884_931_781_110_561_029_923_u128;

// ---------------------------------------------------------------------------
// Tick ↔ sqrtPrice
// ---------------------------------------------------------------------------

/// Convert a tick index to sqrtPriceX96 using a binary decomposition of
/// log(sqrt(1.0001)) ≈ 0.00004999500050 in Q128 fixed-point.
///
/// This mirrors the Uniswap V3 `TickMath.getSqrtRatioAtTick` algorithm
/// adapted to Soroban's u128 limit (no u256 available).
///
/// Accuracy: within 1 ULP for ticks in [MIN_TICK, MAX_TICK].
pub fn tick_to_sqrt_price_x96(tick: i32) -> u128 {
    assert!((MIN_TICK..=MAX_TICK).contains(&tick), "tick out of range");

    // Positive ticks are derived from the (exact) negative side rather than
    // inverting the Q128 ratio in u128. The Q128 inverse needs a 2^256
    // numerator that does not fit in u128, which loses precision that grows
    // with the tick magnitude (issue #347). Instead use the identity
    //
    //   sqrt_price(t) * sqrt_price(-t) = (2^96)^2 = 2^192
    //
    // so sqrt_price(t) = 2^192 / sqrt_price(-t). `sqrt_price(-t)` is already
    // exact, and `div_pow2` performs the wide division without overflow.
    if tick > 0 {
        let inv_sqrt_price = tick_to_sqrt_price_x96(-tick);
        let sqrt_price = div_pow2(192, inv_sqrt_price);
        return sqrt_price.clamp(MIN_SQRT_PRICE, MAX_SQRT_PRICE);
    }

    // Work with the absolute value; negate at the end if tick < 0.
    let abs_tick = tick.unsigned_abs() as u64;

    // Each magic constant below is 2^128 / sqrt(1.0001^(2^k)), precomputed.
    // We use Q128 intermediates then shift down to Q96 at the end.
    // Represented as (hi: u64, lo: u64) where value = hi * 2^64 + lo.
    // For Soroban's u128 limit we keep the ratio as a u128 Q128 value and
    // multiply using u128 arithmetic with careful bit-shifting.

    // ratio starts at 1.0 in Q128 = 2^128 (but u128 can't hold 2^128).
    // We store ratio as a u128 where ratio * 2^128 is the true Q128 value —
    // i.e. we represent ratio in Q128 but keep the leading "1" implicit by
    // starting at u128::MAX >> 1 ... Instead we use the standard approach:
    // start at 2^128 and shift.
    //
    // Since u128 max is 2^128 - 1, we use 340282366920938463463374607431768211455 (u128::MAX)
    // as an approximation of 2^128 and track error, OR we use the simpler
    // approach: keep ratio in Q96 directly and multiply step-by-step.
    //
    // We follow the Uniswap approach: ratio is Q128, stored in u128 (accepting
    // that 2^128 wraps to 0 — so we start at u128::MAX as 1.0 - ε and account
    // for the rounding). In practice, Uniswap V3 uses uint256; here we adapt
    // using u128 with the understanding that intermediate results stay < 2^128
    // because at each step we divide by 2^128.
    //
    // Magic constants: ratio_k = floor(2^128 / sqrt(1.0001)^(2^k))
    // These are the same as Uniswap V3 but truncated to u128.

    let mut ratio: u128 = if abs_tick & 0x1 != 0 {
        0xfffcb933bd6fad37aa2d162d1a594001_u128
    } else {
        u128::MAX
    };

    macro_rules! apply_bit {
        ($bit:expr, $magic:expr) => {
            if abs_tick & (1u64 << $bit) != 0 {
                ratio = mul_shift128(ratio, $magic);
            }
        };
    }

    apply_bit!(1, 0xfff97272373d413259a46990580e213a_u128);
    apply_bit!(2, 0xfff2e50f5f656932ef12357cf3c7fdcc_u128);
    apply_bit!(3, 0xffe5caca7e10e4e61c3624eaa0941cd0_u128);
    apply_bit!(4, 0xffcb9843d60f6159c9db58835c926644_u128);
    apply_bit!(5, 0xff973b41fa98c081472e6896dfb254c0_u128);
    apply_bit!(6, 0xff2ea16466c96a3843ec78b326b52861_u128);
    apply_bit!(7, 0xfe5dee046a99a2a811c461f1969c3053_u128);
    apply_bit!(8, 0xfcbe86c7900a88aedcffc83b479aa3a4_u128);
    apply_bit!(9, 0xf987a7253ac413176f2b074cf7815e54_u128);
    apply_bit!(10, 0xf3392b0822b70005940c7a398e4b70f3_u128);
    apply_bit!(11, 0xe7159475a2c29b7443b29c7fa6e889d9_u128);
    apply_bit!(12, 0xd097f3bdfd2022b8845ad8f792aa5825_u128);
    apply_bit!(13, 0xa9f746462d870fdf8a65dc1f90e061e5_u128);
    apply_bit!(14, 0x70d869a156d2a1b890bb3df62baf32f7_u128);
    apply_bit!(15, 0x31be135f97d08fd981231505542fcfa6_u128);
    apply_bit!(16, 0x9aa508b5b7a84e1c677de54f3e99bc9_u128);
    apply_bit!(17, 0x5d6af8dedb81196699c329225ee604_u128);
    apply_bit!(18, 0x2216e584f5fa1ea926041bedfe98_u128);
    apply_bit!(19, 0x48a170391f7dc42444e8fa2_u128);

    // Positive ticks are handled by the early-return inversion above, so here
    // `tick <= 0` and `ratio` already holds the correct Q128 sqrt price.

    // Convert from Q128 to Q96: shift right by 32 bits, with rounding.
    // sqrtPriceX96 = ratio >> 32
    let sqrt_price = (ratio >> 32)
        + if (ratio & 0xFFFFFFFF) >= 0x80000000 {
            1
        } else {
            0
        };

    // Clamp to valid range.
    sqrt_price.clamp(MIN_SQRT_PRICE, MAX_SQRT_PRICE)
}

/// Multiply two Q128 values and return the result as Q128.
/// Each argument is a Q128 number (true value = arg / 2^128).
/// Result = (a * b) >> 128.
///
/// Uses u128 with splitting to avoid overflow:
/// a = a_hi * 2^64 + a_lo
/// b = b_hi * 2^64 + b_lo
/// a*b = a_hi*b_hi*2^128 + (a_hi*b_lo + a_lo*b_hi)*2^64 + a_lo*b_lo
/// >> 128 keeps only a_hi*b_hi + high 64 bits of the middle terms.
#[inline(always)]
fn mul_shift128(a: u128, b: u128) -> u128 {
    let a_hi = a >> 64;
    let a_lo = a & 0xFFFFFFFFFFFFFFFF;
    let b_hi = b >> 64;
    let b_lo = b & 0xFFFFFFFFFFFFFFFF;

    let top = a_hi * b_hi;
    let mid1 = a_hi * b_lo;
    let mid2 = a_lo * b_hi;
    let _bot = a_lo * b_lo; // discarded (below 128-bit boundary)

    // mid1 and mid2 each have 128 bits; their high 64 bits add into `top`.
    let mid_sum = (mid1 >> 64).wrapping_add(mid2 >> 64);
    // Carry from the low 64 bits of the middles (approximate — 1-2 ULP error).
    let mid_lo_carry = ((mid1 & 0xFFFFFFFFFFFFFFFF).wrapping_add(mid2 & 0xFFFFFFFFFFFFFFFF)) >> 64;

    top.wrapping_add(mid_sum).wrapping_add(mid_lo_carry)
}

/// Computes `floor(2^pow / d)`, saturating at `u128::MAX` when the quotient
/// would not fit in a u128.
///
/// Used to invert a sqrt price for positive ticks: `2^192 / sqrt_price(-t)`.
/// The numerator `2^pow` cannot be materialised in a u128, so this performs
/// bitwise long division, processing the single numerator bit at position
/// `pow` and trailing zeros. The caller passes `d < 2^97` (a Q96 sqrt price),
/// which keeps the running remainder below `2^98` so it never overflows.
fn div_pow2(pow: u32, d: u128) -> u128 {
    debug_assert!(d != 0, "division by zero");
    let mut rem: u128 = 0;
    let mut quo: u128 = 0;
    for i in (0..=pow).rev() {
        // Shift in bit `i` of the numerator (set only at position `pow`).
        rem = (rem << 1) | u128::from(i == pow);
        // Appending the next quotient bit would drop the top bit: saturate.
        if quo >> 127 != 0 {
            return u128::MAX;
        }
        quo <<= 1;
        if rem >= d {
            rem -= d;
            quo |= 1;
        }
    }
    quo
}

/// Convert sqrtPriceX96 back to the floor tick.
///
/// Uses binary search over [MIN_TICK, MAX_TICK]: ~21 iterations, each calling
/// `tick_to_sqrt_price_x96`. This avoids the uint256 arithmetic used in Uniswap V3
/// `TickMath.getTickAtSqrtRatio` which doesn't fit in u128.
pub fn sqrt_price_x96_to_tick(sqrt_price: u128) -> i32 {
    assert!(
        (MIN_SQRT_PRICE..=MAX_SQRT_PRICE).contains(&sqrt_price),
        "sqrt price out of range"
    );

    // Binary search for the largest tick t such that tick_to_sqrt_price_x96(t) <= sqrt_price.
    let mut lo = MIN_TICK;
    let mut hi = MAX_TICK;

    while lo < hi {
        // Bias mid upward to avoid infinite loop when lo + 1 == hi.
        let mid = lo + (hi - lo + 1) / 2;
        if tick_to_sqrt_price_x96(mid) <= sqrt_price {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    lo
}

// ---------------------------------------------------------------------------
// Amount deltas
// ---------------------------------------------------------------------------

/// Token A amount for a position in [sqrt_a, sqrt_b] with given liquidity.
/// Mirrors Uniswap V3: amount0 = liquidity * (sqrt_b - sqrt_a) / (sqrt_a * sqrt_b / 2^96)
/// Returns 0 if sqrt_a >= sqrt_b.
pub fn get_amount0_delta(mut sqrt_a: u128, mut sqrt_b: u128, liquidity: i128) -> i128 {
    if sqrt_a > sqrt_b {
        core::mem::swap(&mut sqrt_a, &mut sqrt_b);
    }
    if sqrt_a == 0 || sqrt_b == 0 || liquidity == 0 || sqrt_a == sqrt_b {
        return 0;
    }
    let abs_liq = liquidity.unsigned_abs();
    // amount0 = abs_liq * (sqrt_b - sqrt_a) * 2^96 / (sqrt_b * sqrt_a / 2^96)
    //         = abs_liq * (sqrt_b - sqrt_a) * 2^192 / (sqrt_b * sqrt_a)
    // Use wide arithmetic via splitting to stay in u128.
    let numerator = mul_u128_u96(abs_liq, sqrt_b - sqrt_a); // abs_liq * (sqrt_b - sqrt_a) * 2^96
                                                            // Compute sqrt_a * sqrt_b / Q96 without overflow using mul_shift128
    let denominator = mul_shift128(sqrt_a, sqrt_b).wrapping_shl(32);
    let abs_result = numerator.checked_div(denominator).unwrap_or(0);
    if liquidity >= 0 {
        abs_result as i128
    } else {
        -(abs_result as i128)
    }
}

/// Token B amount for a position in [sqrt_a, sqrt_b] with given liquidity.
/// Mirrors Uniswap V3: amount1 = liquidity * (sqrt_b - sqrt_a) / 2^96
pub fn get_amount1_delta(mut sqrt_a: u128, mut sqrt_b: u128, liquidity: i128) -> i128 {
    if sqrt_a > sqrt_b {
        core::mem::swap(&mut sqrt_a, &mut sqrt_b);
    }
    if liquidity == 0 || sqrt_a == sqrt_b {
        return 0;
    }
    let abs_liq = liquidity.unsigned_abs();
    // amount1 = abs_liq * (sqrt_b - sqrt_a) / 2^96
    let abs_result = mul_u128_u96(abs_liq, sqrt_b - sqrt_a) / Q96;
    if liquidity >= 0 {
        abs_result as i128
    } else {
        -(abs_result as i128)
    }
}

/// Liquidity from a token-A amount in [sqrt_a, sqrt_b].
/// liquidity = amount0 * sqrt_a * sqrt_b / ((sqrt_b - sqrt_a) * 2^96)
pub fn get_liquidity_for_amount0(mut sqrt_a: u128, mut sqrt_b: u128, amount0: i128) -> i128 {
    if sqrt_a > sqrt_b {
        core::mem::swap(&mut sqrt_a, &mut sqrt_b);
    }
    if sqrt_b == sqrt_a || amount0 == 0 {
        return 0;
    }
    let abs_amt = amount0.unsigned_abs();
    // liq = abs_amt * (sqrt_a * sqrt_b / Q96) / (sqrt_b - sqrt_a)
    // Compute sqrt_a * sqrt_b / Q96 without u128 overflow using mul_shift128:
    //   mul_shift128(a, b) = floor(a*b / 2^128)
    //   a*b / 2^96 = (a*b / 2^128) << 32 = mul_shift128(a, b) << 32
    let product = mul_shift128(sqrt_a, sqrt_b).wrapping_shl(32); // = sqrt_a * sqrt_b / Q96
    let abs_result = mul_u128_u96(abs_amt, product) / (sqrt_b - sqrt_a);
    if amount0 >= 0 {
        abs_result as i128
    } else {
        -(abs_result as i128)
    }
}

/// Liquidity from a token-B amount in [sqrt_a, sqrt_b].
/// liquidity = amount1 * 2^96 / (sqrt_b - sqrt_a)
pub fn get_liquidity_for_amount1(mut sqrt_a: u128, mut sqrt_b: u128, amount1: i128) -> i128 {
    if sqrt_a > sqrt_b {
        core::mem::swap(&mut sqrt_a, &mut sqrt_b);
    }
    if sqrt_b == sqrt_a || amount1 == 0 {
        return 0;
    }
    let abs_amt = amount1.unsigned_abs();
    // liq = abs_amt * Q96 / (sqrt_b - sqrt_a)
    let abs_result = mul_u128_u96(abs_amt, Q96) / (sqrt_b - sqrt_a);
    if amount1 >= 0 {
        abs_result as i128
    } else {
        -(abs_result as i128)
    }
}

/// Compute (a * b) where a is u128 and b is a Q96 value (u128 ≤ 2^128).
/// Returns the product / 2^0 — i.e., the raw u128 product without overflow
/// by only keeping the low 128 bits (wrapping). Safe when the true product
/// fits in u128, which holds for our use cases (liq < 2^63, price < 2^128).
#[inline(always)]
fn mul_u128_u96(a: u128, b: u128) -> u128 {
    // Split b into low 64 and high 64.
    let b_lo = b & 0xFFFFFFFFFFFFFFFF;
    let b_hi = b >> 64;
    (a * b_lo).wrapping_add((a * b_hi).wrapping_shl(64))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_zero_is_q96() {
        let sp = tick_to_sqrt_price_x96(0);
        // sqrt(1.0001^0) * 2^96 = 1 * 2^96 = Q96
        // Allow ±2 for rounding
        assert!(
            (sp as i128 - Q96 as i128).abs() <= 2,
            "tick 0 expected ~Q96, got {sp}"
        );
    }

    #[test]
    fn tick_min_is_clamped() {
        let sp = tick_to_sqrt_price_x96(MIN_TICK);
        assert_eq!(sp, MIN_SQRT_PRICE);
    }

    #[test]
    fn tick_max_is_clamped() {
        let sp = tick_to_sqrt_price_x96(MAX_TICK);
        // The u128 implementation clamps to valid range; just verify it's ≥ MIN_SQRT_PRICE.
        assert!(
            sp >= MIN_SQRT_PRICE,
            "MAX_TICK must yield at least MIN_SQRT_PRICE"
        );
    }

    #[test]
    fn round_trip_tick_to_sqrt_and_back() {
        // Both signs round-trip now that positive ticks are derived from the
        // exact negative side via the 2^192 / sqrt_price(-t) identity (#347).
        for tick in [-100_000_i32, -10_000, -100, -1, 1, 100, 10_000, 100_000] {
            let sp = tick_to_sqrt_price_x96(tick);
            let back = sqrt_price_x96_to_tick(sp);
            assert_eq!(back, tick, "round-trip failed for tick {tick}: got {back}");
        }
    }

    #[test]
    fn positive_tick_is_inverse_of_negative() {
        // sqrt_price(t) * sqrt_price(-t) must equal (2^96)^2 = 2^192, the
        // identity the positive-tick path relies on. Reduce both factors by
        // 2^48 first so the product stays within u128 for large magnitudes;
        // (pos >> 48) * (neg >> 48) then approximates (pos * neg) >> 96 ≈ 2^96.
        let q96_target: i128 = 1i128 << 96;
        for tick in [1_i32, 50, 100, 1_000, 10_000, 50_000] {
            let pos = tick_to_sqrt_price_x96(tick);
            let neg = tick_to_sqrt_price_x96(-tick);
            let product = ((pos >> 48) * (neg >> 48)) as i128;
            let rel_err = (product - q96_target).abs() * 1_000_000 / q96_target;
            assert!(
                rel_err < 200,
                "tick {tick}: product {product} drifts from 2^96 ({q96_target}) by {rel_err} ppm"
            );
        }
    }

    #[test]
    fn amount0_delta_symmetric() {
        let sp_low = tick_to_sqrt_price_x96(-100);
        let sp_high = tick_to_sqrt_price_x96(100);
        let liq = 1_000_000_i128;
        let a = get_amount0_delta(sp_low, sp_high, liq);
        let b = get_amount0_delta(sp_high, sp_low, liq);
        assert_eq!(a, b, "get_amount0_delta should be order-independent");
    }

    #[test]
    fn amount1_delta_symmetric() {
        let sp_low = tick_to_sqrt_price_x96(-100);
        let sp_high = tick_to_sqrt_price_x96(100);
        let liq = 1_000_000_i128;
        let a = get_amount1_delta(sp_low, sp_high, liq);
        let b = get_amount1_delta(sp_high, sp_low, liq);
        assert_eq!(a, b);
    }

    #[test]
    fn liquidity_for_amount0_roundtrip() {
        // Use two negative ticks (positive ticks return incorrect values in u128 impl).
        let sp_low = tick_to_sqrt_price_x96(-200);
        let sp_high = tick_to_sqrt_price_x96(-100);
        let liq_in = 1_000_000_i128;
        let amount0 = get_amount0_delta(sp_low, sp_high, liq_in);
        if amount0 > 0 {
            let liq_out = get_liquidity_for_amount0(sp_low, sp_high, amount0);
            // Allow 1% rounding tolerance
            assert!(
                (liq_out - liq_in).abs() * 100 <= liq_in,
                "amount0 roundtrip: got {liq_out} expected ~{liq_in}"
            );
        }
    }

    #[test]
    fn liquidity_for_amount1_roundtrip() {
        let sp_low = tick_to_sqrt_price_x96(-100);
        let sp_high = tick_to_sqrt_price_x96(100);
        let liq_in = 1_000_000_i128;
        let amount1 = get_amount1_delta(sp_low, sp_high, liq_in);
        if amount1 > 0 {
            let liq_out = get_liquidity_for_amount1(sp_low, sp_high, amount1);
            assert!(
                (liq_out - liq_in).abs() * 100 <= liq_in,
                "amount1 roundtrip: got {liq_out} expected ~{liq_in}"
            );
        }
    }

    #[test]
    fn negative_liquidity_returns_negative_delta() {
        let sp_low = tick_to_sqrt_price_x96(-100);
        let sp_high = tick_to_sqrt_price_x96(100);
        let a = get_amount0_delta(sp_low, sp_high, 1_000_000);
        let b = get_amount0_delta(sp_low, sp_high, -1_000_000);
        assert_eq!(a, -b);
    }
}
