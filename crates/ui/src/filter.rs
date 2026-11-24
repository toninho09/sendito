//! jq expression filtering of JSON response bodies (display only). Built on
//! jaq, so the full jq language works: paths, pipes, `select`, `map`,
//! slicing, arithmetic, regular expressions.
//!
//! Two builtins are removed from the library set because they do not belong
//! in a viewer: `halt` (exits the process) and `stderr`/`halt_error` (write
//! straight to the terminal). Using them is a compile error.

use jaq_all::data;
use jaq_all::jaq_core::{Vars, unwrap_valr};
use jaq_all::{json, load};

/// Builtins that must not run inside the UI process, and the standard
/// definitions that reference them.
const REMOVED_BUILTINS: &[&str] = &["halt", "stderr_empty"];
const REMOVED_DEFS: &[&str] = &["halt", "halt_error", "stderr"];

/// Compiles a jq expression with the standard functions, minus the ones that
/// exit the process or write to the terminal.
fn compile(expr: &str) -> Result<data::Filter, String> {
    let funs = data::base_funs().filter(|(name, _, _)| !REMOVED_BUILTINS.contains(name));
    let defs = jaq_all::defs().filter(|d| !REMOVED_DEFS.contains(&d.name));
    jaq_all::compile_with(expr, defs, funs, &[]).map_err(|frs| first_line(&report(&frs)))
}

/// Formats jaq's file reports as plain text (no colours).
fn report(frs: &[load::FileReports]) -> String {
    frs.iter()
        .map(|fr| format!("{}", load::FileReportsDisp::new(fr)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first line of a (possibly multi-line, code-annotated) error.
fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("invalid expression")
        .trim()
        .to_string()
}

/// Most outputs one filter may give. A filter such as `repeat(1)` never
/// ends; it runs on the UI thread, so it stops here.
const MAX_OUTPUTS: usize = 100_000;
/// Most bytes all outputs of one filter may hold.
const MAX_OUTPUT_BYTES: usize = 256 * 1024 * 1024;

/// Runs `expr` over the JSON document in `input` once and returns every
/// output value as compact JSON bytes.
///
/// Errors are human-readable, single-line strings meant for the filter prompt
/// and the status line.
pub fn apply(expr: &str, input: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let filter = compile(expr)?;
    let input: json::Val =
        json::read::parse_single(input).map_err(|e| first_line(&e.to_string()))?;
    let filename: json::Val = "<response>".to_string().into();
    let runner = data::Runner::default();
    let vars = Vars::default();
    let inputs = std::iter::once(Ok::<_, std::convert::Infallible>(input));
    let mut out = Vec::new();
    let mut total = 0usize;
    let run = data::run(
        &runner,
        &filter,
        vars,
        (&filename, inputs),
        |e| e,
        |vx| {
            let v = unwrap_valr(vx).map_err(|e| first_line(&e.to_string()))?;
            let mut buf = Vec::new();
            let pp = json::write::Pp::default();
            json::write::write(&mut buf, &pp, 0, &v).map_err(|e| first_line(&e.to_string()))?;
            total += buf.len();
            if out.len() >= MAX_OUTPUTS || total > MAX_OUTPUT_BYTES {
                return Err(format!(
                    "filter stopped: more than {MAX_OUTPUTS} outputs or {} MB",
                    MAX_OUTPUT_BYTES / (1024 * 1024)
                ));
            }
            out.push(buf);
            Ok(())
        },
    );
    run.map_err(|e| first_line(&e))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::apply;
    use serde_json::json;

    #[test]
    fn an_endless_filter_stops() {
        let e = apply("repeat(1)", b"{}").unwrap_err();
        assert!(e.contains("filter stopped"), "{e}");
    }

    fn one(expr: &str, input: serde_json::Value) -> serde_json::Value {
        let out = apply(expr, serde_json::to_string(&input).unwrap().as_bytes()).unwrap();
        assert_eq!(out.len(), 1, "{expr:?} -> {out:?}");
        serde_json::from_slice(&out.into_iter().next().unwrap()).unwrap()
    }

    fn many(expr: &str, input: serde_json::Value) -> Vec<serde_json::Value> {
        let out = apply(expr, serde_json::to_string(&input).unwrap().as_bytes()).unwrap();
        out.iter()
            .map(|b| serde_json::from_slice(b).unwrap())
            .collect()
    }

    #[test]
    fn field_paths() {
        let v = json!({"data": {"items": [{"id": 1}, {"id": 2}]}});
        assert_eq!(one(".data.items", v.clone()), json!([{"id": 1}, {"id": 2}]));
        assert_eq!(one(".data.items[1].id", v.clone()), json!(2));
        assert_eq!(one(".missing", v), json!(null));
    }

    #[test]
    fn iteration_and_select() {
        let v =
            json!({"items": [{"n": 1, "ok": true}, {"n": 2, "ok": false}, {"n": 3, "ok": true}]});
        assert_eq!(
            many(".items[] | .n", v.clone()),
            vec![json!(1), json!(2), json!(3)]
        );
        assert_eq!(
            many(".items[] | select(.ok) | .n", v.clone()),
            vec![json!(1), json!(3)]
        );
        assert_eq!(one("[.items[] | select(.ok) | .n]", v), json!([1, 3]));
    }

    #[test]
    fn map_add_and_slicing() {
        let v = json!({"xs": [1, 2, 3, 4]});
        assert_eq!(one(".xs | map(.*10) | add", v.clone()), json!(100));
        assert_eq!(one(".xs[1:3]", v.clone()), json!([2, 3]));
        assert_eq!(one(".xs[-1]", v.clone()), json!(4));
        assert_eq!(one(".xs | length", v), json!(4));
    }

    #[test]
    fn strings_regex_and_keys() {
        let v = json!({"name": "Ada Lovelace", "tags": ["x", "y"]});
        assert_eq!(
            one(".name | ascii_downcase", v.clone()),
            json!("ada lovelace")
        );
        assert_eq!(one(".name | test(\"^Ada\")", v.clone()), json!(true));
        assert_eq!(
            one(".name | split(\" \") | .[1]", v.clone()),
            json!("Lovelace")
        );
        assert_eq!(one("keys", v), json!(["name", "tags"]));
    }

    #[test]
    fn scalar_outputs_stay_json() {
        assert_eq!(one(".a", json!({"a": "text"})), json!("text"));
        assert_eq!(one(".a", json!({"a": 1.5})), json!(1.5));
        assert_eq!(one(".a", json!({"a": null})), json!(null));
        assert_eq!(one(".a", json!({"a": true})), json!(true));
        assert_eq!(one("1 + 2", json!({})), json!(3));
    }

    #[test]
    fn big_integers_round_trip() {
        // serde_json would lose this in an f64; jaq's own parser keeps it.
        let out = apply(".id", b"{\"id\": 123456789012345678901234567890}").unwrap();
        assert_eq!(out, vec![b"123456789012345678901234567890".to_vec()]);
    }

    #[test]
    fn empty_output() {
        assert!(many(".a[]", json!({"a": []})).is_empty());
        assert!(many(".a | select(. == 9)", json!({"a": 1})).is_empty());
    }

    #[test]
    fn invalid_input_is_an_error() {
        assert!(apply(".", b"not json").is_err());
        assert!(apply(".", b"").is_err());
    }

    #[test]
    fn invalid_expression_is_an_error() {
        assert!(!apply(".a[", b"{\"a\":1}".as_ref()).unwrap_err().is_empty());
        assert!(!apply("missing_fn", b"{}".as_ref()).unwrap_err().is_empty());
    }

    #[test]
    fn runtime_error_is_an_error() {
        assert!(
            !apply(".a | to_entries", b"{\"a\":3}".as_ref())
                .unwrap_err()
                .is_empty()
        );
        // A path into a scalar reports the offending value.
        assert!(!apply(".a.b", b"{\"a\":3}".as_ref()).unwrap_err().is_empty());
    }

    #[test]
    fn halt_and_stderr_are_not_available() {
        assert!(apply("halt", b"{}".as_ref()).is_err());
        assert!(apply("halt_error", b"{}".as_ref()).is_err());
        assert!(apply(". | stderr", b"{}".as_ref()).is_err());
    }

    #[test]
    fn debug_is_a_silent_passthrough_and_input_is_exhausted() {
        // No logger is initialized, so `debug` logs nowhere and passes the
        // value through; `input`/`inputs` have no further inputs and yield
        // nothing instead of blocking (both would otherwise disturb the
        // terminal).
        assert_eq!(
            apply(". as $x | debug", b"{\"a\":1}".as_ref()).unwrap(),
            vec![b"{\"a\":1}".to_vec()]
        );
        assert!(apply("input", b"{}".as_ref()).unwrap().is_empty());
        assert!(apply("inputs", b"[1]".as_ref()).unwrap().is_empty());
    }

    #[test]
    fn large_input_stays_fast() {
        let items: Vec<serde_json::Value> =
            (0..30_000).map(|i| json!({"id": i, "name": "x"})).collect();
        let v = serde_json::to_string(&json!({"items": items})).unwrap();
        assert!(v.len() > 500_000, "{} bytes", v.len());
        let t = std::time::Instant::now();
        let out = apply(".items[] | select(.id % 100 == 0) | .id", v.as_bytes()).unwrap();
        let ms = t.elapsed().as_millis();
        eprintln!(
            "jq on {} KB: {} outputs in {} ms",
            v.len() / 1024,
            out.len(),
            ms
        );
        assert_eq!(out.len(), 300);
        // CI-safe bound: this machine measures ~250 ms (see the report); the
        // gate itself is generous so a loaded runner does not flake.
        assert!(ms < 2000, "took {ms} ms");
    }
}
