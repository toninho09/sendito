//! Declarative scripts: pre-request variable assignment, post-response
//! extraction and assertions. No code execution.

use serde_json::Value;
use serde_json_path::JsonPath;

use crate::model::{AssertOp, Assertion, Extract, SetVar, Source};
use crate::vars::{Scope, VarValue};

/// Name of the scope layer that holds variables set by scripts.
pub const RUNTIME_LAYER: &str = "runtime";

/// The parts of a response that scripts can read.
#[derive(Debug, Clone, Copy)]
pub struct ResponseView<'a> {
    pub status: u16,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

/// Runs pre-request steps in order. Each assignment is visible to the next.
/// Returns the assignments made. A value is secret if a secret contributed to it.
pub fn run_pre(steps: &[SetVar], scope: &mut Scope) -> Vec<(String, VarValue)> {
    let mut out = Vec::new();
    for s in steps.iter().filter(|s| !s.var.trim().is_empty()) {
        let r = scope.render(&s.value);
        let v = VarValue {
            value: r.text,
            secret: r.secret,
        };
        scope.set(RUNTIME_LAYER, s.var.trim(), v.clone());
        out.push((s.var.trim().to_string(), v));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractResult {
    pub var: String,
    pub value: Result<VarValue, String>,
}

/// Evaluates a JSONPath against a body and returns the matched nodes.
pub fn json_query(body: &[u8], expr: &str) -> Result<Vec<Value>, String> {
    let path = JsonPath::parse(expr.trim()).map_err(|e| format!("invalid JSONPath: {e}"))?;
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("response body is not JSON: {e}"))?;
    Ok(path.query(&v).all().into_iter().cloned().collect())
}

/// String form of a JSON value: strings unquoted, everything else as compact JSON.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn read(source: Source, expr: &str, resp: &ResponseView<'_>) -> Result<Option<String>, String> {
    match source {
        Source::Status => Ok(Some(resp.status.to_string())),
        Source::Header => Ok(resp
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(expr.trim()))
            .map(|(_, v)| v.clone())),
        Source::Body => {
            let nodes = json_query(resp.body, expr)?;
            Ok(match nodes.len() {
                0 => None,
                1 => Some(value_text(&nodes[0])),
                _ => Some(Value::Array(nodes).to_string()),
            })
        }
    }
}

/// Runs extraction rules and stores successful results in the runtime layer.
pub fn run_extract(
    rules: &[Extract],
    resp: &ResponseView<'_>,
    scope: &mut Scope,
) -> Vec<ExtractResult> {
    let mut out = Vec::new();
    for r in rules.iter().filter(|r| !r.var.trim().is_empty()) {
        let value = match read(r.source, &r.expr, resp) {
            Ok(Some(v)) => Ok(VarValue {
                value: v,
                secret: r.secret,
            }),
            Ok(None) => Err(format!("no match for {} '{}'", r.source.as_str(), r.expr)),
            Err(e) => Err(e),
        };
        if let Ok(v) = &value {
            scope.set(RUNTIME_LAYER, r.var.trim(), v.clone());
        }
        out.push(ExtractResult {
            var: r.var.trim().to_string(),
            value,
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertResult {
    /// Human-readable description, e.g. `status == 200`.
    pub label: String,
    pub pass: bool,
    /// Actual value (or error) for display.
    pub actual: String,
}

pub fn describe(a: &Assertion) -> String {
    let subject = match a.source {
        Source::Status => "status".to_string(),
        Source::Header => format!("header {}", a.expr.trim()),
        Source::Body => a.expr.trim().to_string(),
    };
    if a.op.takes_value() {
        format!("{subject} {} {}", a.op.symbol(), a.value)
    } else {
        format!("{subject} {}", a.op.symbol())
    }
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

fn compare(op: AssertOp, actual: Option<&str>, expected: &str) -> Result<bool, String> {
    let Some(actual) = actual else {
        return Ok(matches!(
            op,
            AssertOp::NotExists | AssertOp::Ne | AssertOp::NotContains
        ));
    };
    Ok(match op {
        AssertOp::Exists => true,
        AssertOp::NotExists => false,
        AssertOp::Eq => match (num(actual), num(expected)) {
            (Some(a), Some(b)) => a == b,
            _ => actual == expected,
        },
        AssertOp::Ne => match (num(actual), num(expected)) {
            (Some(a), Some(b)) => a != b,
            _ => actual != expected,
        },
        AssertOp::Contains => actual.contains(expected),
        AssertOp::NotContains => !actual.contains(expected),
        AssertOp::Lt | AssertOp::Le | AssertOp::Gt | AssertOp::Ge => {
            let (Some(a), Some(b)) = (num(actual), num(expected)) else {
                return Err(format!(
                    "cannot compare '{actual}' and '{expected}' as numbers"
                ));
            };
            match op {
                AssertOp::Lt => a < b,
                AssertOp::Le => a <= b,
                AssertOp::Gt => a > b,
                _ => a >= b,
            }
        }
        AssertOp::Matches => regex::Regex::new(expected)
            .map_err(|e| format!("invalid regex: {e}"))?
            .is_match(actual),
    })
}

/// Evaluates assertions. Expected values may reference variables.
pub fn run_asserts(
    asserts: &[Assertion],
    resp: &ResponseView<'_>,
    scope: &Scope,
) -> Vec<AssertResult> {
    asserts
        .iter()
        .map(|a| {
            let label = describe(a);
            let expected = scope.render(&a.value).text;
            match read(a.source, &a.expr, resp) {
                Err(e) => AssertResult {
                    label,
                    pass: false,
                    actual: e,
                },
                Ok(actual) => match compare(a.op, actual.as_deref(), &expected) {
                    Ok(pass) => AssertResult {
                        label,
                        pass,
                        actual: actual.unwrap_or_else(|| "(missing)".into()),
                    },
                    Err(e) => AssertResult {
                        label,
                        pass: false,
                        actual: e,
                    },
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn resp<'a>(headers: &'a [(String, String)], body: &'a [u8]) -> ResponseView<'a> {
        ResponseView {
            status: 201,
            headers,
            body,
        }
    }

    #[test]
    fn pre_sets_in_order() {
        let mut m = BTreeMap::new();
        m.insert("tok".into(), VarValue::secret("s"));
        let mut scope = Scope::new().with_layer("env", m);
        let out = run_pre(
            &[
                SetVar {
                    var: "a".into(),
                    value: "1".into(),
                },
                SetVar {
                    var: "b".into(),
                    value: "{{a}}-{{tok}}".into(),
                },
                SetVar {
                    var: " ".into(),
                    value: "x".into(),
                },
            ],
            &mut scope,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].1, VarValue::secret("1-s"));
        assert_eq!(scope.render("{{b}}").text, "1-s");
    }

    #[test]
    fn extract_rules() {
        let headers = vec![("X-Id".to_string(), "42".to_string())];
        let body = br#"{"token":"abc","items":[{"id":1},{"id":2}],"n":3.5}"#;
        let mut scope = Scope::new();
        let rules = vec![
            Extract {
                var: "t".into(),
                source: Source::Body,
                expr: "$.token".into(),
                secret: true,
            },
            Extract {
                var: "ids".into(),
                source: Source::Body,
                expr: "$.items[*].id".into(),
                secret: false,
            },
            Extract {
                var: "h".into(),
                source: Source::Header,
                expr: "x-id".into(),
                secret: false,
            },
            Extract {
                var: "s".into(),
                source: Source::Status,
                expr: String::new(),
                secret: false,
            },
            Extract {
                var: "m".into(),
                source: Source::Body,
                expr: "$.missing".into(),
                secret: false,
            },
            Extract {
                var: "bad".into(),
                source: Source::Body,
                expr: "$[".into(),
                secret: false,
            },
        ];
        let out = run_extract(&rules, &resp(&headers, body), &mut scope);
        assert_eq!(out[0].value, Ok(VarValue::secret("abc")));
        assert_eq!(out[1].value, Ok(VarValue::plain("[1,2]")));
        assert_eq!(out[2].value, Ok(VarValue::plain("42")));
        assert_eq!(out[3].value, Ok(VarValue::plain("201")));
        assert!(out[4].value.is_err());
        assert!(out[5].value.is_err());
        assert_eq!(scope.render("{{t}}{{h}}").text, "abc42");
        let nonjson = run_extract(&rules[..1], &resp(&headers, b"<html>"), &mut Scope::new());
        assert!(nonjson[0].value.is_err());
    }

    #[test]
    fn assertions() {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        let body = br#"{"n":3.5,"name":"ana","ok":true}"#;
        let mut m = BTreeMap::new();
        m.insert("expected".into(), VarValue::plain("ana"));
        let scope = Scope::new().with_layer("env", m);
        let a = |source, expr: &str, op, value: &str| Assertion {
            source,
            expr: expr.into(),
            op,
            value: value.into(),
        };
        let asserts = vec![
            a(Source::Status, "", AssertOp::Eq, "201"),
            a(Source::Status, "", AssertOp::Lt, "300"),
            a(Source::Status, "", AssertOp::Ge, "201"),
            a(Source::Header, "content-type", AssertOp::Contains, "json"),
            a(Source::Header, "x-none", AssertOp::NotExists, ""),
            a(Source::Body, "$.name", AssertOp::Eq, "{{expected}}"),
            a(Source::Body, "$.n", AssertOp::Gt, "3"),
            a(Source::Body, "$.n", AssertOp::Le, "3.5"),
            a(Source::Body, "$.ok", AssertOp::Eq, "true"),
            a(Source::Body, "$.name", AssertOp::Matches, "^a.a$"),
            a(Source::Body, "$.name", AssertOp::Ne, "bob"),
            a(Source::Body, "$.name", AssertOp::NotContains, "z"),
            a(Source::Body, "$.n", AssertOp::Exists, ""),
            a(Source::Body, "$.n", AssertOp::Ne, "4"),
        ];
        let res = run_asserts(&asserts, &resp(&headers, body), &scope);
        for r in &res {
            assert!(r.pass, "{} failed: {}", r.label, r.actual);
        }
        let failing = vec![
            a(Source::Status, "", AssertOp::Eq, "200"),
            a(Source::Body, "$.name", AssertOp::Gt, "1"),
            a(Source::Body, "$.name", AssertOp::Matches, "("),
            a(Source::Body, "$.zzz", AssertOp::Exists, ""),
            a(Source::Body, "$.n", AssertOp::NotExists, ""),
            a(Source::Body, "$[", AssertOp::Eq, ""),
        ];
        let res = run_asserts(&failing, &resp(&headers, body), &scope);
        assert!(res.iter().all(|r| !r.pass));
        assert_eq!(res[0].label, "status == 200");
        assert_eq!(res[3].label, "$.zzz exists");
        assert_eq!(res[3].actual, "(missing)");
    }
}
