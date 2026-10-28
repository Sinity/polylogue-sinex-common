//! Exact decimal spellings for identity and scalar key comparison.
use std::cmp::Ordering;
struct Decimal {
    negative: bool,
    digits: String,
    scale: i64,
}
impl Decimal {
    fn parse(text: &str) -> Option<Self> {
        let (negative, text) = text
            .strip_prefix('-')
            .map_or((false, text.strip_prefix('+').unwrap_or(text)), |t| {
                (true, t)
            });
        let (mantissa, exponent) = text
            .split_once(['e', 'E'])
            .map_or(Some((text, 0i64)), |(m, e)| Some((m, e.parse().ok()?)))?;
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty() && fraction.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let mut digits = format!("{whole}{fraction}")
            .trim_start_matches('0')
            .to_string();
        if digits.is_empty() {
            return Some(Self {
                negative: false,
                digits: "0".into(),
                scale: 0,
            });
        }
        let mut scale = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
        while digits.ends_with('0') {
            digits.pop();
            scale = scale.checked_add(1)?;
        }
        Some(Self {
            negative,
            digits,
            scale,
        })
    }
    fn absolute_cmp(&self, other: &Self) -> Ordering {
        if self.digits == "0" || other.digits == "0" {
            return (self.digits != "0").cmp(&(other.digits != "0"));
        }
        let magnitude = i128::from(self.scale) + self.digits.len() as i128;
        let other_magnitude = i128::from(other.scale) + other.digits.len() as i128;
        let order = magnitude.cmp(&other_magnitude);
        if order != Ordering::Equal {
            return order;
        }
        let width = self.digits.len().max(other.digits.len());
        self.digits
            .bytes()
            .chain(std::iter::repeat(b'0'))
            .take(width)
            .cmp(
                other
                    .digits
                    .bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(width),
            )
    }
}
/// Exponents remain symbolic, without allocating in proportion to the exponent.
pub(crate) fn canonical(text: &str) -> Option<String> {
    let value = Decimal::parse(text)?;
    if value.digits == "0" {
        return Some("0".into());
    }
    Some(format!(
        "{}{}e{}",
        if value.negative { "-" } else { "" },
        value.digits,
        value.scale
    ))
}
pub(crate) fn compare(left: &str, right: &str) -> Option<Ordering> {
    let left = Decimal::parse(left)?;
    let right = Decimal::parse(right)?;
    Some(match (left.negative, right.negative) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (negative, _) => {
            let order = left.absolute_cmp(&right);
            if negative { order.reverse() } else { order }
        }
    })
}
