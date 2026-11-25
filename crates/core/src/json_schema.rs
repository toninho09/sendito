//! A sample value from a JSON Schema: the MCP argument template and the
//! OpenAPI request bodies.

use serde_json::{Map, Value, json};

/// How deep a sample goes into nested objects (and `$ref` chains).
const MAX_DEPTH: usize = 8;

/// A sample value for `schema`: its `example`, `default` or first `enum`
/// value, else an empty value of its type (objects with every property,
/// arrays empty). `resolve` turns a `$ref` into its schema (`None`: unknown).
/// A `$ref` inside its own schema gives `null`, so a recursive schema does
/// not grow the sample.
pub fn sample(schema: &Value, resolve: &dyn Fn(&str) -> Option<Value>) -> Value {
    sample_at(schema, resolve, 0, &mut Vec::new())
}

/// `refs` holds the `$ref`s that lead to `schema`.
fn sample_at(
    schema: &Value,
    resolve: &dyn Fn(&str) -> Option<Value>,
    depth: usize,
    refs: &mut Vec<String>,
) -> Value {
    if depth >= MAX_DEPTH {
        return Value::Null;
    }
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        if refs.iter().any(|seen| seen == r) {
            return Value::Null;
        }
        let Some(s) = resolve(r) else {
            return Value::Null;
        };
        refs.push(r.to_string());
        let v = sample_at(&s, resolve, depth + 1, refs);
        refs.pop();
        return v;
    }
    if let Some(value) = schema
        .get("example")
        .or_else(|| schema.get("default"))
        .or_else(|| {
            schema
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|v| v.first())
        })
    {
        return value.clone();
    }
    // allOf merges its objects; oneOf / anyOf take the first choice.
    if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
        let mut out = Map::new();
        for part in all {
            if let Value::Object(o) = sample_at(part, resolve, depth + 1, refs) {
                out.extend(o);
            }
        }
        return Value::Object(out);
    }
    if let Some(first) = ["oneOf", "anyOf"].iter().find_map(|k| {
        schema
            .get(*k)
            .and_then(Value::as_array)
            .and_then(|v| v.first())
    }) {
        return sample_at(first, resolve, depth + 1, refs);
    }
    let kind = match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        // JSON Schema 2020-12 (OpenAPI 3.1): ["string", "null"].
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        _ if schema.get("properties").is_some() => Some("object"),
        _ => None,
    };
    match kind {
        Some("object") => {
            let mut out = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (name, value) in properties {
                    out.insert(name.clone(), sample_at(value, resolve, depth + 1, refs));
                }
            }
            Value::Object(out)
        }
        Some("array") => json!([]),
        Some("string") => json!(""),
        Some("integer" | "number") => json!(0),
        Some("boolean") => json!(false),
        _ => Value::Null,
    }
}

/// The sample of an object schema as pretty JSON text; `{}` for any other
/// schema (the MCP argument template).
pub fn object_template(schema: &Value) -> String {
    let value = sample(schema, &|_| None);
    if value.is_object() {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
    } else {
        "{}".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_follow_the_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer", "default": 7},
                "kind": {"enum": ["a", "b"]},
                "tags": {"type": "array"},
                "note": {"type": ["string", "null"]},
                "owner": {"$ref": "#/components/schemas/User"},
                "both": {"allOf": [{"properties": {"x": {"example": 1}}}, {"properties": {"y": {"type": "boolean"}}}]},
                "either": {"oneOf": [{"type": "number"}, {"type": "string"}]}
            }
        });
        let user = json!({"properties": {"id": {"type": "integer", "example": 42}}});
        let v = sample(&schema, &|r| {
            (r == "#/components/schemas/User").then(|| user.clone())
        });
        assert_eq!(
            v,
            json!({
                "name": "", "age": 7, "kind": "a", "tags": [], "note": "",
                "owner": {"id": 42}, "both": {"x": 1, "y": false}, "either": 0
            })
        );
        assert_eq!(object_template(&json!({"type": "string"})), "{}");
    }

    #[test]
    fn a_ref_cycle_stops() {
        let node = json!({"properties": {"next": {"$ref": "#/n"}}});
        let v = sample(&node, &|_| Some(node.clone()));
        assert!(v.to_string().len() < 200, "{v}");
        // Many fields that refer to their own schema: the sample stays small.
        let person = json!({"properties": {
            "a": {"$ref": "#/p"}, "b": {"$ref": "#/p"}, "c": {"$ref": "#/p"},
            "d": {"$ref": "#/p"}, "e": {"$ref": "#/p"}, "f": {"$ref": "#/p"}
        }});
        let v = sample(&json!({"$ref": "#/p"}), &|_| Some(person.clone()));
        assert_eq!(
            v,
            json!({"a": null, "b": null, "c": null, "d": null, "e": null, "f": null})
        );
    }
}
