//! Lua scripts: `[scripts.lua] pre` runs after the `pre` rows and can set
//! variables and change the request; `post` runs after the assertions and can
//! set variables, read the response and record tests.
//!
//! The Lua 5.4 state loads only the `string`, `table`, `math` and `utf8`
//! libraries, and the base functions that load code or files are removed.
//! Scripts cannot reach files, the network or other processes, and a run
//! stops at fixed instruction and memory limits.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine as _;
use mlua::{
    DeserializeOptions, HookTriggers, Lua, LuaOptions, LuaSerdeExt, SerializeOptions, StdLib,
    UserData, UserDataFields, UserDataMethods, Value, VmState,
};

use crate::model::{Body, KeyValue, LuaScript, Request};
use crate::script::{AssertResult, RUNTIME_LAYER, ResponseView, TIME_PHASES, digest};
use crate::vars::{Rendered, Scope, VarValue, scrub};

/// Lua instructions one script may run before it stops.
const MAX_INSTRUCTIONS: u64 = 10_000_000;
/// The hook counts instructions in steps of this size.
const HOOK_STEP: u32 = 10_000;
/// Memory the Lua state may hold.
const MAX_MEMORY: usize = 64 * 1024 * 1024;
/// Base functions removed: they load code or read files.
const REMOVED: [&str; 4] = ["dofile", "loadfile", "load", "collectgarbage"];

type LuaResult<T> = mlua::Result<T>;

/// What the scripts of one phase did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LuaOut {
    /// Variables set, in order (also stored in the runtime layer).
    pub sets: Vec<(String, VarValue)>,
    /// `test(name, pass)` results of the post scripts.
    pub tests: Vec<AssertResult>,
    /// `print` lines, secrets masked.
    pub logs: Vec<String>,
}

struct State {
    scope: Scope,
    /// The request the pre scripts change; `None` in the post phase.
    request: Option<Request>,
    out: LuaOut,
}

type Shared = Rc<RefCell<State>>;

struct Req(Shared);

struct Resp {
    status: i64,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    timing: Vec<(String, f64)>,
}

/// Runs the pre scripts in order on `req`. Each script sees the variables and
/// the request changes of the scripts before it. The first error stops the
/// phase; the error names the script and the line.
pub fn run_lua_pre(
    scripts: &[LuaScript],
    req: &mut Request,
    scope: &mut Scope,
) -> Result<LuaOut, String> {
    if scripts.iter().all(|s| s.code.trim().is_empty()) {
        return Ok(LuaOut::default());
    }
    let catalog = scope.catalog().clone();
    let shared = share(scope, Some(req.clone()));
    let failed = match state(&shared) {
        Ok((lua, budget)) => {
            let request = lua.create_userdata(Req(shared.clone()));
            match request.and_then(|r| lua.globals().set("request", r)) {
                Ok(()) => run_all(&lua, &budget, catalog.files.iter().chain(scripts)).err(),
                Err(e) => Some(format!("Lua setup: {e}")),
            }
        }
        Err(e) => Some(format!("Lua setup: {e}")),
    };
    let (out, request) = finish(shared, scope);
    if let Some(e) = failed {
        return Err(scrub(&e, &scope.secret_values()));
    }
    if let Some(r) = request {
        *req = r;
    }
    Ok(out)
}

/// Runs the post scripts in order. An error is a failed test named after the
/// script; the scripts after it still run.
pub fn run_lua_post(scripts: &[LuaScript], resp: &ResponseView<'_>, scope: &mut Scope) -> LuaOut {
    if scripts.iter().all(|s| s.code.trim().is_empty()) {
        return LuaOut::default();
    }
    let catalog = scope.catalog().clone();
    let shared = share(scope, None);
    let setup = state(&shared).and_then(|(lua, budget)| {
        let sh = shared.clone();
        let test = lua.create_function(move |_, (name, pass): (String, bool)| {
            sh.borrow_mut().out.tests.push(AssertResult {
                label: format!("test: {name}"),
                pass,
                actual: pass.to_string(),
            });
            Ok(())
        })?;
        lua.globals().set("test", test)?;
        let response = lua.create_userdata(Resp {
            status: i64::from(resp.status),
            headers: resp.headers.to_vec(),
            body: resp.body.to_vec(),
            timing: resp
                .timing
                .iter()
                .map(|(n, d)| (n.to_string(), d.as_secs_f64() * 1000.0))
                .collect(),
        })?;
        lua.globals().set("response", response)?;
        Ok((lua, budget))
    });
    match setup {
        Ok((lua, budget)) => {
            let all = catalog.files.iter().chain(scripts);
            for s in all.filter(|s| !s.code.trim().is_empty()) {
                if let Err(e) = run_one(&lua, &budget, s) {
                    shared.borrow_mut().out.tests.push(AssertResult {
                        label: format!("{} script", origin(s)),
                        pass: false,
                        actual: e,
                    });
                }
            }
        }
        Err(e) => shared.borrow_mut().out.tests.push(AssertResult {
            label: "Lua setup".into(),
            pass: false,
            actual: e.to_string(),
        }),
    }
    let (mut out, _) = finish(shared, scope);
    let secrets = scope.secret_values();
    for t in &mut out.tests {
        t.actual = scrub(&t.actual, &secrets);
    }
    out
}

fn run_all<'a>(
    lua: &Lua,
    budget: &Rc<Cell<u64>>,
    scripts: impl Iterator<Item = &'a LuaScript>,
) -> Result<(), String> {
    for s in scripts.filter(|s| !s.code.trim().is_empty()) {
        run_one(lua, budget, s)?;
    }
    Ok(())
}

/// Runs one script with a fresh instruction budget. The chunk name is the
/// origin, so messages read `request script:3: ...`.
fn run_one(lua: &Lua, budget: &Rc<Cell<u64>>, s: &LuaScript) -> Result<(), String> {
    budget.set(0);
    let name = format!("{} script", origin(s));
    lua.load(s.code.as_str())
        .set_name(format!("={name}"))
        .exec()
        .map_err(|e| error_text(&name, &e))
}

/// The global names Sendito gives to scripts. A catalog file must not change
/// them.
const SENDITO_NAMES: [&str; 11] = [
    "vars",
    "render",
    "print",
    "hash",
    "base64_encode",
    "base64_decode",
    "json_parse",
    "json_stringify",
    "request",
    "response",
    "test",
];

/// The user's Lua functions: the `*.lua` files of the global config folder
/// and of `.sendito/lua/`. Every script phase runs the files first; a
/// template calls a function as `{{ $lua.name(args) }}`.
#[derive(Debug, Clone, Default)]
pub struct LuaCatalog {
    files: Vec<LuaScript>,
    /// The global functions the files define, sorted.
    names: Vec<String>,
}

impl LuaCatalog {
    /// Runs `files` in order and keeps the ones that load. A file that fails
    /// or changes a Sendito name is left out, with a message that names it.
    pub fn new(files: Vec<LuaScript>) -> (Self, Vec<String>) {
        let mut catalog = Self::default();
        let mut errors = Vec::new();
        if files.is_empty() {
            return (catalog, errors);
        }
        let shared = share(&mut Scope::new(), None);
        let (lua, budget) = match state(&shared) {
            Ok(s) => s,
            Err(e) => return (catalog, vec![format!("Lua setup: {e}")]),
        };
        for f in files {
            match load_file(&lua, &budget, &f) {
                Ok(names) => {
                    catalog.names.extend(names);
                    catalog.files.push(f);
                }
                Err(e) => errors.push(e),
            }
        }
        catalog.names.sort_unstable();
        catalog.names.dedup();
        (catalog, errors)
    }

    /// `true` when a catalog file defines the global function `name`.
    pub fn has(&self, name: &str) -> bool {
        self.names
            .binary_search_by(|n| n.as_str().cmp(name))
            .is_ok()
    }
}

/// Runs one catalog file and returns the global functions it adds.
fn load_file(lua: &Lua, budget: &Rc<Cell<u64>>, f: &LuaScript) -> Result<Vec<String>, String> {
    let globals = || -> Result<HashMap<String, Value>, String> {
        lua.globals()
            .pairs::<String, Value>()
            .collect::<LuaResult<_>>()
            .map_err(|e| e.to_string())
    };
    let before = globals()?;
    run_one(lua, budget, f)?;
    let after = globals()?;
    if let Some(n) = SENDITO_NAMES
        .iter()
        .find(|n| before.get(**n) != after.get(**n))
    {
        return Err(format!(
            "{} script: it changes the Sendito name `{n}`",
            origin(f)
        ));
    }
    Ok(after
        .into_iter()
        .filter(|(k, v)| matches!(v, Value::Function(_)) && !before.contains_key(k))
        .map(|(k, _)| k)
        .collect())
}

/// A Lua state and the instruction budget of its running script.
type Budgeted = (Lua, Rc<Cell<u64>>);

thread_local! {
    /// The Lua state of the template calls of one scope (one send), with the
    /// catalog files loaded, by scope id.
    static TEMPLATE_STATE: RefCell<Option<(u64, Budgeted)>> = const { RefCell::new(None) };
}

/// Calls the catalog function `name` for a template and returns its text and
/// whether it read a secret. The calls of one scope share one Lua state, so
/// the catalog files run once per send. The function can read variables
/// (`vars.get`, `vars.has`, `render`) one level below `depth`; a `render`
/// that leaves a placeholder unresolved is an error. `print` writes nothing.
/// An error message has the secrets that the function read masked.
pub(crate) fn call_catalog(
    scope: &Scope,
    name: &str,
    args: &[String],
    depth: usize,
) -> Result<(String, bool), String> {
    let kept = TEMPLATE_STATE
        .with_borrow_mut(Option::take)
        .filter(|(id, ..)| *id == scope.id());
    let (lua, budget) = match kept {
        Some((_, state)) => state,
        None => template_state(scope)?,
    };
    let read = RefCell::new(Vec::new());
    let result = call_in(&lua, &budget, scope, name, args, depth, &read);
    // A nested call took the state out and put its own back; this one replaces it.
    TEMPLATE_STATE.with_borrow_mut(|s| *s = Some((scope.id(), (lua, budget))));
    let read = read.into_inner();
    match result {
        Ok(text) => Ok((text, !read.is_empty())),
        Err(e) => Err(scrub(&error_text(&format!("$lua.{name}"), &e), &read)),
    }
}

/// The state of the template calls: the sandbox, a `print` that writes
/// nothing, and the catalog files, which run before `vars` exists.
fn template_state(scope: &Scope) -> Result<Budgeted, String> {
    let setup = || -> LuaResult<_> {
        let (lua, budget) = sandbox()?;
        lua.globals().set(
            "print",
            lua.create_function(|_, _: mlua::MultiValue| Ok(()))?,
        )?;
        Ok((lua, budget))
    };
    let (lua, budget) = setup().map_err(|e| format!("Lua setup: {e}"))?;
    run_all(&lua, &budget, scope.catalog().files.iter())?;
    Ok((lua, budget))
}

/// One template call in `lua`, with `vars` and `render` bound to `scope` for
/// this call only. `read` gets the text of every secret value the function reads.
fn call_in(
    lua: &Lua,
    budget: &Rc<Cell<u64>>,
    scope: &Scope,
    name: &str,
    args: &[String],
    depth: usize,
    read: &RefCell<Vec<String>>,
) -> LuaResult<String> {
    let seen = |r: Rendered| {
        if r.secret {
            read.borrow_mut().push(r.text.clone());
        }
        r.text
    };
    lua.scope(|s| {
        let g = lua.globals();
        let vars = lua.create_table()?;
        vars.set(
            "get",
            s.create_function(|_, n: String| Ok(scope.lookup_at(n.trim(), depth + 1).map(seen)))?,
        )?;
        vars.set(
            "has",
            s.create_function(|_, n: String| Ok(scope.is_defined(n.trim())))?,
        )?;
        g.set("vars", vars)?;
        g.set(
            "render",
            s.create_function(|_, t: String| {
                let r = scope.render_at(&t, depth + 1);
                match r.unresolved.first() {
                    Some(u) => Err(fail(format!("render: {{{{{u}}}}} is not resolved"))),
                    None => Ok(seen(r)),
                }
            })?,
        )?;
        budget.set(0);
        let f: mlua::Function = g.get(name)?;
        let v: Value = f.call(mlua::Variadic::from_iter(args.iter().cloned()))?;
        text(lua, v)
    })
}

/// Moves the scope into the shared state for the run.
fn share(scope: &mut Scope, request: Option<Request>) -> Shared {
    Rc::new(RefCell::new(State {
        scope: std::mem::replace(scope, Scope::new()),
        request,
        out: LuaOut::default(),
    }))
}

/// Gives the scope back and returns the result. The logs are masked with the
/// secrets known at the end of the run.
fn finish(shared: Shared, scope: &mut Scope) -> (LuaOut, Option<Request>) {
    let mut st = shared.borrow_mut();
    *scope = std::mem::replace(&mut st.scope, Scope::new());
    let mut out = std::mem::take(&mut st.out);
    let request = st.request.take();
    let secrets = scope.secret_values();
    for l in &mut out.logs {
        *l = scrub(l, &secrets);
    }
    (out, request)
}

fn origin(s: &LuaScript) -> &str {
    if s.origin.is_empty() {
        "request"
    } else {
        &s.origin
    }
}

/// The message of a Lua error without the stack traceback. An error that a
/// Sendito function raised gets the line of the call from the traceback.
fn error_text(chunk: &str, e: &mlua::Error) -> String {
    let first = |s: &str| {
        s.split("\nstack traceback:")
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    match e {
        mlua::Error::RuntimeError(m) => first(m),
        mlua::Error::SyntaxError { message, .. } => first(message),
        mlua::Error::MemoryError(_) => format!("{chunk}: the script used too much memory"),
        mlua::Error::CallbackError { traceback, cause } => {
            let msg = error_text(chunk, cause);
            let prefix = format!("{chunk}:");
            let line = traceback.lines().find_map(|l| {
                let rest = l.trim().strip_prefix(&prefix)?;
                let n = rest.split(':').next()?;
                n.parse::<u32>().ok()
            });
            match line {
                Some(n) if !msg.starts_with(&prefix) => format!("{chunk}:{n}: {msg}"),
                _ => msg,
            }
        }
        other => first(&other.to_string()),
    }
}

fn fail(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

/// The text of a script value: strings as they are, numbers and booleans as
/// Lua writes them, tables as JSON.
fn text(lua: &Lua, v: Value) -> LuaResult<String> {
    match v {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        Value::Integer(_) | Value::Number(_) => Ok(lua
            .coerce_string(v)?
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()),
        Value::Table(_) => json_text(lua, v),
        Value::Nil => Err(fail("the value is nil")),
        other => Err(fail(format!("a {} has no text", other.type_name()))),
    }
}

/// A Lua value as compact JSON, object keys sorted.
fn json_text(lua: &Lua, v: Value) -> LuaResult<String> {
    let json: serde_json::Value =
        lua.from_value_with(v, DeserializeOptions::new().sort_keys(true))?;
    serde_json::to_string(&json).map_err(|e| fail(e.to_string()))
}

/// The Lua state with the sandbox, the limits and the functions that need no
/// scope. The returned counter is the instruction budget of the running script.
fn sandbox() -> LuaResult<Budgeted> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
        LuaOptions::new(),
    )?;
    lua.set_memory_limit(MAX_MEMORY)?;
    let budget = Rc::new(Cell::new(0u64));
    let used = budget.clone();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_STEP),
        move |_, _| {
            used.set(used.get() + u64::from(HOOK_STEP));
            if used.get() > MAX_INSTRUCTIONS {
                return Err(fail(format!(
                    "the script ran more than {MAX_INSTRUCTIONS} instructions"
                )));
            }
            Ok(VmState::Continue)
        },
    )?;
    let g = lua.globals();
    for name in REMOVED {
        g.set(name, Value::Nil)?;
    }
    let string: mlua::Table = g.get("string")?;
    string.set("dump", Value::Nil)?;

    g.set(
        "hash",
        lua.create_function(|_, (algo, value): (String, mlua::String)| {
            digest(algo.trim(), &value.as_bytes()).map_err(fail)
        })?,
    )?;
    g.set(
        "base64_encode",
        lua.create_function(|_, value: mlua::String| {
            Ok(base64::engine::general_purpose::STANDARD.encode(value.as_bytes()))
        })?,
    )?;
    g.set(
        "base64_decode",
        lua.create_function(|lua, value: String| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value.trim())
                .map_err(|e| fail(format!("base64_decode: {e}")))?;
            lua.create_string(bytes)
        })?,
    )?;
    g.set(
        "json_parse",
        lua.create_function(|lua, value: String| {
            let json: serde_json::Value =
                serde_json::from_str(&value).map_err(|e| fail(format!("json_parse: {e}")))?;
            json_value(lua, &json)
        })?,
    )?;
    g.set(
        "json_stringify",
        lua.create_function(|lua, value: Value| json_text(lua, value))?,
    )?;
    Ok((lua, budget))
}

/// The state of the scripts: the sandbox, plus `print`, `vars` and `render`
/// on the shared scope.
fn state(shared: &Shared) -> LuaResult<Budgeted> {
    let (lua, budget) = sandbox()?;
    let g = lua.globals();
    let sh = shared.clone();
    let print = lua.create_function(move |lua, args: mlua::MultiValue| {
        let mut parts = Vec::new();
        for v in args {
            parts.push(match v {
                Value::String(s) => s.to_string_lossy(),
                Value::Table(_) => json_text(lua, v.clone()).unwrap_or_else(|_| "table".into()),
                other => lua
                    .coerce_string(other.clone())?
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_else(|| format!("{other:?}")),
            });
        }
        sh.borrow_mut().out.logs.push(parts.join("\t"));
        Ok(())
    })?;
    g.set("print", print)?;

    let vars = lua.create_table()?;
    let sh = shared.clone();
    vars.set(
        "get",
        lua.create_function(move |_, name: String| {
            Ok(sh.borrow().scope.lookup(name.trim()).map(|r| r.text))
        })?,
    )?;
    let sh = shared.clone();
    vars.set(
        "has",
        lua.create_function(move |_, name: String| Ok(sh.borrow().scope.is_defined(name.trim())))?,
    )?;
    let sh = shared.clone();
    vars.set(
        "set",
        lua.create_function(move |lua, (name, value): (String, Value)| {
            set_var(&sh, &name, text(lua, value)?, false)
        })?,
    )?;
    let sh = shared.clone();
    vars.set(
        "set_secret",
        lua.create_function(move |lua, (name, value): (String, Value)| {
            set_var(&sh, &name, text(lua, value)?, true)
        })?,
    )?;
    g.set("vars", vars)?;

    let sh = shared.clone();
    g.set(
        "render",
        lua.create_function(move |_, template: String| {
            Ok(sh.borrow().scope.render(&template).text)
        })?,
    )?;
    Ok((lua, budget))
}

/// A JSON value as a Lua value; `null` becomes `nil`.
fn json_value(lua: &Lua, json: &serde_json::Value) -> LuaResult<Value> {
    lua.to_value_with(
        json,
        SerializeOptions::new()
            .serialize_none_to_null(false)
            .serialize_unit_to_null(false),
    )
}

/// Sets a runtime variable. The value is secret when `secret` is set or when
/// it holds the value of a secret variable.
fn set_var(shared: &Shared, name: &str, value: String, secret: bool) -> LuaResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(fail("vars.set: the variable name is empty"));
    }
    let mut st = shared.borrow_mut();
    let secret = secret
        || st
            .scope
            .secret_values()
            .iter()
            .any(|s| value.contains(s.as_str()));
    let v = VarValue { value, secret };
    st.scope.set(RUNTIME_LAYER, name, v.clone());
    st.out.sets.push((name.to_string(), v));
    Ok(())
}

impl UserData for Resp {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("status", |_, r| Ok(r.status));
        fields.add_field_method_get("body", |lua, r| lua.create_string(&r.body));
        fields.add_field_method_get("headers", |lua, r| {
            let list = lua.create_table()?;
            for (n, v) in &r.headers {
                let h = lua.create_table()?;
                h.set("name", n.as_str())?;
                h.set("value", v.as_str())?;
                list.push(h)?;
            }
            Ok(list)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("header", |_, r, name: String| {
            Ok(r.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
                .map(|(_, v)| v.clone()))
        });
        methods.add_method("json", |lua, r, ()| {
            let json: serde_json::Value = serde_json::from_slice(&r.body)
                .map_err(|e| fail(format!("response:json: the body is not JSON: {e}")))?;
            json_value(lua, &json)
        });
        methods.add_method("time", |_, r, phase: String| {
            let phase = phase.trim();
            if !TIME_PHASES.contains(&phase) {
                return Err(fail(format!(
                    "response:time: unknown phase '{phase}' ({})",
                    TIME_PHASES.join(", ")
                )));
            }
            Ok(r.timing.iter().find(|(n, _)| n == phase).map(|(_, ms)| *ms))
        });
    }
}

/// The short name of a request kind for error messages.
fn kind(r: &Request) -> &'static str {
    match r {
        Request::Http(_) => "HTTP",
        Request::Grpc(_) => "gRPC",
        Request::Ws(_) => "WebSocket",
        Request::Sse(_) => "SSE",
        Request::Mqtt(_) => "MQTT",
        Request::Mcp(_) => "MCP",
    }
}

/// Runs `f` on the request of the pre phase.
fn with_req<T>(r: &Req, f: impl FnOnce(&mut Request) -> LuaResult<T>) -> LuaResult<T> {
    let mut st = r.0.borrow_mut();
    match st.request.as_mut() {
        Some(req) => f(req),
        None => Err(fail("request is not available here")),
    }
}

fn unsupported(member: &str, kind: &str) -> mlua::Error {
    fail(format!(
        "request.{member}: {kind} requests have no {member}"
    ))
}

fn url_mut(r: &mut Request) -> LuaResult<&mut String> {
    let k = kind(r);
    match r {
        Request::Http(h) => Ok(&mut h.url),
        Request::Grpc(g) => Ok(&mut g.target),
        Request::Ws(w) => Ok(&mut w.url),
        Request::Sse(s) => Ok(&mut s.url),
        Request::Mqtt(m) => Ok(&mut m.url),
        Request::Mcp(_) => Err(unsupported("url", k)),
    }
}

fn headers_mut(r: &mut Request) -> LuaResult<&mut Vec<KeyValue>> {
    let k = kind(r);
    match r {
        Request::Http(h) => Ok(&mut h.headers),
        Request::Grpc(g) => Ok(&mut g.metadata),
        Request::Ws(w) => Ok(&mut w.headers),
        Request::Sse(s) => Ok(&mut s.headers),
        Request::Mqtt(_) | Request::Mcp(_) => Err(unsupported("headers", k)),
    }
}

fn params_mut(r: &mut Request) -> LuaResult<&mut Vec<KeyValue>> {
    let k = kind(r);
    match r {
        Request::Http(h) => Ok(&mut h.params),
        Request::Sse(s) => Ok(&mut s.params),
        _ => Err(unsupported("params", k)),
    }
}

fn body_mut(r: &mut Request) -> LuaResult<&mut String> {
    let k = kind(r);
    match r {
        Request::Http(h) => match &mut h.body {
            Body::Json { text } | Body::Raw { text, .. } => Ok(text),
            Body::GraphQL { query, .. } => Ok(query),
            other => Err(fail(format!(
                "request.body: a {} body has no text; change the body type in the request",
                other.kind().as_str()
            ))),
        },
        Request::Grpc(g) => Ok(&mut g.message),
        Request::Ws(w) => Ok(&mut w.messages),
        Request::Mcp(m) => Ok(&mut m.arguments),
        Request::Sse(_) | Request::Mqtt(_) => Err(unsupported("body", k)),
    }
}

/// The value of the first enabled row named `name` (any case).
fn row(rows: &[KeyValue], name: &str) -> Option<String> {
    rows.iter()
        .find(|r| r.enabled && r.name.trim().eq_ignore_ascii_case(name.trim()))
        .map(|r| r.value.clone())
}

/// Sets the first row named `name` (any case) to `value` and enables it; the
/// other rows with that name are removed. With no such row, adds one.
fn set_row(rows: &mut Vec<KeyValue>, name: &str, value: String) -> LuaResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(fail("the name is empty"));
    }
    let same = |r: &KeyValue| r.name.trim().eq_ignore_ascii_case(name);
    match rows.iter().position(same) {
        Some(i) => {
            rows[i].value = value;
            rows[i].enabled = true;
            let mut n = 0;
            rows.retain(|r| {
                n += 1;
                n - 1 == i || !same(r)
            });
        }
        None => rows.push(KeyValue::new(name, value)),
    }
    Ok(())
}

fn remove_row(rows: &mut Vec<KeyValue>, name: &str) {
    rows.retain(|r| !r.name.trim().eq_ignore_ascii_case(name.trim()));
}

impl UserData for Req {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("url", |_, r| with_req(r, |q| url_mut(q).map(|u| u.clone())));
        fields.add_field_method_set("url", |lua, r, v: Value| {
            let v = text(lua, v)?;
            with_req(r, |q| url_mut(q).map(|u| *u = v))
        });
        fields.add_field_method_get("method", |_, r| {
            with_req(r, |q| match q {
                Request::Http(h) => Ok(h.method.clone()),
                other => Err(unsupported("method", kind(other))),
            })
        });
        fields.add_field_method_set("method", |lua, r, v: Value| {
            let v = text(lua, v)?;
            with_req(r, |q| match q {
                Request::Http(h) => {
                    h.method = v.trim().to_ascii_uppercase();
                    Ok(())
                }
                other => Err(unsupported("method", kind(other))),
            })
        });
        fields.add_field_method_get("body", |_, r| {
            with_req(r, |q| body_mut(q).map(|b| b.clone()))
        });
        fields.add_field_method_set("body", |lua, r, v: Value| {
            let v = text(lua, v)?;
            with_req(r, |q| body_mut(q).map(|b| *b = v))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("header", |_, r, name: String| {
            with_req(r, |q| headers_mut(q).map(|h| row(h, &name)))
        });
        methods.add_method("set_header", |lua, r, (name, v): (String, Value)| {
            let v = text(lua, v)?;
            with_req(r, |q| set_row(headers_mut(q)?, &name, v))
        });
        methods.add_method("remove_header", |_, r, name: String| {
            with_req(r, |q| headers_mut(q).map(|h| remove_row(h, &name)))
        });
        methods.add_method("param", |_, r, name: String| {
            with_req(r, |q| params_mut(q).map(|p| row(p, &name)))
        });
        methods.add_method("set_param", |lua, r, (name, v): (String, Value)| {
            let v = text(lua, v)?;
            with_req(r, |q| set_row(params_mut(q)?, &name, v))
        });
        methods.add_method("remove_param", |_, r, name: String| {
            with_req(r, |q| params_mut(q).map(|p| remove_row(p, &name)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GrpcRequest, HttpRequest, McpRequest, MqttRequest};
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn own(code: &str) -> Vec<LuaScript> {
        vec![LuaScript {
            origin: String::new(),
            code: code.into(),
        }]
    }

    fn scope() -> Scope {
        let mut env = BTreeMap::new();
        env.insert("host".to_string(), VarValue::plain("example.com"));
        env.insert("key".to_string(), VarValue::secret("s3cr3t"));
        let mut s = Scope::new();
        s.push_layer("env", env);
        s.push_layer(RUNTIME_LAYER, BTreeMap::new());
        s
    }

    fn http() -> Request {
        Request::Http(HttpRequest {
            url: "https://{{host}}/a".into(),
            headers: vec![
                KeyValue::new("Accept", "*/*"),
                KeyValue::new("accept", "x").disabled(),
            ],
            body: Body::Json { text: "{}".into() },
            ..Default::default()
        })
    }

    #[test]
    fn pre_sets_variables_and_changes_the_request() {
        let mut s = scope();
        let mut req = http();
        let out = run_lua_pre(
            &own(r#"
                vars.set("n", 1 + 2)
                vars.set("obj", { b = 2, a = 1 })
                vars.set("k2", "key=" .. vars.get("key"))
                if vars.get("missing") == nil then vars.set("missing_ok", true) end
                request.url = request.url .. "/b"
                request.method = "post"
                request:set_header("ACCEPT", "application/json")
                request:set_header("X-Sig", hash("sha256", "abc"))
                request:set_param("q", "{{host}}")
                request.body = json_stringify({ n = vars.get("n") })
                print("host is " .. vars.get("host"), "key " .. vars.get("key"))
            "#),
            &mut req,
            &mut s,
        )
        .unwrap();
        let names: Vec<_> = out.sets.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["n", "obj", "k2", "missing_ok"]);
        assert_eq!(out.sets[0].1, VarValue::plain("3"));
        // Object keys are sorted.
        assert_eq!(out.sets[1].1.value, r#"{"a":1,"b":2}"#);
        // A value that holds a secret is a secret.
        assert!(out.sets[2].1.secret);
        assert_eq!(s.lookup("n").unwrap().text, "3");
        assert_eq!(out.logs, ["host is example.com\tkey ••••••"]);
        let Request::Http(h) = req else { panic!() };
        assert_eq!(h.url, "https://{{host}}/a/b");
        assert_eq!(h.method, "POST");
        // The first `Accept` row is replaced; the other one is removed.
        assert_eq!(h.headers.len(), 2);
        assert_eq!(h.headers[0], KeyValue::new("Accept", "application/json"));
        assert_eq!(h.headers[1].name, "X-Sig");
        assert_eq!(h.params, vec![KeyValue::new("q", "{{host}}")]);
        assert_eq!(
            h.body,
            Body::Json {
                text: r#"{"n":"3"}"#.into()
            }
        );
    }

    #[test]
    fn helpers_render_hash_base64_and_json() {
        let mut s = scope();
        let mut req = http();
        let out = run_lua_pre(
            &own(r#"
                vars.set("r", render("https://{{host}}"))
                vars.set("h", hash("md5", ""))
                vars.set("b", base64_decode(base64_encode("hé")))
                local j = json_parse('{"a":[1,2],"z":null}')
                vars.set("j", j.a[2])
                vars.set("null_is_nil", j.z == nil)
                vars.set("arr", json_stringify(j.a))
                vars.set_secret("t", "plain")
                vars.set("has", vars.has("host") and not vars.has("nope"))
                vars.set("f", 1.5)
            "#),
            &mut req,
            &mut s,
        )
        .unwrap();
        let get = |n: &str| s.lookup(n).unwrap();
        assert_eq!(get("r").text, "https://example.com");
        assert_eq!(get("h").text, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(get("b").text, "hé");
        assert_eq!(get("j").text, "2");
        assert_eq!(get("null_is_nil").text, "true");
        assert_eq!(get("arr").text, "[1,2]");
        assert!(get("t").secret);
        assert_eq!(get("has").text, "true");
        assert_eq!(get("f").text, "1.5");
        assert_eq!(out.sets.len(), 9);
    }

    #[test]
    fn pre_errors_name_the_script_and_line_and_keep_the_request() {
        let mut s = scope();
        let mut req = http();
        let scripts = vec![
            LuaScript {
                origin: "folder api".into(),
                code: r#"vars.set("a", "1"); request.url = "changed""#.into(),
            },
            LuaScript {
                origin: String::new(),
                code: "local x = 1\nerror(\"bad \" .. vars.get(\"key\"))".into(),
            },
        ];
        let e = run_lua_pre(&scripts, &mut req, &mut s).unwrap_err();
        assert_eq!(e, "request script:2: bad ••••••");
        // The scope comes back; the request is not changed on error.
        assert!(s.lookup("host").is_some());
        let Request::Http(h) = req else { panic!() };
        assert_eq!(h.url, "https://{{host}}/a");
        // A folder script error names the folder; an error a Sendito
        // function raises gets the line of the call.
        let e = run_lua_pre(
            &[LuaScript {
                origin: "folder api".into(),
                code: "local a = 1\n\nvars.set(\"\", 1)".into(),
            }],
            &mut http(),
            &mut s,
        )
        .unwrap_err();
        assert_eq!(
            e,
            "folder api script:3: vars.set: the variable name is empty"
        );
        let e = run_lua_pre(&own("x = = 1"), &mut http(), &mut s).unwrap_err();
        assert!(e.starts_with("request script:1:"), "{e}");
    }

    #[test]
    fn members_a_kind_lacks_are_errors() {
        let mut s = scope();
        let cases: Vec<(Request, &str, &str)> = vec![
            (
                Request::Mqtt(MqttRequest::default()),
                "request:set_header('a', 'b')",
                "MQTT requests have no headers",
            ),
            (
                Request::Mcp(McpRequest::default()),
                "local u = request.url",
                "MCP requests have no url",
            ),
            (
                Request::Grpc(GrpcRequest::default()),
                "local m = request.method",
                "gRPC requests have no method",
            ),
            (
                Request::Grpc(GrpcRequest::default()),
                "request:param('a')",
                "gRPC requests have no params",
            ),
            (
                Request::new_sse(),
                "request.body = 'x'",
                "SSE requests have no body",
            ),
            (
                Request::new_http(),
                "request.body = 'x'",
                "a none body has no text",
            ),
            (
                Request::new_http(),
                "vars.set('a', nil)",
                "the value is nil",
            ),
        ];
        for (mut req, code, want) in cases {
            let e = run_lua_pre(&own(code), &mut req, &mut s).unwrap_err();
            assert!(e.contains(want), "{code}: {e}");
        }
        // The fields each kind has.
        let mut g = Request::Grpc(GrpcRequest::default());
        run_lua_pre(
            &own(r#"request.url = "h:1"; request:set_header("x-id", 1); request.body = '{"a":1}'"#),
            &mut g,
            &mut s,
        )
        .unwrap();
        let Request::Grpc(g) = g else { panic!() };
        assert_eq!(
            (g.target.as_str(), g.message.as_str()),
            ("h:1", r#"{"a":1}"#)
        );
        assert_eq!(g.metadata, vec![KeyValue::new("x-id", "1")]);
        let mut m = Request::Mcp(McpRequest::default());
        run_lua_pre(&own(r#"request.body = '{"q":2}'"#), &mut m, &mut s).unwrap();
        let Request::Mcp(m) = m else { panic!() };
        assert_eq!(m.arguments, r#"{"q":2}"#);
    }

    #[test]
    fn the_sandbox_stops_loops_and_has_no_files_or_loaders() {
        let mut s = scope();
        let e = run_lua_pre(&own("while true do end"), &mut http(), &mut s).unwrap_err();
        assert!(e.contains("instructions"), "{e}");
        let e = run_lua_pre(
            &own("local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1000) end"),
            &mut http(),
            &mut s,
        )
        .unwrap_err();
        assert!(e.contains("memory") || e.contains("instructions"), "{e}");
        for code in [
            "io.open('x')",
            "os.execute('true')",
            "require('x')",
            "dofile('x')",
            "loadfile('x')",
            "load('return 1')",
            "debug.getinfo(1)",
            "string.dump(print)",
            "package.loadlib('x', 'y')",
        ] {
            assert!(
                run_lua_pre(&own(code), &mut http(), &mut s).is_err(),
                "{code}"
            );
        }
        // The limit is per script: a fresh budget for the next one.
        let scripts = vec![
            LuaScript {
                origin: String::new(),
                code: "for i = 1, 3000000 do end".into(),
            },
            LuaScript {
                origin: String::new(),
                code: "for i = 1, 3000000 do end".into(),
            },
        ];
        assert!(run_lua_pre(&scripts, &mut http(), &mut s).is_ok());
    }

    #[test]
    fn blank_scripts_do_nothing() {
        let mut s = scope();
        let mut req = http();
        assert_eq!(
            run_lua_pre(&own("  \n"), &mut req, &mut s),
            Ok(LuaOut::default())
        );
        assert_eq!(req, http());
    }

    #[test]
    fn post_reads_the_response_and_records_tests() {
        let mut s = scope();
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        let timing = [("total", Duration::from_millis(12))];
        let view = ResponseView {
            status: 201,
            headers: &headers,
            body: br#"{"token":"abc","items":[1,2,3]}"#,
            timing: &timing,
        };
        let scripts = vec![
            LuaScript {
                origin: "folder /".into(),
                code: r#"test("created", response.status == 201)"#.into(),
            },
            LuaScript {
                origin: String::new(),
                code: r#"
                    local j = response:json()
                    test("three items", #j.items == 3)
                    test("json header", response:header("content-type") == "application/json")
                    test("fast", response:time("total") < 100)
                    test("no ttfb", response:time("ttfb") == nil)
                    test("headers", response.headers[1].name == "Content-Type")
                    test("body", response.body:find("token") ~= nil)
                    test("fails", false)
                    vars.set_secret("token", j.token)
                "#
                .into(),
            },
            LuaScript {
                origin: String::new(),
                code: "response:time(\"lag\")".into(),
            },
        ];
        let out = run_lua_post(&scripts, &view, &mut s);
        let got: Vec<_> = out
            .tests
            .iter()
            .map(|t| (t.label.as_str(), t.pass))
            .collect();
        assert_eq!(
            got[..8],
            [
                ("test: created", true),
                ("test: three items", true),
                ("test: json header", true),
                ("test: fast", true),
                ("test: no ttfb", true),
                ("test: headers", true),
                ("test: body", true),
                ("test: fails", false),
            ]
        );
        // The script error is a failed test with the line; the token is a secret.
        assert_eq!(out.tests[8].label, "request script");
        assert!(!out.tests[8].pass);
        assert!(
            out.tests[8]
                .actual
                .starts_with("request script:1: response:time"),
            "{}",
            out.tests[8].actual
        );
        assert_eq!(
            s.lookup("token").unwrap(),
            crate::vars::Rendered {
                text: "abc".into(),
                secret: true,
                unresolved: vec![],
            }
        );
    }

    #[test]
    fn post_has_no_request_and_pre_has_no_response() {
        let mut s = scope();
        let view = ResponseView {
            status: 200,
            headers: &[],
            body: b"",
            timing: &[],
        };
        let out = run_lua_post(&own("local u = request.url"), &view, &mut s);
        assert!(!out.tests[0].pass);
        assert!(run_lua_pre(&own("local s = response.status"), &mut http(), &mut s).is_err());
        assert!(run_lua_pre(&own("test('a', true)"), &mut http(), &mut s).is_err());
    }

    fn catalog(code: &str) -> LuaCatalog {
        let (c, errors) = LuaCatalog::new(vec![LuaScript {
            origin: "lua/util.lua".into(),
            code: code.into(),
        }]);
        assert!(errors.is_empty(), "{errors:?}");
        c
    }

    #[test]
    fn catalog_runs_before_the_scripts_of_both_phases() {
        let mut s = scope();
        s.set_catalog(std::sync::Arc::new(catalog(
            "function shout(t) return t:upper() end\nlocal function hidden() end",
        )));
        let mut req = http();
        run_lua_pre(
            &own(r#"vars.set("a", shout("x")) vars.set("r", render("{{ $lua.shout(host) }}"))"#),
            &mut req,
            &mut s,
        )
        .unwrap();
        assert_eq!(s.lookup("a").unwrap().text, "X");
        assert_eq!(s.lookup("r").unwrap().text, "EXAMPLE.COM");
        let body = b"{}";
        let resp = ResponseView {
            status: 200,
            headers: &[],
            body,
            timing: &[],
        };
        let out = run_lua_post(&own(r#"test("t", shout("y") == "Y")"#), &resp, &mut s);
        assert!(out.tests.iter().all(|t| t.pass), "{:?}", out.tests);
        assert!(s.catalog().has("shout"));
        assert!(!s.catalog().has("hidden"));
        assert!(!s.catalog().has("pairs"));
        // No script: the catalog does not run.
        assert_eq!(run_lua_pre(&[], &mut req, &mut s), Ok(LuaOut::default()));
    }

    #[test]
    fn catalog_files_that_fail_are_left_out() {
        let file = |name: &str, code: &str| LuaScript {
            origin: format!("lua/{name}"),
            code: code.into(),
        };
        let (c, errors) = LuaCatalog::new(vec![
            file("a.lua", "function a() return 1 end"),
            file("bad.lua", "function b("),
            file("hash.lua", "function hash() return 1 end"),
            file("c.lua", "function c() return a() end"),
        ]);
        assert!(c.has("a") && c.has("c") && !c.has("b") && !c.has("hash"));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].starts_with("lua/bad.lua script:"), "{errors:?}");
        assert!(errors[1].contains("`hash`"), "{errors:?}");
        // A pre script names the catalog file that fails at run time.
        let (c, _) = LuaCatalog::new(vec![file(
            "env.lua",
            "if request then error('no request') end",
        )]);
        let mut s = scope();
        s.set_catalog(std::sync::Arc::new(c));
        let e = run_lua_pre(&own("vars.set('a', 1)"), &mut http(), &mut s).unwrap_err();
        assert!(e.starts_with("lua/env.lua script:1:"), "{e}");
    }
}
