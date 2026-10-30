mod support;
use query_language::{
    ErrorKind,
    bind::bind,
    catalog::*,
    complete::{CompletionKind, complete, requirements},
    format::format_query,
    identity::{normal_form, question_id},
    parse::{parse, relation_patterns},
    scope::NodePath,
};
use std::collections::BTreeMap;

struct Case {
    catalog: Catalog,
    unit: &'static str,
    column: &'static str,
    native: &'static str,
    qualifier: &'static str,
}
fn cases() -> [Case; 2] {
    [
        Case {
            catalog: support::polylogue(),
            unit: "tool_calls",
            column: "message",
            native: "session.repo",
            qualifier: "exists tool_call(outcome:error)",
        },
        Case {
            catalog: support::sinex(),
            unit: "events",
            column: "id",
            native: "payload.command",
            qualifier: "exists commands(exit_code = 1)",
        },
    ]
}
fn retained_scope(text: &str, catalog: &Catalog) -> OutputSchema {
    let bound = bind(&parse(text, catalog).unwrap(), catalog).unwrap();
    let typing = bound.typing().query(&NodePath::default()).unwrap();
    let scope = typing
        .stages
        .last()
        .map_or(&typing.source, |stage| &stage.after);
    OutputSchema {
        unit: scope.unit.clone(),
        fields: scope.fields.iter().map(|info| info.field.clone()).collect(),
        main_time: None,
        match_patterns: relation_patterns(&format_query(bound.query(), catalog).unwrap(), catalog)
            .unwrap()
            .unwrap(),
    }
}
fn install(catalog: &mut Catalog, unit: &str, column: &str, qualifier: &str) {
    let scope = retained_scope(&format!("{unit} where {qualifier}"), catalog);
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Saved {
            name: "retained".into(),
            arguments: vec![],
        },
        schema: OutputSchema {
            unit: Some(unit.into()),
            fields: vec![catalog.field(unit, column).unwrap().clone()],
            ..OutputSchema::default()
        },
        relation_scope: Some(scope),
    });
}
#[test]
fn retained_semantic_scope_is_shared_by_suffix_binding_identity_and_completion() {
    for Case {
        mut catalog,
        unit,
        column,
        native,
        qualifier,
    } in cases()
    {
        install(&mut catalog, unit, column, qualifier);
        let text = format!("query:retained {native}:001 | sort by {native}");
        let syntax = parse(&text, &catalog).unwrap();
        let bound = bind(&syntax, &catalog).unwrap();
        let root = bound.typing().query(&NodePath::default()).unwrap();
        assert!(root.source.field(native).is_some());
        assert!(root.source.field(column).is_some());
        assert!(
            !catalog
                .source_output(&syntax.source)
                .unwrap()
                .fields
                .iter()
                .any(|field| field.name == native)
        );
        let quoted = parse(
            &format!("query:retained {native}:\"001\" | sort by {native}"),
            &catalog,
        )
        .unwrap();
        assert_eq!(
            question_id(bound.query(), &catalog, &BTreeMap::new()).unwrap(),
            question_id(&quoted, &catalog, &BTreeMap::new()).unwrap()
        );
        assert_ne!(
            normal_form(bound.query(), &catalog).unwrap(),
            normal_form(
                &parse(&text.replace(":001", ":1"), &catalog).unwrap(),
                &catalog
            )
            .unwrap()
        );
        let formatted = format_query(bound.query(), &catalog).unwrap();
        assert_eq!(
            normal_form(&parse(&formatted, &catalog).unwrap(), &catalog).unwrap(),
            normal_form(bound.query(), &catalog).unwrap()
        );
        let prefix = format!("query:retained | where {}", &native[..native.len() - 1]);
        let found = complete(&prefix, prefix.len(), &catalog);
        assert!(
            found
                .iter()
                .any(|candidate| candidate.kind == CompletionKind::Field
                    && candidate.value == native),
            "{found:?}"
        );
        let value = format!("query:retained | where {native}:001");
        assert_eq!(
            requirements(&value, value.len(), &catalog)
                .value
                .unwrap()
                .field,
            native
        );
    }
}
#[test]
fn retained_scope_is_narrowed_by_actual_group_fields_and_matches_stages() {
    for Case {
        mut catalog,
        unit,
        column,
        native,
        qualifier,
    } in cases()
    {
        install(&mut catalog, unit, column, qualifier);
        let fields = format!("query:retained | fields {column} | where {native}:x");
        assert_eq!(
            parse(&fields, &catalog).unwrap_err().kind,
            ErrorKind::UnknownField
        );
        let grouped = format!("query:retained | group by {native} | where count > 1");
        bind(&parse(&grouped, &catalog).unwrap(), &catalog).unwrap();
        let invalid = format!("{grouped} | where {column}:x");
        assert_eq!(
            parse(&invalid, &catalog).unwrap_err().kind,
            ErrorKind::UnknownField
        );
        let source = catalog.source_outputs[0]
            .relation_scope
            .as_ref()
            .unwrap()
            .match_patterns[0]
            .clone();
        catalog.match_outputs.push(MatchOutput {
            source: MatchSource {
                parent_unit: unit.into(),
                pattern: source,
            },
            schema: OutputSchema {
                unit: Some(unit.into()),
                fields: vec![Field {
                    name: "probe".into(),
                    kind: FieldKind::Integer,
                    ..Field::default()
                }],
                ..OutputSchema::default()
            },
        });
        let matched = "query:retained | matches | where probe > 1";
        let bound = bind(&parse(matched, &catalog).unwrap(), &catalog).unwrap();
        let after = &bound.typing().query(&NodePath::default()).unwrap().stages[0].after;
        assert!(after.field("probe").is_some());
        assert!(after.field(native).is_none());
        assert!(parse(&format!("{matched} | where {native}:x"), &catalog).is_err());
        let prefix = "query:retained | matches | where pro";
        assert!(
            complete(prefix, prefix.len(), &catalog)
                .iter()
                .any(|candidate| candidate.value == "probe")
        );
        assert!(parse("query:retained | matches | matches", &catalog).is_err());
    }
}
#[test]
fn runtime_source_scope_is_not_canonical_data_and_missing_metadata_cannot_widen() {
    for Case {
        mut catalog,
        unit,
        column,
        native,
        qualifier,
    } in cases()
    {
        install(&mut catalog, unit, column, qualifier);
        let snapshot = &catalog.source_outputs[0];
        let bytes = serde_json::to_vec(snapshot).unwrap();
        let mut produced_only = snapshot.clone();
        produced_only.relation_scope = None;
        assert_eq!(bytes, serde_json::to_vec(&produced_only).unwrap());
        assert!(!String::from_utf8_lossy(&bytes).contains("relation_scope"));
        let schema = serde_json::to_string(&schemars::schema_for!(SourceOutput)).unwrap();
        assert!(!schema.contains("relation_scope"));
        let restored: SourceOutput = serde_json::from_slice(&bytes).unwrap();
        assert!(restored.relation_scope.is_none());
        assert_eq!(restored.schema, snapshot.schema);
        catalog.source_outputs[0] = restored;
        assert!(parse(&format!("query:retained {native}:x"), &catalog).is_err());
        let missing = parse(&format!("query:absent {native}:x"), &catalog).unwrap();
        assert_eq!(
            bind(&missing, &catalog).unwrap_err().kind,
            ErrorKind::BadValue
        );
    }
}
#[test]
fn both_schema_values_are_validated_without_changing_the_canonical_row_domain() {
    for Case {
        mut catalog,
        unit,
        column,
        qualifier,
        ..
    } in cases()
    {
        install(&mut catalog, unit, column, qualifier);
        let syntax = parse("query:retained", &catalog).unwrap();
        let valid = catalog.clone();
        let scope = catalog.source_outputs[0].relation_scope.as_mut().unwrap();
        scope.fields.push(scope.fields[0].clone());
        assert_eq!(
            bind(&syntax, &catalog).unwrap_err().kind,
            ErrorKind::BadValue
        );
        catalog = valid.clone();
        catalog.source_outputs[0]
            .relation_scope
            .as_mut()
            .unwrap()
            .unit = None;
        assert_eq!(
            bind(&syntax, &catalog).unwrap_err().kind,
            ErrorKind::BadValue
        );
        catalog = valid;
        let scope = catalog.source_outputs[0].relation_scope.as_mut().unwrap();
        scope.main_time = Some("not_a_field".into());
        assert_eq!(
            bind(&syntax, &catalog).unwrap_err().kind,
            ErrorKind::BadValue
        );
    }
}
