//! Exact decimal spellings for identity and scalar key comparison.
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
    Decimal::parse(text).is_some_and(|value| !value.scale.starts_with('-'))
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

struct Decimal {
    negative: bool,
    digits: String,
    scale: String,
}
impl Decimal {
    fn parse(text: &str) -> Option<Self> {
        let (negative, whole, fraction, exponent) = parts(text)?;
        let mut digits = format!("{whole}{fraction}")
            .trim_start_matches('0')
            .to_string();
        if digits.is_empty() {
            return Some(Self {
                negative: false,
                digits: "0".into(),
                scale: "0".into(),
            });
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
    fn absolute_cmp(&self, other: &Self) -> Ordering {
        if self.digits == "0" || other.digits == "0" {
            return (self.digits != "0").cmp(&(other.digits != "0"));
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

#[cfg(test)]
mod tests;
