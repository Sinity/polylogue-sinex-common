use super::*;

#[test]
fn changes_inside_test_functions_select_the_tests() {
    let before = "#[test]\nfn reads() {\n    assert_eq!(1, 1);\n}\n\n#[devkit::test]\nasync fn writes() {\n    assert_eq!(2, 2);\n}\n";
    let after = before.replace("1, 1", "2, 2").replace("2, 2", "3, 3");
    let names = names_for_diff(before, &after, "@@ -3 +3 @@\n@@ -8 +8 @@").unwrap();
    assert_eq!(names, BTreeSet::from(["reads".into(), "writes".into()]));
}

#[test]
fn helper_changes_and_partial_hunk_coverage_select_the_package() {
    let source = "fn helper() {}\n#[test]\nfn reads() {\n    helper();\n}\n";
    assert!(names_for_diff(source, source, "@@ -1 +1 @@").is_none());
    assert!(names_for_diff(source, source, "@@ -1,5 +1,5 @@").is_none());
    assert!(names_for_diff(source, source, "@@ -4,2 +4,3 @@").is_none());
    assert!(names_for_diff(source, source, "@@ -4,0 +4,1 @@").is_none());
    assert!(names_for_diff(source, source, "@@ -4 +4,0 @@").is_none());
    assert!(names_for_diff(source, "not Rust", "@@ -4 +4 @@").is_none());
    assert!(names_for_diff(source, source, "").is_none());
}

#[test]
fn nested_tests_are_found_without_confusing_helpers_with_tests() {
    let source = "#[cfg(test)]\nmod tests {\n    fn helper() {}\n    #[test]\n    fn reads() { helper(); }\n}\n";
    assert_eq!(
        names_for_diff(source, source, "@@ -5 +5 @@").unwrap(),
        BTreeSet::from(["reads".into()])
    );
    assert!(names_for_diff(source, source, "@@ -3 +3 @@").is_none());
}

#[test]
fn items_sharing_test_lines_keep_package_scope() {
    for source in [
        "fn helper() {} #[test] fn reads() {}",
        "#[test] fn reads() {} #[test] fn writes() {}",
    ] {
        assert!(names_for_diff(source, source, "@@ -1 +1 @@").is_none());
    }
}

#[test]
fn raw_test_names_match_the_names_nextest_lists() {
    let source = "#[test]\nfn r#type() { assert!(true); }\n";
    assert_eq!(
        names_for_diff(source, source, "@@ -2 +2 @@").unwrap(),
        BTreeSet::from(["type".into()])
    );
}
