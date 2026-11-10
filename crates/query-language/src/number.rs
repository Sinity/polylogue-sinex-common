//! Exact decimals: validation, identity keys, comparison and scaling to
//! integers. Both products convert decimal text (query literals, JSON numbers
//! under `arbitrary_precision`, source timestamps) through [`Decimal`] and no
//! other decimal code.
use std::cmp::Ordering;
/// A finite decimal lexeme, including decimal fractions and scientific notation.
/// Validation does not convert to a machine integer or float and does not bound
/// the exponent. Products preserve the raw scalar and apply physical SQL limits.
pub fn is_decimal(text: &str) -> bool {
    Decimal::parse(text).is_some()
}

/// Whether an exact decimal lexeme represents an integer. Equivalent integral
/// spellings such as `1.0` and `1e3` are accepted without a machine-width cap.
pub fn is_integer(text: &str) -> bool {
    Decimal::parse(text).is_some_and(|value| value.is_integer())
}

// One lexical rule is shared by validation, identity and key comparison. The
// scale stays symbolic, so the accepted lexeme also has an exact canonical key.
fn parts(text: &str) -> Option<(bool, &str, &str, &str)> {
    let (negative, text) = text
        .strip_prefix('-')
        .map_or((false, text.strip_prefix('+').unwrap_or(text)), |text| {
            (true, text)
        });
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
    let exponent_digits = exponent
        .strip_prefix('-')
        .or_else(|| exponent.strip_prefix('+'))
        .unwrap_or(exponent);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|digit| digit.is_ascii_digit())
        || exponent_digits.is_empty()
        || !exponent_digits.bytes().all(|digit| digit.is_ascii_digit())
    {
        return None;
    }
    Some((negative, whole, fraction, exponent))
}

/// An exact finite decimal: the one decimal type of both products. It holds
/// the significant digits and a symbolic power-of-ten scale, so parsing,
/// comparison and scaling cost what the spelling costs, never what the
/// exponent's value would. Equal values compare equal however they are
/// spelled (`1.50`, `15e-1`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Decimal {
    negative: bool,
    digits: String,
    scale: String,
}
impl Decimal {
    /// Parse a decimal lexeme: an optional sign, digits with an optional
    /// fraction, and an optional exponent. `NaN`, infinities, whitespace and
    /// separators are refused.
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, whole, fraction, exponent) = parts(text)?;
        let mut digits = format!("{whole}{fraction}")
            .trim_start_matches('0')
            .to_string();
        if digits.is_empty() {
            return Some(Self::zero());
        }
        let trailing = digits
            .bytes()
            .rev()
            .take_while(|digit| *digit == b'0')
            .count();
        digits.truncate(digits.len() - trailing);
        let adjustment = trailing as i128 - fraction.len() as i128;
        let scale = add_integer(exponent, &adjustment.to_string());
        Some(Self {
            negative,
            digits,
            scale,
        })
    }

    fn zero() -> Self {
        Self {
            negative: false,
            digits: "0".into(),
            scale: "0".into(),
        }
    }

    fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    /// Whether the value is below zero. Zero is never negative, however spelled.
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// Whether the value is a whole number (`1.0` and `1e3` are).
    pub fn is_integer(&self) -> bool {
        !self.scale.starts_with('-')
    }

    /// The value times `10^places`, rounded to the nearest integer with half
    /// ties away from zero. `None` when the rounded result is outside `i128`.
    /// A product converts the result to its own column width with `try_from`,
    /// so an out-of-range value is refused rather than saturated.
    pub fn scaled_i128(&self, places: i64) -> Option<i128> {
        if self.is_zero() {
            return Some(0);
        }
        let scale = add_integer(&self.scale, &places.to_string());
        // The count of digits left of the decimal point after scaling.
        let width = add_integer(&scale, &self.digits.len().to_string());
        let magnitude = match integer_cmp(&width, "0") {
            Ordering::Less => 0,
            Ordering::Equal => u128::from(self.digits.as_bytes()[0] >= b'5'),
            Ordering::Greater => {
                if integer_cmp(&width, "39") == Ordering::Greater {
                    return None;
                }
                let width: usize = width.parse().ok()?;
                let mut integer = 0u128;
                for digit in self
                    .digits
                    .bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(width)
                {
                    integer = integer
                        .checked_mul(10)?
                        .checked_add(u128::from(digit - b'0'))?;
                }
                if self
                    .digits
                    .as_bytes()
                    .get(width)
                    .is_some_and(|digit| *digit >= b'5')
                {
                    integer = integer.checked_add(1)?;
                }
                integer
            }
        };
        if self.negative {
            0i128.checked_sub_unsigned(magnitude)
        } else {
            i128::try_from(magnitude).ok()
        }
    }

    /// The value times `10^places` when that is a whole number inside
    /// `i128`; `None` when it would need rounding or is out of range. For
    /// integral fields (`1.5` milliseconds is refused) and for rates that must
    /// stay exact.
    pub fn exact_scaled_i128(&self, places: i64) -> Option<i128> {
        let fractional = !self.is_zero()
            && integer_cmp(&add_integer(&self.scale, &places.to_string()), "0") == Ordering::Less;
        if fractional {
            None
        } else {
            self.scaled_i128(places)
        }
    }

    fn absolute_cmp(&self, other: &Self) -> Ordering {
        if self.is_zero() || other.is_zero() {
            return (!self.is_zero()).cmp(&!other.is_zero());
        }
        let magnitude = add_integer(&self.scale, &self.digits.len().to_string());
        let other_magnitude = add_integer(&other.scale, &other.digits.len().to_string());
        let order = integer_cmp(&magnitude, &other_magnitude);
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
// Arithmetic on signed decimal exponent strings stays proportional to the
// lexeme, never to the represented magnitude. Inputs have passed `parts`.
fn integer_parts(text: &str) -> (bool, &str) {
    let (negative, digits) = text
        .strip_prefix('-')
        .map_or((false, text.strip_prefix('+').unwrap_or(text)), |digits| {
            (true, digits)
        });
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        (false, "0")
    } else {
        (negative, digits)
    }
}
fn unsigned_cmp(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}
fn integer_cmp(left: &str, right: &str) -> Ordering {
    let (a, left) = integer_parts(left);
    let (b, right) = integer_parts(right);
    match (a, b) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (negative, _) => {
            let order = unsigned_cmp(left, right);
            if negative { order.reverse() } else { order }
        }
    }
}
fn add_integer(left: &str, right: &str) -> String {
    let (a, left) = integer_parts(left);
    let (b, right) = integer_parts(right);
    let (negative, digits) = if a == b {
        (a, add_digits(left, right))
    } else {
        match unsigned_cmp(left, right) {
            Ordering::Equal => return "0".into(),
            Ordering::Greater => (a, subtract_digits(left, right)),
            Ordering::Less => (b, subtract_digits(right, left)),
        }
    };
    if negative {
        format!("-{digits}")
    } else {
        digits
    }
}
fn add_digits(left: &str, right: &str) -> String {
    let mut left = left.bytes().rev();
    let mut right = right.bytes().rev();
    let mut carry = 0;
    let mut out = Vec::new();
    loop {
        let (a, b) = (left.next(), right.next());
        if a.is_none() && b.is_none() && carry == 0 {
            break;
        }
        let sum = a.map_or(0, |value| value - b'0') + b.map_or(0, |value| value - b'0') + carry;
        out.push(b'0' + sum % 10);
        carry = sum / 10;
    }
    out.reverse();
    String::from_utf8(out).expect("decimal digits")
}
fn subtract_digits(left: &str, right: &str) -> String {
    let mut right = right.bytes().rev();
    let mut borrow = 0i8;
    let mut out = Vec::with_capacity(left.len());
    for digit in left.bytes().rev() {
        let difference =
            (digit - b'0') as i8 - right.next().map_or(0, |value| (value - b'0') as i8) - borrow;
        borrow = i8::from(difference < 0);
        out.push(
            b'0' + if difference < 0 {
                difference + 10
            } else {
                difference
            } as u8,
        );
    }
    while out.len() > 1 && out.last() == Some(&b'0') {
        out.pop();
    }
    out.reverse();
    String::from_utf8(out).expect("decimal digits")
}

/// Exponents remain symbolic, without allocating in proportion to the exponent.
pub(crate) fn canonical(text: &str) -> Option<String> {
    let value = Decimal::parse(text)?;
    if value.is_zero() {
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
    Some(Decimal::parse(left)?.cmp(&Decimal::parse(right)?))
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (negative, _) => {
                let order = self.absolute_cmp(other);
                if negative { order.reverse() } else { order }
            }
        }
    }
}
impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests;
