//! Frozen duration normalization used by historical question identity.
use jiff::SignedDuration;

/// A duration represented exactly at nanosecond precision. Decimal input
/// uses integer arithmetic; overflow and fractional nanoseconds are refused.
pub fn duration(text: &str) -> Option<SignedDuration> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = text.split_at(split);
    let scale: i128 = match unit {
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        "w" => 604_800_000_000_000,
        _ => return None,
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let fraction = fraction.trim_end_matches('0');
    let factor = 10i128.checked_pow(fraction.len().try_into().ok()?)?;
    let fractional_numerator = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i128>().ok()?.checked_mul(scale)?
    };
    if fractional_numerator % factor != 0 {
        return None;
    }
    let nanos = whole
        .parse::<i128>()
        .ok()?
        .checked_mul(scale)?
        .checked_add(fractional_numerator / factor)?;
    SignedDuration::try_from_nanos_i128(nanos)
}
/// The exact duration only when it can be represented in whole milliseconds.
pub fn duration_ms(text: &str) -> Option<i64> {
    let nanos = duration(text)?.as_nanos();
    if nanos % 1_000_000 != 0 {
        return None;
    }
    (nanos / 1_000_000).try_into().ok()
}
