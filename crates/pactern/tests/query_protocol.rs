//! Structural schema smoke coverage without introducing a JSON Schema runtime
//! dependency. This deliberately does not claim a general Draft 2020-12 validator.
use std::collections::BTreeMap;

use pactern::editor::EditorRequest;
use pactern::semantic::{IdentityLedger, QueryRequest, SemanticIndex};
use pactern::workspace::source_snapshot;
use serde_json::{Value, json};

fn schema(name: &str) -> Value {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/spec/0.1.0/schemas");
    serde_json::from_slice(&std::fs::read(root.join(format!("{name}.schema.json"))).unwrap())
        .unwrap()
}

fn capture(name: &str, schema: &str, document: &Value) {
    if let Some(directory) = std::env::var_os("PACTERN_QUERY_CAPTURE_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&json!({"schema":schema,"document":document})).unwrap(),
        )
        .unwrap();
    }
}

fn structure(value: &Value, node: &Value, root: &Value) -> bool {
    if let Some(reference) = node["$ref"].as_str() {
        return structure(
            value,
            root.pointer(&reference[1..])
                .expect("local schema reference"),
            root,
        );
    }
    if let Some(constant) = node.get("const")
        && value != constant
    {
        return false;
    }
    if let Some(variants) = node["enum"].as_array()
        && !variants.contains(value)
    {
        return false;
    }
    if let Some(variants) = node["anyOf"].as_array()
        && !variants.iter().any(|branch| structure(value, branch, root))
    {
        return false;
    }
    if let Some(variants) = node["oneOf"].as_array()
        && variants
            .iter()
            .filter(|branch| structure(value, branch, root))
            .count()
            != 1
    {
        return false;
    }
    match node["type"].as_str() {
        Some("object") => {
            let Some(object) = value.as_object() else {
                return false;
            };
            if node["required"].as_array().is_some_and(|required| {
                required
                    .iter()
                    .any(|key| !object.contains_key(key.as_str().unwrap()))
            }) {
                return false;
            }
            for (key, value) in object {
                if let Some(property) = node["properties"].get(key) {
                    if !structure(value, property, root) {
                        return false;
                    }
                } else if node["additionalProperties"] == false {
                    return false;
                }
            }
        }
        Some("array") => {
            let Some(values) = value.as_array() else {
                return false;
            };
            if node["minItems"]
                .as_u64()
                .is_some_and(|minimum| values.len() < minimum as usize)
            {
                return false;
            }
            if node["uniqueItems"] == true
                && values
                    .iter()
                    .enumerate()
                    .any(|(index, value)| values[..index].contains(value))
            {
                return false;
            }
            if values
                .iter()
                .any(|value| !structure(value, &node["items"], root))
            {
                return false;
            }
        }
        Some("string") => {
            let Some(text) = value.as_str() else {
                return false;
            };
            if node["minLength"]
                .as_u64()
                .is_some_and(|minimum| text.chars().count() < minimum as usize)
                || node["maxLength"]
                    .as_u64()
                    .is_some_and(|maximum| text.chars().count() > maximum as usize)
            {
                return false;
            }
        }
        Some("integer") => {
            let Some(number) = value.as_u64() else {
                return false;
            };
            if node["minimum"]
                .as_u64()
                .is_some_and(|minimum| number < minimum)
                || node["maximum"]
                    .as_u64()
                    .is_some_and(|maximum| number > maximum)
            {
                return false;
            }
        }
        Some("boolean") if !value.is_boolean() => return false,
        Some("null") if !value.is_null() => return false,
        _ => {}
    }
    true
}

#[test]
fn every_semantic_query_request_and_output_matches_schema_structure() {
    let files = BTreeMap::from([(
        "src/main.ptn".into(),
        b"module main; fn helper()->I64{return 1;} pub fn run()->I64{return helper();}".to_vec(),
    )]);
    let source = source_snapshot(&files).unwrap();
    let index = SemanticIndex::build(&source, &IdentityLedger::default()).unwrap();
    let request_schema = schema("semantic-query-request");
    let response_schema = schema("semantic-query-response");
    for query in [
        json!({"operation":"inspect","selector":"run"}),
        json!({"operation":"callers","selector":"helper"}),
        json!({"operation":"impact","selectors":["helper"]}),
        json!({"operation":"affected_tests","selectors":["helper"],"tests":[{"test":"suite","declarations":[],"complete":false}]}),
    ] {
        let operation = query["operation"].as_str().unwrap().to_owned();
        let request = json!({"protocol_version":"0.1.0","kind":"query_request","source_revision":source.revision(),"query":query});
        capture(
            &format!("semantic-{operation}-request"),
            "semantic-query-request",
            &request,
        );
        assert!(structure(&request, &request_schema, &request_schema));
        let decoded = QueryRequest::decode(&serde_json::to_vec(&request).unwrap()).unwrap();
        let response = serde_json::to_value(decoded.execute(&index).unwrap()).unwrap();
        capture(
            &format!("semantic-{operation}-response"),
            "semantic-query-response",
            &response,
        );
        assert!(
            structure(&response, &response_schema, &response_schema),
            "{response}"
        );
        let mut extra = response.clone();
        extra["unrecognized"] = true.into();
        assert!(!structure(&extra, &response_schema, &response_schema));
        let mut malformed = response;
        malformed["result"] = 42.into();
        assert!(!structure(&malformed, &response_schema, &response_schema));
        let mut stale = decoded;
        stale.source_revision = format!("sha256:{}", "0".repeat(64));
        assert_eq!(
            stale.execute(&index).unwrap_err().code,
            "PTN_CHANGE_STALE_REVISION"
        );
    }
}

#[test]
fn editor_diagnostics_definition_and_absent_definition_match_schema() {
    let files = BTreeMap::from([(
        "src/main.ptn".into(),
        b"module main; fn run()->I64{return true;}".to_vec(),
    )]);
    let source = source_snapshot(&files).unwrap();
    let index = SemanticIndex::build(&source, &IdentityLedger::default()).unwrap();
    let request_schema = schema("editor-request");
    let response_schema = schema("editor-response");
    for query in [
        json!({"operation":"diagnostics"}),
        json!({"operation":"definition","path":"src/main.ptn","byte":15}),
        json!({"operation":"definition","path":"src/main.ptn","byte":0}),
    ] {
        let operation = format!(
            "{}-{}",
            query["operation"].as_str().unwrap(),
            query.get("byte").map_or(0, |byte| byte.as_u64().unwrap())
        );
        let request = json!({"protocol_version":"0.1.0","kind":"editor_request","source_revision":source.revision(),"query":query});
        capture(
            &format!("editor-{operation}-request"),
            "editor-request",
            &request,
        );
        assert!(structure(&request, &request_schema, &request_schema));
        let decoded = EditorRequest::decode(&serde_json::to_vec(&request).unwrap()).unwrap();
        let response = decoded.execute(&source, &index).unwrap();
        capture(
            &format!("editor-{operation}-response"),
            "editor-response",
            &response,
        );
        assert!(
            structure(&response, &response_schema, &response_schema),
            "{response}"
        );
    }
}

#[test]
fn query_decoders_reject_unknown_fields_duplicate_keys_and_invalid_shapes() {
    let revision = format!("sha256:{}", "a".repeat(64));
    let good = json!({"protocol_version":"0.1.0","kind":"query_request","source_revision":revision,"query":{"operation":"inspect","selector":"run"}});
    let mut unknown = good.clone();
    unknown["query"]["approved"] = true.into();
    assert!(QueryRequest::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
    let duplicate = serde_json::to_string(&good).unwrap().replacen(
        "\"selector\":",
        "\"selector\":\"other\",\"selector\":",
        1,
    );
    assert!(QueryRequest::decode(duplicate.as_bytes()).is_err());
    let mut empty = good;
    empty["query"] = json!({"operation":"impact","selectors":[]});
    assert!(QueryRequest::decode(&serde_json::to_vec(&empty).unwrap()).is_err());
    let bad = json!({"protocol_version":"0.1.0","kind":"editor_request","source_revision":revision,"query":{"operation":"definition","path":"../outside","byte":0}});
    assert!(EditorRequest::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
}
