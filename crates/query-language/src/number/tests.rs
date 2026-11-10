use super::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn symbolic_scale_arithmetic_matches_independent_small_integer_oracle(a in any::<i64>(), b in any::<i64>()) {
        let expected = i128::from(a) + i128::from(b);
        prop_assert_eq!(add_integer(&a.to_string(), &b.to_string()), expected.to_string());
        prop_assert_eq!(integer_cmp(&a.to_string(), &b.to_string()), a.cmp(&b));
    }

    #[test]
    fn exact_decimal_comparison_matches_independent_scaled_integer_oracle(
        a in -1_000_000i64..1_000_000, ae in -6i32..7,
        b in -1_000_000i64..1_000_000, be in -6i32..7,
    ) {
        let left = i128::from(a) * 10i128.pow((ae + 6) as u32);
        let right = i128::from(b) * 10i128.pow((be + 6) as u32);
        prop_assert_eq!(compare(&format!("{a}e{ae}"), &format!("{b}e{be}")), Some(left.cmp(&right)));
    }
}

#[test]
fn decimal_comparison_and_canonical_keys_keep_large_exponents_exact() {
    let exponent = "100000000000000000000000000000000";
    assert_eq!(
        canonical(&format!("1e{exponent}")),
        Some(format!("1e{exponent}"))
    );
    assert_eq!(
        compare(
            &format!("1e{exponent}"),
            "10e99999999999999999999999999999999"
        ),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare(
            &format!("-1e{exponent}"),
            "-1e99999999999999999999999999999999"
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare(
            &format!("1e-{exponent}"),
            "1e-99999999999999999999999999999999"
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        canonical("1.00000000000000000001"),
        Some("100000000000000000001e-20".into())
    );
}

#[test]
fn canonical_scale_crosses_machine_integer_limits_without_changing_value() {
    for (text, expected) in [
        ("10e9223372036854775807", "1e9223372036854775808"),
        ("1.0e-9223372036854775808", "1e-9223372036854775808"),
        ("0.01e-9223372036854775808", "1e-9223372036854775810"),
        ("-1000.00e-9223372036854775808", "-1e-9223372036854775805"),
        ("-0e-99999999999999999999999999999", "0"),
    ] {
        assert_eq!(canonical(text).as_deref(), Some(expected), "{text}");
    }
    assert_eq!(
        compare("1e9223372036854775808", "10e9223372036854775807"),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare(
            "0e99999999999999999999999999999",
            "-0e-99999999999999999999999999999"
        ),
        Some(Ordering::Equal)
    );
}

fn scaled(text: &str, places: i64) -> Option<i128> {
    Decimal::parse(text)?.scaled_i128(places)
}

#[test]
fn scaled_matches_hand_values_and_refuses_out_of_range() {
    for (text, places, expected) in [
        ("0.1", 9, Some(100_000_000)),
        ("1e300", 9, None),
        ("-1e300", 9, None),
        ("12345678.123456789", 9, Some(12_345_678_123_456_789)),
        (
            "170141183460469231731687303715884105727",
            0,
            Some(i128::MAX),
        ),
        (
            "-170141183460469231731687303715884105728",
            0,
            Some(i128::MIN),
        ),
        (
            "170141183460469231731687303715884105727.4",
            0,
            Some(i128::MAX),
        ),
        ("170141183460469231731687303715884105727.5", 0, None),
        ("170141183460469231731687303715884105728", 0, None),
        ("-170141183460469231731687303715884105728.5", 0, None),
        ("-170141183460469231731687303715884105729", 0, None),
        ("9007199.254740993", 9, Some(9_007_199_254_740_993)),
        (".0000000005", 9, Some(1)),
        ("-.0000000005", 9, Some(-1)),
        (".00000000049", 9, Some(0)),
        ("-.00000000049", 9, Some(0)),
        ("+12300e-3", 1, Some(123)),
        ("15", -1, Some(2)),
        ("-15", -1, Some(-2)),
        ("1.50", 2, Some(150)),
        ("1e100000000000000000000000", 9, None),
        ("-1e100000000000000000000000", 9, None),
        ("1e-100000000000000000000000", 9, Some(0)),
        ("-0e100000000000000000000000", 9, Some(0)),
        ("1", i64::MAX, None),
        ("1", i64::MIN, Some(0)),
        ("NaN", 9, None),
        ("inf", 9, None),
        ("1e", 9, None),
    ] {
        assert_eq!(scaled(text, places), expected, "{text} * 10^{places}");
    }
}

#[test]
fn decimals_compare_by_value_whatever_the_spelling() {
    let parse = |text| Decimal::parse(text).unwrap();
    assert_eq!(parse("1.50"), parse("15e-1"));
    assert_eq!(parse("-0.0"), parse("0"));
    assert!(!parse("-0").is_negative());
    assert!(parse("-1e-9") < parse("0"));
    assert!(parse("1731000000.123456789") > parse("1731000000.123456788"));
    assert!(parse("1e3").is_integer());
    assert!(!parse("1.5").is_integer());
}

proptest! {
    #[test]
    fn scaled_integer_rounding_matches_independent_integer_division(
        numerator in any::<i64>(), scale in 0u32..10,
    ) {
        let denominator = 10i128.pow(scale);
        let magnitude = i128::from(numerator).abs();
        let rounded = (magnitude + denominator / 2) / denominator;
        let expected = if numerator < 0 { -rounded } else { rounded };
        prop_assert_eq!(scaled(&format!("{numerator}e-{scale}"), 0), Some(expected));
    }
}
