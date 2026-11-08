use super::test_features;

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn focused_test_selection_keeps_conformance_cases_and_filters() {
    let selection = args(&["-p", "query-language", "-E", "test(compiler_cases)"]);
    let mut expected = vec!["--all-features".to_owned()];
    expected.extend(selection.clone());
    assert_eq!(test_features(selection), expected);
}

#[test]
fn explicit_feature_configuration_is_preserved() {
    for features in [
        vec!["--all-features"],
        vec!["--no-default-features"],
        vec!["--features", "conformance"],
        vec!["--features=conformance"],
        vec!["-F", "conformance"],
        vec!["-Fconformance"],
    ] {
        let mut selection = args(&["-p", "query-language"]);
        selection.extend(args(&features));
        assert_eq!(test_features(selection.clone()), selection);
    }
}

#[test]
fn default_feature_argument_precedes_test_name_separator() {
    assert_eq!(
        test_features(args(&["--", "--features"])),
        args(&["--all-features", "--", "--features"])
    );
}
