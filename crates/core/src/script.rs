//! Declarative scripts: pre-request variable assignment, post-response
//! extraction and assertions. No code execution.

use std::time::Duration;

use md5::Md5;
use serde_json::Value;
use serde_json_path::JsonPath;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

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
    /// The measured time phases by name (`total`, `ttfb`, ...). A phase the
    /// protocol does not measure is not in the list.
    pub timing: &'a [(&'a str, Duration)],
}

/// The time phases the `time` source knows.
pub const TIME_PHASES: [&str; 6] = ["total", "ttfb", "dns", "connect", "tls", "download"];

/// Approximate size of a header block in bytes: `name: value\r\n` per header.
pub fn headers_size(headers: &[(String, String)]) -> usize {
    headers.iter().map(|(n, v)| n.len() + v.len() + 4).sum()
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
    query_parsed(&parse_body(body), expr)
}

/// The body as JSON, or why it is not.
fn parse_body(body: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(body).map_err(|e| format!("response body is not JSON: {e}"))
}

fn query_parsed(body: &Result<Value, String>, expr: &str) -> Result<Vec<Value>, String> {
    let path = JsonPath::parse(expr.trim()).map_err(|e| format!("invalid JSONPath: {e}"))?;
    let v = body.as_ref().map_err(Clone::clone)?;
    Ok(path.query(v).all().into_iter().cloned().collect())
}

/// The response body parsed once for all the rules of a run.
#[derive(Default)]
struct Parsed {
    json: std::cell::OnceCell<Result<Value, String>>,
    xml: crate::xpath::Xml,
}

/// String form of a JSON value: strings unquoted, everything else as compact JSON.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn read(
    source: Source,
    expr: &str,
    resp: &ResponseView<'_>,
    parsed: &Parsed,
) -> Result<Option<String>, String> {
    let expr = expr.trim();
    match source {
        Source::Status => Ok(Some(resp.status.to_string())),
        Source::Header => Ok(resp
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(expr))
            .map(|(_, v)| v.clone())),
        Source::Body => {
            let nodes = query_parsed(parsed.json.get_or_init(|| parse_body(resp.body)), expr)?;
            Ok(match nodes.len() {
                0 => None,
                1 => Some(value_text(&nodes[0])),
                _ => Some(Value::Array(nodes).to_string()),
            })
        }
        Source::Time => {
            let phase = if expr.is_empty() { "total" } else { expr };
            if !TIME_PHASES.contains(&phase) {
                return Err(format!(
                    "unknown time phase '{phase}' ({})",
                    TIME_PHASES.join(", ")
                ));
            }
            Ok(resp
                .timing
                .iter()
                .find(|(n, _)| *n == phase)
                .map(|(_, d)| millis(*d)))
        }
        Source::Size => {
            let body = resp.body.len();
            let headers = headers_size(resp.headers);
            let n = match expr {
                "" | "total" => body + headers,
                "body" => body,
                "headers" => headers,
                other => {
                    return Err(format!(
                        "unknown size part '{other}' (total, body, headers)"
                    ));
                }
            };
            Ok(Some(n.to_string()))
        }
        Source::Hash => digest(expr, resp.body).map(Some),
        Source::XPath => {
            let items = parsed.xml.query(resp.body, expr)?;
            Ok(match items.len() {
                0 => None,
                1 => items.into_iter().next(),
                _ => Some(Value::from(items).to_string()),
            })
        }
        Source::Regex => {
            let re = regex::Regex::new(expr).map_err(|e| format!("invalid regex: {e}"))?;
            let text = String::from_utf8_lossy(resp.body);
            Ok(re.captures(&text).and_then(|c| {
                c.get(1)
                    .or_else(|| c.get(0))
                    .map(|m| m.as_str().to_string())
            }))
        }
        Source::Cookie => Ok(resp
            .headers
            .iter()
            .rev()
            .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
            .find_map(|(_, v)| {
                let pair = v.split(';').next().unwrap_or_default();
                let (name, value) = pair.split_once('=')?;
                (name.trim() == expr).then(|| value.trim().to_string())
            })),
    }
}

/// A duration in milliseconds, with at most three decimals.
fn millis(d: Duration) -> String {
    let s = format!("{:.3}", d.as_secs_f64() * 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The lowercase hex digest of `bytes` with the hash `algo`.
pub(crate) fn digest(algo: &str, bytes: &[u8]) -> Result<String, String> {
    Ok(match algo {
        "md5" => hex(&Md5::digest(bytes)),
        "sha1" => hex(&Sha1::digest(bytes)),
        "sha256" => hex(&Sha256::digest(bytes)),
        "sha512" => hex(&Sha512::digest(bytes)),
        other => {
            return Err(format!(
                "unknown hash '{other}' (md5, sha1, sha256, sha512)"
            ));
        }
    })
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Runs extraction rules and stores successful results in the runtime layer.
pub fn run_extract(
    rules: &[Extract],
    resp: &ResponseView<'_>,
    scope: &mut Scope,
) -> Vec<ExtractResult> {
    let mut out = Vec::new();
    let parsed = Parsed::default();
    for r in rules.iter().filter(|r| !r.var.trim().is_empty()) {
        let value = match read(r.source, &r.expr, resp, &parsed) {
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
        Source::Time | Source::Size => {
            let part = a.expr.trim();
            let part = if part.is_empty() { "total" } else { part };
            format!("{} {part}", a.source.as_str())
        }
        Source::Hash => a.expr.trim().to_string(),
        Source::XPath | Source::Regex | Source::Cookie => {
            format!("{} {}", a.source.as_str(), a.expr.trim())
        }
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
    let parsed = Parsed::default();
    asserts
        .iter()
        .map(|a| {
            let label = describe(a);
            let expected = scope.render(&a.value).text;
            match read(a.source, &a.expr, resp, &parsed) {
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
            timing: &[],
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

    #[test]
    fn time_size_hash_xpath_regex_and_cookie_sources() {
        let headers = vec![
            ("Set-Cookie".to_string(), "sid=old; Path=/".to_string()),
            ("set-cookie".to_string(), "sid=abc; HttpOnly".to_string()),
            ("Set-Cookie".to_string(), "theme=dark".to_string()),
        ];
        let body = b"<r><item id=\"1\">a</item><item id=\"2\">b</item></r> token=xyz";
        let timing = [
            ("total", Duration::from_micros(12_500)),
            ("ttfb", Duration::from_millis(7)),
        ];
        let view = ResponseView {
            status: 200,
            headers: &headers,
            body,
            timing: &timing,
        };
        let read = |source, expr: &str| read(source, expr, &view, &Parsed::default());
        assert_eq!(read(Source::Time, ""), Ok(Some("12.5".into())));
        assert_eq!(read(Source::Time, "ttfb"), Ok(Some("7".into())));
        assert_eq!(read(Source::Time, "tls"), Ok(None));
        assert!(read(Source::Time, "lag").unwrap_err().contains("ttfb"));
        let hs = headers_size(&headers);
        assert_eq!(read(Source::Size, "body"), Ok(Some(body.len().to_string())));
        assert_eq!(read(Source::Size, "headers"), Ok(Some(hs.to_string())));
        assert_eq!(
            read(Source::Size, ""),
            Ok(Some((body.len() + hs).to_string()))
        );
        assert!(read(Source::Size, "wire").is_err());
        let empty = ResponseView { body: b"", ..view };
        let hash = |algo| super::read(Source::Hash, algo, &empty, &Parsed::default());
        assert_eq!(
            hash("md5"),
            Ok(Some("d41d8cd98f00b204e9800998ecf8427e".into()))
        );
        assert_eq!(
            hash("sha1"),
            Ok(Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".into()))
        );
        assert_eq!(
            hash("sha256"),
            Ok(Some(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into()
            ))
        );
        assert_eq!(hash("sha512").map(|h| h.map(|h| h.len())), Ok(Some(128)));
        assert!(read(Source::Hash, "crc32").is_err());
        // The body is not XML as a whole: XPath reports it.
        assert!(read(Source::XPath, "//item").is_err());
        let xml = ResponseView {
            body: b"<r><item id=\"1\">a</item><item id=\"2\">b</item></r>",
            ..view
        };
        let xpath = |expr| super::read(Source::XPath, expr, &xml, &Parsed::default());
        assert_eq!(xpath("//item[@id='2']"), Ok(Some("b".into())));
        assert_eq!(xpath("//item"), Ok(Some(r#"["a","b"]"#.into())));
        assert_eq!(xpath("//none"), Ok(None));
        assert_eq!(read(Source::Regex, r"token=(\w+)"), Ok(Some("xyz".into())));
        assert_eq!(read(Source::Regex, r"id=.\d"), Ok(Some("id=\"1".into())));
        assert_eq!(read(Source::Regex, "nothing"), Ok(None));
        assert!(read(Source::Regex, "(").is_err());
        assert_eq!(read(Source::Cookie, "sid"), Ok(Some("abc".into())));
        assert_eq!(read(Source::Cookie, "theme"), Ok(Some("dark".into())));
        assert_eq!(read(Source::Cookie, "SID"), Ok(None));
    }

    #[test]
    fn labels_of_the_new_sources() {
        let a = |source, expr: &str, op| Assertion {
            source,
            expr: expr.into(),
            op,
            value: "1".into(),
        };
        assert_eq!(
            describe(&a(Source::Time, "", AssertOp::Lt)),
            "time total < 1"
        );
        assert_eq!(
            describe(&a(Source::Size, "body", AssertOp::Le)),
            "size body <= 1"
        );
        assert_eq!(
            describe(&a(Source::Hash, "sha256", AssertOp::Eq)),
            "sha256 == 1"
        );
        assert_eq!(
            describe(&a(Source::Cookie, "sid", AssertOp::Exists)),
            "cookie sid exists"
        );
    }
}
