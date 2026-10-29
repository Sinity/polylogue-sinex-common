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
