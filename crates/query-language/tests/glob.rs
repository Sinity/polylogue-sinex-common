mod support;
use query_language::ast::Expression;
use query_language::conformance::{Dataset, ReferenceEvaluator, Row, Scalar};

#[test]
fn star_patterns_preserve_literal_case_unicode_and_sql_metacharacters() {
    for (text, pattern, expected) in [
        ("", "", true),
        ("x", "", false),
        ("", "*", true),
        ("terminal.focus", "terminal.*", true),
        ("Terminal.focus", "terminal.*", false),
        ("terminal.focus", "terminal", false),
        ("a💡中b", "a*中b", true),
        ("a💡中b", "a*中", false),
        ("abac", "*ab*ac", true),
        ("abac", "*ac*ab", false),
        ("ab", "**a***b**", true),
        ("ab", "a*a*b", false),
        ("x%_\\y", "*%_\\*", true),
        ("x12y", "*%_\\*", false),
        ("a?b", "a?b", true),
        ("acb", "a?b", false),
    ] {
        assert_eq!(
            query_language::glob::matches(text, pattern),
            expected,
            "{text:?} {pattern:?}"
        );
    }
}

#[test]
fn glob_is_a_bound_boolean_expression_and_missing_inputs_stay_unknown() {
    let catalog = support::sinex();
    let query =
        query_language::parse::parse("events where GLOB(source, \"terminal.*\") = true", &catalog)
            .unwrap();
    let bound = query_language::bind::bind(&query, &catalog).unwrap();
    let formatted = query_language::format::format_query(bound.query(), &catalog).unwrap();
    assert!(
        formatted.contains("glob(source, \"terminal.*\")"),
        "{formatted}"
    );
    for text in [
        "events | fields glob(1, \"*\")",
        "events | fields glob(source)",
    ] {
        assert!(
            query_language::parse::parse(text, &catalog).is_err(),
            "{text}"
        );
    }
    let rows = Dataset::from([(
        "events".into(),
        vec![
            Row::new("yes").with("source", "terminal.focus"),
            Row::new("case").with("source", "Terminal.focus"),
            Row::new("missing"),
        ],
    )]);
    let answer = ReferenceEvaluator::new(&rows).execute(&query).unwrap();
    assert_eq!(
        answer
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["yes"]
    );
    let data = Dataset::new();
    let evaluator = ReferenceEvaluator::new(&data);
    let expression = Expression::Function {
        name: "glob".into(),
        arguments: vec![Expression::field("text"), Expression::field("pattern")],
    };
    for (row, expected) in [
        (
            Row::new("yes")
                .with("text", "A%_\\中")
                .with("pattern", "A*中"),
            Scalar::Bool(true),
        ),
        (
            Row::new("case")
                .with("text", "a%_\\中")
                .with("pattern", "A*中"),
            Scalar::Bool(false),
        ),
        (
            Row::new("unknown-text").with("pattern", "*"),
            Scalar::Unknown,
        ),
        (
            Row::new("unknown-pattern").with("text", "a"),
            Scalar::Unknown,
        ),
    ] {
        assert_eq!(evaluator.expression(&row, &expression).unwrap(), expected);
    }
    assert!(
        evaluator
            .expression(
                &Row::new("bad").with("text", 1.).with("pattern", "*"),
                &expression
            )
            .is_err()
    );
}
