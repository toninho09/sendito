//! The report of a folder run, and its writers: JSON, JUnit XML, TAP and HTML.

use sendito_core::script::AssertResult;

/// One request's line in a folder run report.
#[derive(Debug, Clone, Default)]
pub(crate) struct Entry {
    /// The request path relative to the collection (`Users/list.toml`).
    pub(crate) path: String,
    /// `http`, `grpc`, …; `None` when the file did not load.
    pub(crate) kind: Option<&'static str>,
    /// `pass`, `fail` or `error`.
    pub(crate) outcome: &'static str,
    /// `200 OK`, `OK`, …; `None` when the request never ran.
    pub(crate) status: Option<String>,
    pub(crate) duration_ms: Option<u128>,
    /// The assertions and the Lua `test` results, secrets masked.
    pub(crate) asserts: Vec<AssertResult>,
    /// `None` for a file that did not load (the JSON report leaves the key out).
    pub(crate) unresolved: Option<bool>,
    pub(crate) error: Option<String>,
    pub(crate) events: Option<u64>,
    pub(crate) messages: Option<u64>,
    /// How many times the request was sent (0 when it never ran).
    pub(crate) attempts: u32,
}

impl Entry {
    fn failed_asserts(&self) -> impl Iterator<Item = &AssertResult> {
        self.asserts.iter().filter(|a| !a.pass)
    }

    /// The lines that explain a `fail` or an `error`.
    fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .failed_asserts()
            .map(|a| format!("✗ {}  (actual: {})", a.label, a.actual))
            .collect();
        if let Some(e) = &self.error {
            out.push(e.clone());
        } else if out.is_empty() && self.outcome != "pass" {
            out.push(self.status.clone().unwrap_or_default());
        }
        out
    }

    /// The folder part of the path (`.` at the collection root).
    fn class(&self) -> &str {
        match self.path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => ".",
        }
    }
}

/// The result of a folder run.
#[derive(Debug, Clone, Default)]
pub(crate) struct SuiteReport {
    pub(crate) environment: Option<String>,
    /// The folder argument (`.` for the collection root).
    pub(crate) folder: String,
    pub(crate) duration_ms: u128,
    pub(crate) entries: Vec<Entry>,
}

impl SuiteReport {
    fn count(&self, outcome: &str) -> usize {
        self.entries.iter().filter(|e| e.outcome == outcome).count()
    }

    pub(crate) fn passed(&self) -> usize {
        self.count("pass")
    }

    pub(crate) fn failed(&self) -> usize {
        self.count("fail")
    }

    /// Every `error` entry, unresolved variables included.
    pub(crate) fn errors(&self) -> usize {
        self.count("error")
    }

    pub(crate) fn unresolved(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.unresolved == Some(true))
            .count()
    }

    /// The JSON report (`docs/CLI.md`, "The JSON report").
    pub(crate) fn json(&self) -> String {
        let requests: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|e| {
                let mut v = serde_json::json!({
                    "path": e.path,
                    "kind": e.kind,
                    "outcome": e.outcome,
                    "status": e.status,
                    "duration_ms": e.duration_ms,
                    "asserts": e.asserts.len(),
                    "failed_asserts": e.failed_asserts().count(),
                    "attempts": e.attempts,
                });
                if let Some(u) = e.unresolved {
                    v["unresolved"] = u.into();
                }
                v["error"] = e.error.clone().into();
                if let Some(n) = e.events {
                    v["events"] = n.into();
                }
                if let Some(n) = e.messages {
                    v["messages"] = n.into();
                }
                v
            })
            .collect();
        let out = serde_json::json!({
            "environment": self.environment,
            "folder": self.folder,
            "total": self.entries.len(),
            "passed": self.passed(),
            "failed": self.failed(),
            "errors": self.errors(),
            "unresolved": self.unresolved(),
            "duration_ms": self.duration_ms,
            "requests": requests,
        });
        serde_json::to_string_pretty(&out).unwrap_or_default()
    }

    /// JUnit XML: one `<testsuite>` for the folder, one `<testcase>` per
    /// request, `<failure>` for `fail` and `<error>` for `error`.
    pub(crate) fn junit(&self) -> String {
        let counts = format!(
            "tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{}\"",
            self.entries.len(),
            self.failed(),
            self.errors(),
            secs(Some(self.duration_ms))
        );
        let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        x.push_str(&format!("<testsuites name=\"sendito\" {counts}>\n"));
        x.push_str(&format!(
            "  <testsuite name=\"{}\" {counts}>\n",
            xml(&self.folder)
        ));
        if let Some(env) = &self.environment {
            x.push_str(&format!(
                "    <properties>\n      <property name=\"environment\" value=\"{}\"/>\n    </properties>\n",
                xml(env)
            ));
        }
        for e in &self.entries {
            let open = format!(
                "    <testcase classname=\"{}\" name=\"{}\" time=\"{}\"",
                xml(e.class()),
                xml(&e.path),
                secs(e.duration_ms)
            );
            let tag = match e.outcome {
                "pass" => {
                    x.push_str(&open);
                    x.push_str("/>\n");
                    continue;
                }
                "fail" => "failure",
                _ => "error",
            };
            let problems = e.problems();
            let message = problems.first().cloned().unwrap_or_default();
            x.push_str(&open);
            x.push_str(">\n");
            x.push_str(&format!(
                "      <{tag} message=\"{}\">{}</{tag}>\n",
                xml(&message),
                xml(&problems.join("\n"))
            ));
            x.push_str("    </testcase>\n");
        }
        x.push_str("  </testsuite>\n</testsuites>\n");
        x
    }

    /// TAP version 13: one test point per request, a YAML block for each
    /// one that did not pass.
    pub(crate) fn tap(&self) -> String {
        let mut t = format!("TAP version 13\n1..{}\n", self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            let ok = if e.outcome == "pass" { "ok" } else { "not ok" };
            t.push_str(&format!(
                "{ok} {} - {}\n",
                i + 1,
                e.path.replace('#', "\\#")
            ));
            if e.outcome == "pass" {
                continue;
            }
            t.push_str("  ---\n");
            t.push_str(&format!("  outcome: {}\n", e.outcome));
            if let Some(s) = &e.status {
                t.push_str(&format!("  status: {}\n", yaml(s)));
            }
            if let Some(ms) = e.duration_ms {
                t.push_str(&format!("  duration_ms: {ms}\n"));
            }
            if e.attempts > 1 {
                t.push_str(&format!("  attempts: {}\n", e.attempts));
            }
            t.push_str("  problems:\n");
            for p in e.problems() {
                t.push_str(&format!("    - {}\n", yaml(&p)));
            }
            t.push_str("  ...\n");
        }
        t
    }

    /// One HTML page with no script and no external file.
    pub(crate) fn html(&self) -> String {
        let mut rows = String::new();
        for e in &self.entries {
            let asserts = if e.asserts.is_empty() {
                String::new()
            } else {
                format!(
                    "{}/{}",
                    e.asserts.len() - e.failed_asserts().count(),
                    e.asserts.len()
                )
            };
            rows.push_str(&format!(
                "<tr class=\"{o}\"><td>{O}</td><td>{p}</td><td>{k}</td><td>{s}</td><td class=\"n\">{d}</td><td class=\"n\">{a}</td></tr>\n",
                o = e.outcome,
                O = e.outcome.to_ascii_uppercase(),
                p = xml(&e.path),
                k = e.kind.unwrap_or(""),
                s = xml(&format!(
                    "{}{}",
                    e.status.as_deref().unwrap_or(""),
                    if e.attempts > 1 {
                        format!(" ({} attempts)", e.attempts)
                    } else {
                        String::new()
                    }
                )),
                d = e.duration_ms.map(|m| format!("{m} ms")).unwrap_or_default(),
                a = asserts,
            ));
            if e.outcome != "pass" {
                let items: String = e
                    .problems()
                    .iter()
                    .map(|p| format!("<li>{}</li>", xml(p)))
                    .collect();
                rows.push_str(&format!(
                    "<tr class=\"detail\"><td></td><td colspan=\"5\"><ul>{items}</ul></td></tr>\n"
                ));
            }
        }
        let env = self
            .environment
            .as_deref()
            .map(|e| format!(" · environment <b>{}</b>", xml(e)))
            .unwrap_or_default();
        format!(
            r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sendito run: {folder}</title>
<style>
:root {{ color-scheme: light dark; --pass: #1a7f37; --fail: #bf3989; --error: #cf222e; }}
body {{ font: 14px/1.5 system-ui, sans-serif; margin: 2rem; }}
h1 {{ font-size: 1.25rem; margin: 0 0 .25rem; }}
p {{ margin: 0 0 1rem; opacity: .8; }}
table {{ border-collapse: collapse; width: 100%; }}
th, td {{ text-align: left; padding: .3rem .6rem; border-bottom: 1px solid #8884; vertical-align: top; }}
td.n {{ text-align: right; font-variant-numeric: tabular-nums; }}
tr.pass td:first-child {{ color: var(--pass); font-weight: 600; }}
tr.fail td:first-child {{ color: var(--fail); font-weight: 600; }}
tr.error td:first-child {{ color: var(--error); font-weight: 600; }}
tr.detail td {{ border-bottom: 1px solid #8884; }}
tr.detail ul {{ margin: 0; padding-left: 1.2rem; font-family: ui-monospace, monospace; font-size: 13px; }}
</style>
</head>
<body>
<h1>Sendito run: {folder}</h1>
<p>{passed} passed, {failed} failed, {errors} error(s) · {total} request(s) · {secs} s{env}</p>
<table>
<thead><tr><th>Result</th><th>Request</th><th>Kind</th><th>Status</th><th class="n">Time</th><th class="n">Asserts</th></tr></thead>
<tbody>
{rows}</tbody>
</table>
</body>
</html>
"#,
            folder = xml(&self.folder),
            passed = self.passed(),
            failed = self.failed(),
            errors = self.errors(),
            total = self.entries.len(),
            secs = secs(Some(self.duration_ms)),
        )
    }
}

/// Milliseconds as seconds with three decimals (`0.087`).
fn secs(ms: Option<u128>) -> String {
    format!("{:.3}", ms.unwrap_or(0) as f64 / 1000.0)
}

/// Text escaped for XML and HTML, in content and in quoted attributes.
fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 forbids most control characters, even escaped.
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\t' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// A YAML scalar: a JSON string is valid YAML and escapes everything.
fn yaml(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> SuiteReport {
        let a = |label: &str, pass, actual: &str| AssertResult {
            label: label.into(),
            pass,
            actual: actual.into(),
        };
        SuiteReport {
            environment: Some("local".into()),
            folder: "Users".into(),
            duration_ms: 412,
            entries: vec![
                Entry {
                    path: "Users/list.toml".into(),
                    kind: Some("http"),
                    outcome: "pass",
                    status: Some("200 OK".into()),
                    duration_ms: Some(87),
                    asserts: vec![a("status == 200", true, "200")],
                    unresolved: Some(false),
                    ..Default::default()
                },
                Entry {
                    path: "Users/a<b>&\"c\" #1.toml".into(),
                    kind: Some("http"),
                    outcome: "fail",
                    status: Some("404 Not Found".into()),
                    duration_ms: Some(12),
                    asserts: vec![a("status == 200", false, "404")],
                    unresolved: Some(false),
                    ..Default::default()
                },
                Entry {
                    path: "top.toml".into(),
                    kind: Some("grpc"),
                    outcome: "error",
                    unresolved: Some(true),
                    error: Some("unresolved variables: token".into()),
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn counts_include_unresolved_in_errors() {
        let r = report();
        assert_eq!(
            (r.passed(), r.failed(), r.errors(), r.unresolved()),
            (1, 1, 1, 1)
        );
        let v: serde_json::Value = serde_json::from_str(&r.json()).unwrap();
        assert_eq!(v["total"], 3);
        assert_eq!(v["errors"], 1);
        assert_eq!(v["requests"][1]["failed_asserts"], 1);
        assert_eq!(v["requests"][2]["status"], serde_json::Value::Null);
    }

    #[test]
    fn json_leaves_out_unresolved_for_a_file_that_did_not_load() {
        let r = SuiteReport {
            entries: vec![Entry {
                path: "bad.toml".into(),
                outcome: "error",
                error: Some("parse error".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::from_str(&r.json()).unwrap();
        assert!(v["requests"][0].get("unresolved").is_none());
        assert_eq!(v["requests"][0]["kind"], serde_json::Value::Null);
    }

    #[test]
    fn junit_escapes_and_classifies() {
        let x = report().junit();
        assert!(
            x.contains(
                r#"<testsuites name="sendito" tests="3" failures="1" errors="1" time="0.412">"#
            ),
            "{x}"
        );
        assert!(x.contains(r#"<property name="environment" value="local"/>"#));
        assert!(x.contains(r#"<testcase classname="Users" name="Users/list.toml" time="0.087"/>"#));
        assert!(
            x.contains("name=\"Users/a&lt;b&gt;&amp;&quot;c&quot; #1.toml\""),
            "{x}"
        );
        assert!(
            x.contains(r#"<failure message="✗ status == 200  (actual: 404)">"#),
            "{x}"
        );
        assert!(x.contains(r#"<testcase classname="." name="top.toml" time="0.000">"#));
        assert!(x.contains("<error message=\"unresolved variables: token\">"));
        assert!(!x.contains("<a<"));
    }

    #[test]
    fn tap_lists_every_request_with_diagnostics() {
        let t = report().tap();
        assert!(
            t.starts_with("TAP version 13\n1..3\nok 1 - Users/list.toml\n"),
            "{t}"
        );
        assert!(t.contains("not ok 2 - Users/a<b>&\"c\" \\#1.toml\n"), "{t}");
        assert!(t.contains("    - \"✗ status == 200  (actual: 404)\"\n"));
        assert!(t.contains("not ok 3 - top.toml\n  ---\n  outcome: error\n"));
        assert!(t.contains("    - \"unresolved variables: token\"\n  ...\n"));
    }

    #[test]
    fn html_escapes_text() {
        let h = report().html();
        assert!(h.starts_with("<!doctype html>"));
        assert!(h.contains("Users/a&lt;b&gt;&amp;&quot;c&quot; #1.toml"));
        assert!(h.contains("1 passed, 1 failed, 1 error(s) · 3 request(s) · 0.412 s"));
        assert!(h.contains("environment <b>local</b>"));
        assert!(!h.contains("<script"));
    }

    #[test]
    fn xml_drops_forbidden_control_characters() {
        assert_eq!(xml("a\u{1}b\tc"), "ab\tc");
    }
}
