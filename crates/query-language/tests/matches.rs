mod support;
use query_language::{
    ErrorKind,
    ast::{ConditionKind, Source},
    bind::bind,
    catalog::{
        Catalog, Field, FieldKind, MatchOutput, MatchPattern, OutputSchema, OutputSource,
        SourceOutput,
    },
    complete::{CompletionKind, complete, requirements},
    format::format_query,
    identity::normal_form,
    parse::{matches_requirements, parse},
};
fn catalogs() -> [(Catalog, &'static str, &'static str); 2] {
    [
        (
            support::polylogue(),
            "sessions where exists tool_call(outcome:error)",
            "origin",
        ),
        (
            support::sinex(),
            "events where exists commands(exit_code = 1)",
            "source",
        ),
    ]
}
fn output(catalog: &Catalog) -> OutputSchema {
    OutputSchema {
        unit: Some(catalog.default_unit.clone()),
        fields: vec![
            Field {
                name: "probe".into(),
                kind: FieldKind::Integer,
                ..Field::default()
            },
            Field {
                name: "metric".into(),
                kind: FieldKind::Number,
                ..Field::default()
            },
            Field {
                name: "label".into(),
                kind: FieldKind::Text,
                ..Field::default()
            },
            Field {
                name: "status".into(),
                kind: FieldKind::Match,
                values: vec!["ok".into(), "error".into()],
                ..Field::default()
            },
            Field {
                name: "gap_time".into(),
                kind: FieldKind::Duration,
                ..Field::default()
            },
        ],
        ..OutputSchema::default()
    }
}
fn install(catalog: &mut Catalog, text: &str) -> query_language::catalog::MatchSource {
    let requested = matches_requirements(text, catalog).unwrap();
    assert_eq!(requested.len(), 1);
    let source = requested[0].clone();
    catalog.match_outputs.push(MatchOutput {
        source: source.clone(),
        schema: output(catalog),
    });
    source
}
#[test]
fn matches_suffixes_use_only_the_declared_output_scope() {
    for (mut catalog, base, native) in catalogs() {
        let text = format!("{base} | matches | where probe > 0 | sort by probe | fields label");
        let syntax = parse(&text, &catalog).unwrap();
        assert_eq!(
            bind(&syntax, &catalog).unwrap_err().kind,
            ErrorKind::BadValue
        );
        let source = install(&mut catalog, &text);
        let bound = bind(&syntax, &catalog).unwrap();
        let scope = &bound.typing().queries[0].stages[0].after;
        assert_eq!(scope.field("probe").unwrap().field.kind, FieldKind::Integer);
        assert!(scope.field(native).is_none());
        assert_eq!(source.parent_unit, catalog.default_unit);
        assert!(matches!(source.pattern, MatchPattern::Exists { .. }));
        assert_eq!(
            parse(&format!("{base} | matches | where {native}:demo"), &catalog)
                .unwrap_err()
                .kind,
            ErrorKind::UnknownField
        );
        assert_eq!(
            parse(&format!("{base} | matches | where status:solved"), &catalog)
                .unwrap_err()
                .kind,
            ErrorKind::BadValue
        );
        let rendered = format_query(bound.query(), &catalog).unwrap();
        assert_eq!(
            normal_form(bound.query(), &catalog).unwrap(),
            normal_form(&parse(&rendered, &catalog).unwrap(), &catalog).unwrap()
        );
        for changed in [
            format!("| fields {native}"),
            format!("| group by {native}"),
            "| agg count".into(),
            "| matches".into(),
        ] {
            parse(&format!("{base} {changed}"), &catalog).unwrap();
            let error = parse(&format!("{base} {changed} | matches"), &catalog).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidQuery, "{changed}");
            assert!(error.message.contains("matches"), "{changed}: {error}");
        }
    }
}
#[test]
fn matches_schema_drives_completion_and_scalar_types() {
    for (mut catalog, base, native) in catalogs() {
        let prefix = format!("{base} | matches | where pro");
        let wanted = requirements(&prefix, prefix.len(), &catalog);
        assert_eq!(wanted.match_outputs.len(), 1);
        let nested = format!("{base} | matches | where probe in (");
        assert_eq!(
            requirements(&nested, nested.len(), &catalog)
                .match_outputs
                .len(),
            1
        );
        assert!(
            !complete(&prefix, prefix.len(), &catalog)
                .iter()
                .any(|c| c.kind == CompletionKind::Field)
        );
        install(&mut catalog, &format!("{base} | matches"));
        let completed = complete(&prefix, prefix.len(), &catalog);
        assert!(
            completed
                .iter()
                .any(|c| c.kind == CompletionKind::Field && c.value.starts_with("probe"))
        );
        assert!(
            !completed
                .iter()
                .any(|c| c.kind == CompletionKind::Field && c.value.starts_with(native))
        );
        let quoted = format!("{base} | matches | where status:\"e");
        let needed = requirements(&quoted, quoted.len(), &catalog);
        assert_eq!(needed.match_outputs.len(), 1);
        assert_eq!(needed.value.unwrap().field, "status");
        assert!(
            complete(&quoted, quoted.len(), &catalog)
                .iter()
                .any(|c| c.kind == CompletionKind::Value && c.value.contains("error"))
        );
        let form = |condition: &str| {
            normal_form(
                &parse(&format!("{base} | matches | where {condition}"), &catalog).unwrap(),
                &catalog,
            )
            .unwrap()
        };
        assert_eq!(form("metric > 1.0"), form("metric > 1e0"));
        assert_ne!(form("label:1.0"), form("label:1e0"));
    }
}
#[test]
fn saved_relations_retain_only_their_actual_expanded_qualifiers() {
    for (mut catalog, base, _) in catalogs() {
        let expanded = parse(base, &catalog).unwrap();
        let Source::Unit { condition, .. } = &expanded.source else {
            panic!()
        };
        let schema = OutputSchema {
            unit: Some(catalog.default_unit.clone()),
            fields: catalog.unit(&catalog.default_unit).unwrap().fields.clone(),
            match_patterns: query_language::matches::patterns(condition.as_ref()),
            ..OutputSchema::default()
        };
        catalog.source_outputs.push(SourceOutput {
            source: OutputSource::Saved {
                name: "observed".into(),
                arguments: Vec::new(),
            },
            schema,
        });
        let text = "query:observed | matches | where probe > 0";
        let from_saved = matches_requirements(text, &catalog).unwrap();
        let from_native = matches_requirements(&format!("{base} | matches"), &catalog).unwrap();
        assert_eq!(from_saved, from_native);
        install(&mut catalog, text);
        bind(&parse(text, &catalog).unwrap(), &catalog).unwrap();
        catalog.source_outputs[0].schema.match_patterns.clear();
        assert_eq!(
            parse(text, &catalog).unwrap_err().kind,
            ErrorKind::InvalidQuery
        );
    }
}
#[test]
fn sequence_patterns_preserve_all_flags_gaps_and_step_counts_without_aliases() {
    for (mut catalog, _, _) in catalogs() {
        let (step, next) = if catalog.default_unit == "sessions" {
            ("action:shell", "outcome:error")
        } else {
            ("event_type:command.executed", "event_type:command.imported")
        };
        let two = format!("seq({step} -> {next}) | matches");
        let request = install(&mut catalog, &two);
        assert!(matches!(
            &request.pattern,
            MatchPattern::Sequence { steps: 2, .. }
        ));
        let all = format!("seq all({step} -> [next] {next}) | matches | agg p50:gap_time,count");
        assert_eq!(matches_requirements(&all, &catalog).unwrap(), [request]);
        bind(&parse(&all, &catalog).unwrap(), &catalog).unwrap();
        let three = format!("seq all({step} -> [within:5m] {next} -> [next] {step}) | matches");
        let requested = matches_requirements(&three, &catalog).unwrap();
        assert!(matches!(
            &requested[0].pattern,
            MatchPattern::Sequence { steps: 3, .. }
        ));
        assert_eq!(
            bind(&parse(&three, &catalog).unwrap(), &catalog)
                .unwrap_err()
                .kind,
            ErrorKind::BadValue
        );
        let error = query_language::semantics::ReferenceEvaluator {
            data: &Default::default(),
        }
        .execute(&parse(&all, &catalog).unwrap())
        .unwrap_err();
        assert!(error.message.contains("product adapter"));
    }
}
#[test]
fn ambiguous_or_non_top_level_qualifiers_never_choose_a_projection() {
    for (catalog, base, _) in catalogs() {
        let query = parse(base, &catalog).unwrap();
        let Source::Unit {
            condition: Some(condition),
            ..
        } = query.source
        else {
            panic!()
        };
        let ConditionKind::Exists { unit, .. } = &condition.kind else {
            panic!()
        };
        assert_eq!(
            parse(&format!("{base} AND exists {unit}() | matches"), &catalog)
                .unwrap_err()
                .kind,
            ErrorKind::Ambiguous
        );
        assert_eq!(
            parse(&format!("{} | matches", catalog.default_unit), &catalog)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidQuery
        );
        assert!(
            query_language::matches::patterns(Some(&query_language::ast::Condition {
                span: Default::default(),
                kind: ConditionKind::Not {
                    condition: Box::new(condition)
                }
            }))
            .is_empty()
        );
    }
}
#[test]
fn empty_runtime_metadata_keeps_existing_serialized_schema_bytes() {
    let old = serde_json::json!({"unit":null,"fields":[],"main_time":null});
    let schema: OutputSchema = serde_json::from_value(old.clone()).unwrap();
    assert!(schema.match_patterns.is_empty());
    assert_eq!(serde_json::to_value(&schema).unwrap(), old);
    let mut runtime = schema;
    runtime.match_patterns.push(MatchPattern::Exists {
        unit: "events".into(),
    });
    assert_eq!(serde_json::to_value(runtime).unwrap(), old);
    let declaration = serde_json::to_value(schemars::schema_for!(OutputSchema)).unwrap();
    assert!(declaration["properties"].get("match_patterns").is_none());
    let declaration = serde_json::to_value(schemars::schema_for!(Catalog)).unwrap();
    assert!(declaration["properties"].get("match_outputs").is_none());
    assert!(
        serde_json::to_value(Catalog::new("events"))
            .unwrap()
            .get("match_outputs")
            .is_none()
    );
}

#[test]
fn final_relation_metadata_is_collected_by_the_same_parser() {
    for (mut catalog, base, native) in catalogs() {
        assert_eq!(
            query_language::parse::relation_patterns(base, &catalog)
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        for stage in [
            format!("fields {native}"),
            format!("group by {native}"),
            "agg count".into(),
            catalog.default_unit.clone(),
        ] {
            assert_eq!(
                query_language::parse::relation_patterns(&format!("{base} | {stage}"), &catalog)
                    .unwrap(),
                Some(Vec::new()),
                "{stage}"
            );
        }
        let source = format!("{base} | matches");
        assert_eq!(
            query_language::parse::relation_patterns(&source, &catalog).unwrap(),
            None
        );
        install(&mut catalog, &source);
        assert_eq!(
            query_language::parse::relation_patterns(&source, &catalog).unwrap(),
            Some(Vec::new())
        );
        let mut duplicate = catalog.clone();
        duplicate
            .match_outputs
            .push(duplicate.match_outputs[0].clone());
        assert_eq!(
            bind(&parse(&source, &catalog).unwrap(), &duplicate)
                .unwrap_err()
                .kind,
            ErrorKind::Ambiguous
        );
    }
}

#[test]
fn selections_preserve_the_actual_relation_qualifier_and_declared_shape() {
    for (mut catalog, base, _) in catalogs() {
        let source = install(&mut catalog, &format!("{base} | matches"));
        let expected = query_language::parse::relation_patterns(base, &catalog).unwrap();
        for selection in ["limit 1", "offset 1", "first 1", "last 1", "distinct"] {
            let entering = format!("{base} | {selection}");
            assert_eq!(
                query_language::parse::relation_patterns(&entering, &catalog).unwrap(),
                expected
            );
            let text = format!("{entering} | matches | where probe > 0");
            assert_eq!(
                matches_requirements(&text, &catalog).unwrap(),
                [source.clone()]
            );
            bind(&parse(&text, &catalog).unwrap(), &catalog).unwrap();
        }
    }
}

#[test]
fn reference_sequence_projection_uses_only_the_selected_parent_rows() {
    use query_language::semantics::{Dataset, ReferenceEvaluator, Row};
    for (catalog, _, _) in catalogs() {
        let (field, value) = if catalog.default_unit == "sessions" {
            ("action", "shell")
        } else {
            ("event_type", "command.executed")
        };
        let text = format!("seq({field}:{value}) | limit 1 | matches");
        let query = parse(&text, &catalog).unwrap();
        let requested = matches_requirements(&text, &catalog).unwrap();
        let MatchPattern::Sequence {
            unit: child_unit, ..
        } = &requested[0].pattern
        else {
            panic!()
        };
        let mut data = Dataset::new();
        data.insert(
            catalog.default_unit.clone(),
            vec![Row::new("p1"), Row::new("p2")],
        );
        for (id, parent) in [("c1", "p1"), ("c2", "p2")] {
            let mut row = Row::new(id).with(field, value);
            row.parent = Some(parent.into());
            data.entry(child_unit.clone()).or_default().push(row);
        }
        let answer = ReferenceEvaluator { data: &data }.execute(&query).unwrap();
        assert_eq!(answer.total, 1);
        assert_eq!(answer.rows[0].parent.as_deref(), Some("p1"));
        assert_eq!(answer.rows[0].matches, ["c1"]);
    }
}
