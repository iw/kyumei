//! How a series' timestamps and values are written.
//!
//! Timestamps are milliseconds, written as the first one, the first gap, then
//! each change in the gap. A range query's samples are evenly spaced, so after
//! the first two they are runs of zeros, which compression then removes; gaps
//! where a series had no sample cost a few bytes each.
//!
//! Values use whichever of four exact encodings is smallest for the series:
//!
//! * integers: the first value, then each change, for series whose values are
//!   all whole numbers (counts, slots, pollers);
//! * decimals: values written as integers times one shared power of ten,
//!   taken from each value's shortest decimal form, for series whose values
//!   have the same number of decimal places;
//! * significant digits: each value as a mantissa and a power of ten that
//!   changes only when it must, for series that span magnitudes, such as
//!   values rounded to a few significant digits;
//! * float bits: each value's 64 bits XORed with the previous value's, keeping
//!   only the bytes that differ, as Prometheus stores samples. This one takes
//!   anything, NaN and infinities included.
//!
//! All four give back the same bits that went in. Arithmetic wraps, so any
//! input round-trips and corrupt input cannot overflow.

use crate::wire::{Reader, WireError, Writer};

const INTEGERS: u8 = 0;
const DECIMALS: u8 = 1;
const FLOAT_BITS: u8 = 2;
const SIGNIFICANT: u8 = 3;

/// Integers up to this size are exact in an `f64`.
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

pub(crate) fn write_times(w: &mut Writer, times: &[i64]) {
    let mut prev = 0i64;
    let mut prev_gap = 0i64;
    for (i, &t) in times.iter().enumerate() {
        match i {
            0 => w.ivarint(t),
            _ => {
                let gap = t.wrapping_sub(prev);
                w.ivarint(if i == 1 {
                    gap
                } else {
                    gap.wrapping_sub(prev_gap)
                });
                prev_gap = gap;
            }
        }
        prev = t;
    }
}

pub(crate) fn read_times(r: &mut Reader<'_>, n: usize) -> Result<Vec<i64>, WireError> {
    let mut times = Vec::with_capacity(n);
    let mut prev = 0i64;
    let mut gap = 0i64;
    for i in 0..n {
        let v = r.ivarint()?;
        let t = match i {
            0 => v,
            1 => {
                gap = v;
                prev.wrapping_add(gap)
            }
            _ => {
                gap = gap.wrapping_add(v);
                prev.wrapping_add(gap)
            }
        };
        times.push(t);
        prev = t;
    }
    Ok(times)
}

/// Write values with the smallest exact encoding for them.
pub(crate) fn write_values(w: &mut Writer, values: &[f64]) {
    let mut best = (FLOAT_BITS, float_bits(values));
    for candidate in [
        integers(values).map(|b| (INTEGERS, b)),
        decimals(values).map(|b| (DECIMALS, b)),
        significant(values).map(|b| (SIGNIFICANT, b)),
    ]
    .into_iter()
    .flatten()
    {
        if candidate.1.len() < best.1.len() {
            best = candidate;
        }
    }
    w.u8(best.0);
    w.bytes(&best.1);
}

pub(crate) fn read_values(r: &mut Reader<'_>, n: usize) -> Result<Vec<f64>, WireError> {
    match r.u8()? {
        INTEGERS => Ok(read_integers(r, n)?.into_iter().map(|v| v as f64).collect()),
        DECIMALS => {
            let exponent = r.ivarint()?;
            let exponent = i32::try_from(exponent)
                .map_err(|_| WireError("a decimal exponent is out of range"))?;
            read_integers(r, n)?
                .into_iter()
                .map(|m| from_decimal(m, exponent))
                .collect()
        }
        FLOAT_BITS => read_float_bits(r, n),
        SIGNIFICANT => read_significant(r, n),
        _ => Err(WireError("a series uses an unknown value encoding")),
    }
}

fn is_negative_zero(v: f64) -> bool {
    v == 0.0 && v.is_sign_negative()
}

fn write_integers(w: &mut Writer, ints: &[i64]) {
    let mut prev = 0i64;
    for &v in ints {
        w.ivarint(v.wrapping_sub(prev));
        prev = v;
    }
}

fn read_integers(r: &mut Reader<'_>, n: usize) -> Result<Vec<i64>, WireError> {
    let mut out = Vec::with_capacity(n);
    let mut prev = 0i64;
    for _ in 0..n {
        prev = prev.wrapping_add(r.ivarint()?);
        out.push(prev);
    }
    Ok(out)
}

fn integers(values: &[f64]) -> Option<Vec<u8>> {
    let ints = values
        .iter()
        .map(|&v| {
            (v.is_finite()
                && v.fract() == 0.0
                && v.abs() <= MAX_EXACT_INTEGER
                && !is_negative_zero(v))
            .then_some(v as i64)
        })
        .collect::<Option<Vec<i64>>>()?;
    let mut w = Writer::default();
    write_integers(&mut w, &ints);
    Some(w.buf)
}

/// A value's shortest decimal form as `mantissa × 10^exponent`.
fn shortest_decimal(v: f64) -> Option<(i64, i32)> {
    if !v.is_finite() || is_negative_zero(v) {
        return None;
    }
    if v == 0.0 {
        return Some((0, 0));
    }
    let text = format!("{v:e}");
    let (mantissa, exponent) = text.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    let fraction_digits = mantissa.split_once('.').map_or(0, |(_, f)| f.len());
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let magnitude: i64 = digits.parse().ok()?;
    let mantissa = if mantissa.starts_with('-') {
        -magnitude
    } else {
        magnitude
    };
    Some((mantissa, exponent - i32::try_from(fraction_digits).ok()?))
}

fn from_decimal(mantissa: i64, exponent: i32) -> Result<f64, WireError> {
    format!("{mantissa}e{exponent}")
        .parse()
        .map_err(|_| WireError("a decimal value cannot be read"))
}

fn decimals(values: &[f64]) -> Option<Vec<u8>> {
    let parts = values
        .iter()
        .map(|&v| shortest_decimal(v))
        .collect::<Option<Vec<_>>>()?;
    let exponent = parts
        .iter()
        .filter(|(m, _)| *m != 0)
        .map(|(_, e)| *e)
        .min()
        .unwrap_or(0);
    let mut ints = Vec::with_capacity(parts.len());
    for (&(m, e), &v) in parts.iter().zip(values) {
        let scaled = if m == 0 {
            0
        } else {
            let shift = u32::try_from(e - exponent).ok()?;
            m.checked_mul(10i64.checked_pow(shift)?)?
        };
        // The shortest form parses back to the same bits; check rather than trust.
        if from_decimal(scaled, exponent).ok()?.to_bits() != v.to_bits() {
            return None;
        }
        ints.push(scaled);
    }
    let mut w = Writer::default();
    w.ivarint(i64::from(exponent));
    write_integers(&mut w, &ints);
    Some(w.buf)
}

/// Each value as a mantissa and a power of ten that holds steady: a value
/// keeps the previous one's exponent when it can be written exactly with it
/// and the mantissa grows at most a hundredfold, else it takes its own.
/// Exponents are written first, as changes (mostly none); then mantissas, as
/// changes while the exponent stays the same, else whole.
fn significant(values: &[f64]) -> Option<Vec<u8>> {
    let mut chosen = Vec::with_capacity(values.len());
    let mut current: Option<i32> = None;
    for &v in values {
        let (m, e) = shortest_decimal(v)?;
        let (m, e) = match current {
            _ if m == 0 => (0, current.unwrap_or(0)),
            Some(c) if e >= c && e - c <= 2 => (
                m.checked_mul(10i64.checked_pow(u32::try_from(e - c).ok()?)?)?,
                c,
            ),
            _ => (m, e),
        };
        if from_decimal(m, e).ok()?.to_bits() != v.to_bits() {
            return None;
        }
        current = Some(e);
        chosen.push((m, e));
    }
    // Exponents as runs: how many values share one, and its change from the last.
    let mut runs: Vec<(u64, i64)> = Vec::new();
    for &(_, e) in &chosen {
        let e = i64::from(e);
        match runs.last_mut() {
            Some((len, last)) if *last == e => *len += 1,
            _ => runs.push((1, e)),
        }
    }
    let mut w = Writer::default();
    w.uvarint(runs.len() as u64);
    let mut prev_e = 0i64;
    for &(len, e) in &runs {
        w.uvarint(len);
        w.ivarint(e.wrapping_sub(prev_e));
        prev_e = e;
    }
    let mut prev: Option<(i64, i32)> = None;
    for &(m, e) in &chosen {
        match prev {
            Some((pm, pe)) if pe == e => w.ivarint(m.wrapping_sub(pm)),
            _ => w.ivarint(m),
        }
        prev = Some((m, e));
    }
    Some(w.buf)
}

fn read_significant(r: &mut Reader<'_>, n: usize) -> Result<Vec<f64>, WireError> {
    let mut exponents = Vec::with_capacity(n);
    let mut e = 0i64;
    for _ in 0..r.count()? {
        let len = r.uvarint()?;
        e = e.wrapping_add(r.ivarint()?);
        let exponent =
            i32::try_from(e).map_err(|_| WireError("a decimal exponent is out of range"))?;
        if len > (n - exponents.len()) as u64 {
            return Err(WireError(
                "exponent runs cover more values than the series has",
            ));
        }
        exponents.extend(std::iter::repeat_n(exponent, len as usize));
    }
    if exponents.len() != n {
        return Err(WireError(
            "exponent runs cover fewer values than the series has",
        ));
    }
    let mut out = Vec::with_capacity(n);
    let mut prev: Option<(i64, i32)> = None;
    for e in exponents {
        let v = r.ivarint()?;
        let m = match prev {
            Some((pm, pe)) if pe == e => pm.wrapping_add(v),
            _ => v,
        };
        out.push(from_decimal(m, e)?);
        prev = Some((m, e));
    }
    Ok(out)
}

fn float_bits(values: &[f64]) -> Vec<u8> {
    let mut w = Writer::default();
    let mut prev = 0u64;
    for (i, v) in values.iter().enumerate() {
        let bits = v.to_bits();
        if i == 0 {
            w.bytes(&bits.to_le_bytes());
        } else {
            let x = bits ^ prev;
            if x == 0 {
                w.u8(0);
            } else {
                let tz = x.trailing_zeros() / 8;
                let lz = x.leading_zeros() / 8;
                let meaningful = 8 - lz - tz;
                w.u8(((tz as u8) << 4) | meaningful as u8);
                w.bytes(&(x >> (8 * tz)).to_le_bytes()[..meaningful as usize]);
            }
        }
        prev = bits;
    }
    w.buf
}

fn read_float_bits(r: &mut Reader<'_>, n: usize) -> Result<Vec<f64>, WireError> {
    let mut out = Vec::with_capacity(n);
    let mut prev = 0u64;
    for i in 0..n {
        let bits = if i == 0 {
            let b = r.bytes(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_le_bytes(a)
        } else {
            let header = r.u8()?;
            let x = if header == 0 {
                0
            } else {
                let tz = u32::from(header >> 4);
                let meaningful = usize::from(header & 0x0f);
                if meaningful == 0 || tz as usize + meaningful > 8 {
                    return Err(WireError("a float in the samples is malformed"));
                }
                let mut a = [0u8; 8];
                a[..meaningful].copy_from_slice(r.bytes(meaningful)?);
                u64::from_le_bytes(a) << (8 * tz)
            };
            prev ^ x
        };
        out.push(f64::from_bits(bits));
        prev = bits;
    }
    Ok(out)
}

/// Round a value to `digits` significant digits, as `--round` does. NaN, the
/// infinities and zero are returned unchanged.
pub fn round_significant(v: f64, digits: u8) -> f64 {
    if !v.is_finite() || v == 0.0 || digits == 0 {
        return v;
    }
    let precision = usize::from(digits.saturating_sub(1));
    format!("{v:.precision$e}").parse().unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(values: &[f64]) -> (u8, Vec<f64>) {
        let mut w = Writer::default();
        write_values(&mut w, values);
        let codec = w.buf.first().copied().unwrap_or(u8::MAX);
        let mut r = Reader::new(&w.buf);
        let back = read_values(&mut r, values.len()).expect("read");
        assert_eq!(r.remaining(), 0);
        (codec, back)
    }

    fn assert_bits(a: &[f64], b: &[f64]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x.to_bits(), y.to_bits(), "{x} vs {y}");
        }
    }

    #[test]
    fn whole_numbers_use_integers() {
        let values = [812.0, 812.0, 790.0, 0.0, 4769.0, 4770.0];
        let (codec, back) = round_trip(&values);
        assert_eq!(codec, INTEGERS);
        assert_bits(&values, &back);
    }

    #[test]
    fn short_decimals_use_decimals() {
        let values = [0.25, 1.5, 12.75, 0.0, 3.125, 100.0];
        let (codec, back) = round_trip(&values);
        assert_eq!(codec, DECIMALS);
        assert_bits(&values, &back);
    }

    #[test]
    fn values_across_magnitudes_use_significant_digits() {
        // Rounded to three digits: under a second before an incident, then
        // around 150 during it.
        let noise = |i: u32| f64::from((i * 7919) % 200) / 1000.0;
        let values: Vec<f64> = (0..60)
            .map(|i| round_significant(0.8 + noise(i), 3))
            .chain((0..60).map(|i| round_significant(150.0 + noise(i) * 300.0, 3)))
            .collect();
        let (codec, back) = round_trip(&values);
        assert_eq!(codec, SIGNIFICANT);
        assert_bits(&values, &back);
    }

    #[test]
    fn anything_else_round_trips_as_float_bits() {
        let values = [
            0.03333333333333333,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            1.0 / 3.0,
            -2.5e-300,
            5e-324,
        ];
        let (codec, back) = round_trip(&values);
        assert_eq!(codec, FLOAT_BITS);
        assert_bits(&values, &back);
    }

    #[test]
    fn negative_zero_and_nan_payloads_keep_their_bits() {
        let odd_nan = f64::from_bits(0x7ff8_0000_0000_0001);
        let values = [-0.0, 0.0, odd_nan, -0.0];
        let (_, back) = round_trip(&values);
        assert_bits(&values, &back);
    }

    #[test]
    fn counter_resets_round_trip() {
        let values = [100.0, 250.0, 400.0, 3.0, 90.0, 0.0, 12.0];
        let (_, back) = round_trip(&values);
        assert_bits(&values, &back);
    }

    #[test]
    fn timestamps_with_gaps_round_trip() {
        let times = [
            1_697_180_000_000,
            1_697_180_015_000,
            1_697_180_030_000,
            1_697_180_090_000,
            1_697_180_105_123,
            i64::MAX,
            i64::MIN,
            0,
        ];
        let mut w = Writer::default();
        write_times(&mut w, &times);
        let mut r = Reader::new(&w.buf);
        assert_eq!(read_times(&mut r, times.len()).expect("read"), times);
    }

    #[test]
    fn even_steps_cost_a_byte_each() {
        let times: Vec<i64> = (0..120).map(|i| 1_697_180_000_000 + i * 15_000).collect();
        let mut w = Writer::default();
        write_times(&mut w, &times);
        assert!(w.buf.len() < 130, "{} bytes", w.buf.len());
    }

    #[test]
    fn rounding_keeps_significant_digits() {
        assert_eq!(round_significant(0.033333333, 3), 0.0333);
        assert_eq!(round_significant(123_456.0, 2), 120_000.0);
        assert_eq!(round_significant(-1.23456, 4), -1.235);
        assert!(round_significant(f64::NAN, 3).is_nan());
        assert_eq!(round_significant(0.0, 3), 0.0);
    }
}
