//! Variable scopes and template rendering.
//!
//! A [`Scope`] is a stack of layers (collection env < active env < runtime).
//! Later layers override earlier ones. Values may themselves reference other
//! variables; references are resolved recursively up to [`MAX_DEPTH`].
//!
//! Every value carries a `secret` flag. A rendered string is secret when any
//! secret value contributed to it, which lets callers mask derived values too.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::funcs::{self, Arg, Call};
use crate::lua_script::{self, LuaCatalog};
use crate::template::{self, Segment};

pub const MAX_DEPTH: usize = 8;

/// The longest text one render produces. Values that reference each other
/// several times grow fast (8 levels of 10 references is 10^8 copies); a
/// reference that would go past this limit stays unresolved.
pub const MAX_RENDERED: usize = 1024 * 1024;

/// Results of one top-level render, by (name, depth): a value referenced many
/// times is resolved once.
type Memo = HashMap<(String, usize), Option<Rendered>>;

/// Placeholder shown instead of secret values.
pub const MASK: &str = "••••••";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarValue {
    pub value: String,
    pub secret: bool,
}

impl VarValue {
    pub fn plain(v: impl Into<String>) -> Self {
        Self {
            value: v.into(),
            secret: false,
        }
    }

    pub fn secret(v: impl Into<String>) -> Self {
        Self {
            value: v.into(),
            secret: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Layer {
    pub name: String,
    pub vars: BTreeMap<String, VarValue>,
}

/// Result of rendering a template.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    pub text: String,
    /// True if a secret value contributed to `text`.
    pub secret: bool,
    /// Names that could not be resolved (left literally in `text`).
    pub unresolved: Vec<String>,
}

#[derive(Debug)]
pub struct Scope {
    /// A number for each new scope; a clone keeps it. The Lua state of the
    /// template calls is kept for one id (one send).
    id: u64,
    layers: Vec<Layer>,
    /// Dynamic values (`$uuid`, ...) generated once per scope so a request and its
    /// history entry agree.
    dynamic: RefCell<HashMap<String, String>>,
    /// The results of random and Lua function calls, by call text, so every
    /// render of one send gives the same value.
    kept: RefCell<HashMap<String, Rendered>>,
    /// The user's Lua functions.
    catalog: Arc<LuaCatalog>,
    /// The secret values are empty stand-ins (see [`Self::set_names_only`]).
    names_only: bool,
    /// A pre-send call with no kept value counts as resolved (see
    /// [`Self::set_pre_send_deferred`]).
    deferred: bool,
    /// The text of each pre-send call that a render met with no kept value,
    /// in order.
    pending: RefCell<Vec<String>>,
}

impl Default for Scope {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            layers: Vec::new(),
            dynamic: RefCell::default(),
            kept: RefCell::default(),
            catalog: Arc::default(),
            names_only: false,
            deferred: false,
            pending: RefCell::default(),
        }
    }
}

impl Clone for Scope {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            layers: self.layers.clone(),
            dynamic: RefCell::new(self.dynamic.borrow().clone()),
            kept: RefCell::new(self.kept.borrow().clone()),
            catalog: self.catalog.clone(),
            names_only: self.names_only,
            deferred: self.deferred,
            pending: RefCell::new(self.pending.borrow().clone()),
        }
    }
}

/// Names of built-in dynamic variables.
pub const DYNAMIC_VARS: &[(&str, &str)] = &[
    ("$uuid", "random UUID v4"),
    ("$timestamp", "Unix time in seconds"),
    ("$timestampMs", "Unix time in milliseconds"),
    ("$isoTimestamp", "current UTC time, RFC 3339"),
    ("$randomInt", "random integer 0-1000"),
];

impl Scope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a layer on top (highest precedence).
    pub fn push_layer(&mut self, name: impl Into<String>, vars: BTreeMap<String, VarValue>) {
        self.layers.push(Layer {
            name: name.into(),
            vars,
        });
    }

    pub fn with_layer(mut self, name: impl Into<String>, vars: BTreeMap<String, VarValue>) -> Self {
        self.push_layer(name, vars);
        self
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn set_catalog(&mut self, catalog: Arc<LuaCatalog>) {
        self.catalog = catalog;
    }

    pub fn catalog(&self) -> &Arc<LuaCatalog> {
        &self.catalog
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Marks a scope whose secret values are empty stand-ins, for the checks
    /// that know the names only. A call with a secret argument and a Lua call
    /// then do not run: they count as resolved, with an empty secret value.
    pub fn set_names_only(&mut self) {
        self.names_only = true;
    }

    /// Marks a scope for the checks before a send: a pre-send call
    /// (`$prompt`, `$select`, `$response`) with no kept value counts as
    /// resolved, and [`Self::pre_send_pending`] lists it.
    pub fn set_pre_send_deferred(&mut self) {
        self.deferred = true;
    }

    /// The pre-send calls that a render of this scope met with no kept value,
    /// as written (`$prompt('OTP')`), in order.
    pub fn pre_send_pending(&self) -> Vec<String> {
        self.pending.borrow().clone()
    }

    /// Gives the value of the call written as `call_text`: the pipeline
    /// resolves a pre-send call before the render. The value stays for the
    /// whole send.
    pub fn keep(&self, call_text: &str, value: Rendered) {
        self.kept.borrow_mut().insert(call_text.to_string(), value);
    }

    /// Why the call `name` (a function call placeholder) does not resolve;
    /// `None` when it resolves or is not a call. Secret values are masked.
    pub fn call_error(&self, name: &str) -> Option<String> {
        let c = funcs::parse(name)?;
        self.call(&c, 0, &mut Memo::new()).err()
    }

    /// Inserts `layers` just below the first layer named `above` (on top
    /// when there is none), keeping their order.
    pub fn insert_below(&mut self, above: &str, layers: Vec<Layer>) {
        let at = self
            .layers
            .iter()
            .position(|l| l.name == above)
            .unwrap_or(self.layers.len());
        self.layers.splice(at..at, layers);
    }

    /// Sets a variable in the top layer named `layer`, creating it if needed.
    pub fn set(&mut self, layer: &str, name: impl Into<String>, value: VarValue) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.name == layer) {
            l.vars.insert(name.into(), value);
        } else {
            let mut vars = BTreeMap::new();
            vars.insert(name.into(), value);
            self.push_layer(layer, vars);
        }
    }

    /// Rendered values of every secret variable, longest first; used to mask
    /// secrets that come back in free-form output such as response bodies.
    pub fn secret_values(&self) -> Vec<String> {
        let mut names: Vec<&str> = self
            .layers
            .iter()
            .flat_map(|l| {
                l.vars
                    .iter()
                    .filter(|(_, v)| v.secret)
                    .map(|(k, _)| k.as_str())
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        let mut out: Vec<String> = Vec::new();
        for n in names {
            if let Some(r) = self.lookup(n)
                && r.secret
                && r.text.len() >= 3
                && !out.contains(&r.text)
            {
                out.push(r.text);
            }
        }
        // Values the send kept for a call (a secret `$prompt`, `$response`,
        // a Lua call that read a secret) are secrets too.
        for r in self.kept.borrow().values() {
            if r.secret && r.text.len() >= 3 && !out.contains(&r.text) {
                out.push(r.text.clone());
            }
        }
        // Process environment values that a render read are secrets too.
        for (k, v) in self.dynamic.borrow().iter() {
            if k.starts_with(&PROCESS_ENV[1..]) && v.len() >= 3 && !out.contains(v) {
                out.push(v.clone());
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.len()));
        out
    }

    /// The raw (unrendered) value and the layer it comes from.
    pub fn raw(&self, name: &str) -> Option<(&VarValue, &str)> {
        self.layers
            .iter()
            .rev()
            .find_map(|l| l.vars.get(name).map(|v| (v, l.name.as_str())))
    }

    pub fn is_defined(&self, name: &str) -> bool {
        if let Some(k) = process_env_key(name) {
            return std::env::var_os(k).is_some();
        }
        if let Some(c) = funcs::parse(name) {
            return self.call_defined(&c);
        }
        name.starts_with('$') && dynamic_known(name) || self.raw(name).is_some()
    }

    /// The function exists, gets the right number of arguments, and every
    /// argument is defined.
    /// For `$default`, only the fallback must be defined.
    fn call_defined(&self, c: &Call) -> bool {
        let arg_defined = |a: &Arg| match a {
            Arg::Text(_) => true,
            Arg::Name(n) => self.is_defined(n),
            Arg::Call(c) => self.call_defined(c),
        };
        if let Some(p) = funcs::pre_send(c) {
            return p.is_ok()
                && (self.names_only || self.deferred || self.kept.borrow().contains_key(c.text));
        }
        let known = match c.name.strip_prefix(funcs::LUA) {
            Some(name) => self.catalog.has(name),
            None => funcs::find(c.name).is_some_and(|f| f.arity() == c.args.len()),
        };
        if !known {
            return false;
        }
        match c.args.as_slice() {
            [_, fallback] if c.name == funcs::DEFAULT => arg_defined(fallback),
            args => args.iter().all(arg_defined),
        }
    }

    /// Fully resolves one variable.
    pub fn lookup(&self, name: &str) -> Option<Rendered> {
        self.lookup_depth(name, 0, &mut Memo::new())
    }

    /// [`Self::lookup`] at `depth`, for a Lua function that a template calls.
    pub(crate) fn lookup_at(&self, name: &str, depth: usize) -> Option<Rendered> {
        self.lookup_depth(name, depth, &mut Memo::new())
    }

    fn lookup_depth(&self, name: &str, depth: usize, memo: &mut Memo) -> Option<Rendered> {
        let key = (name.to_string(), depth);
        if let Some(r) = memo.get(&key) {
            return r.clone();
        }
        let r = self.lookup_uncached(name, depth, memo);
        memo.insert(key, r.clone());
        r
    }

    fn lookup_uncached(&self, name: &str, depth: usize, memo: &mut Memo) -> Option<Rendered> {
        if let Some(k) = process_env_key(name) {
            return self.process_env(k).map(|text| Rendered {
                text,
                secret: true,
                unresolved: vec![],
            });
        }
        if let Some(c) = funcs::parse(name) {
            return self.call(&c, depth, memo).ok();
        }
        if let Some(rest) = name.strip_prefix('$')
            && let Some(v) = self.dynamic_value(rest)
        {
            return Some(Rendered {
                text: v,
                secret: false,
                unresolved: vec![],
            });
        }
        let (v, _) = self.raw(name)?;
        if depth >= MAX_DEPTH {
            return None;
        }
        let mut r = self.render_depth(&v.value, depth + 1, memo);
        r.secret |= v.secret;
        Some(r)
    }

    /// Runs a template function: a built-in one, or `$lua.name` from the
    /// catalog. An unknown function, a wrong number of arguments, an
    /// unresolved argument or an error gives the reason, with the secret
    /// values masked. The result is secret when an argument is, or when the
    /// Lua function read a secret. A random or Lua function keeps its first
    /// result for each call text, as `$uuid` does.
    fn call(&self, c: &Call, depth: usize, memo: &mut Memo) -> Result<Rendered, String> {
        if let Some(r) = self.kept.borrow().get(c.text) {
            return Ok(r.clone());
        }
        if let Some(p) = funcs::pre_send(c) {
            p?;
            if !(self.names_only || self.deferred) {
                return Err("it needs a value before the send".into());
            }
            let mut pending = self.pending.borrow_mut();
            if !pending.iter().any(|t| t == c.text) {
                pending.push(c.text.to_string());
            }
            return Ok(Rendered {
                secret: true,
                ..Default::default()
            });
        }
        let lua = c.name.strip_prefix(funcs::LUA);
        let builtin = match lua {
            Some(name) if !self.catalog.has(name) => {
                return Err(format!("the Lua catalog has no function `{name}`"));
            }
            Some(_) => None,
            None => {
                let f = funcs::find(c.name).ok_or("unknown function")?;
                if f.arity() != c.args.len() {
                    let plural = if f.arity() == 1 { "" } else { "s" };
                    return Err(format!(
                        "it takes {} argument{plural}, not {}",
                        f.arity(),
                        c.args.len()
                    ));
                }
                Some(f)
            }
        };
        if let [first, fallback] = c.args.as_slice()
            && c.name == funcs::DEFAULT
        {
            return self
                .call_arg(first, depth, memo)
                .or_else(|_| self.call_arg(fallback, depth, memo));
        }
        let mut args = Vec::with_capacity(c.args.len());
        let mut secret_args = Vec::new();
        for a in &c.args {
            let r = self.call_arg(a, depth, memo)?;
            if r.secret {
                secret_args.push(r.text.clone());
            }
            args.push(r.text);
        }
        let secret = !secret_args.is_empty();
        if self.names_only && (lua.is_some() || secret) {
            return Ok(Rendered {
                secret: true,
                ..Default::default()
            });
        }
        let result = match builtin {
            Some(f) => (f.apply)(&args).map(|text| (text, false, f.random)),
            None if depth >= MAX_DEPTH => Err("the calls nest too deep".into()),
            None => lua_script::call_catalog(self, &c.name[funcs::LUA.len()..], &args, depth)
                .map(|(text, read)| (text, read, true)),
        };
        // Not `secret_values`: it renders the secrets, which can run this
        // call again. A Lua error comes masked with the secrets it read.
        let (text, read_secret, keep) = result.map_err(|e| scrub(&e, &secret_args))?;
        let r = Rendered {
            text,
            secret: secret || read_secret,
            unresolved: vec![],
        };
        if keep {
            self.kept.borrow_mut().insert(c.text.to_string(), r.clone());
        }
        Ok(r)
    }

    /// One argument of a call, or why it does not fully resolve.
    fn call_arg(&self, a: &Arg, depth: usize, memo: &mut Memo) -> Result<Rendered, String> {
        let r = match a {
            Arg::Text(t) => Rendered {
                text: t.to_string(),
                ..Default::default()
            },
            Arg::Name(n) => self
                .lookup_depth(n, depth, memo)
                .ok_or_else(|| format!("`{n}` is not defined"))?,
            Arg::Call(c) => self
                .call(c, depth, memo)
                .map_err(|e| format!("{}: {e}", c.name))?,
        };
        match r.unresolved.first() {
            Some(u) => Err(format!("`{u}` is not resolved")),
            None => Ok(r),
        }
    }

    fn dynamic_value(&self, name: &str) -> Option<String> {
        if let Some(v) = self.dynamic.borrow().get(name) {
            return Some(v.clone());
        }
        let v = generate_dynamic(name)?;
        self.dynamic
            .borrow_mut()
            .insert(name.to_string(), v.clone());
        Some(v)
    }

    /// Reads `key` from the process environment once per scope. A variable
    /// that is not set or is not UTF-8 gives `None`.
    fn process_env(&self, key: &str) -> Option<String> {
        let slot = format!("{}{key}", &PROCESS_ENV[1..]);
        if let Some(v) = self.dynamic.borrow().get(&slot) {
            return Some(v.clone());
        }
        let v = std::env::var(key).ok()?;
        self.dynamic.borrow_mut().insert(slot, v.clone());
        Some(v)
    }

    /// Renders a template, replacing every resolvable placeholder.
    pub fn render(&self, s: &str) -> Rendered {
        self.render_depth(s, 0, &mut Memo::new())
    }

    /// [`Self::render`] at `depth`, for a Lua function that a template calls.
    pub(crate) fn render_at(&self, s: &str, depth: usize) -> Rendered {
        self.render_depth(s, depth, &mut Memo::new())
    }

    fn render_depth(&self, s: &str, depth: usize, memo: &mut Memo) -> Rendered {
        if !s.contains("{{") {
            return Rendered {
                text: s.to_string(),
                ..Default::default()
            };
        }
        let mut out = Rendered::default();
        for seg in template::parse(s) {
            match seg {
                Segment::Text(t) => out.text.push_str(t),
                Segment::Var { name, span } => match self.lookup_depth(name, depth, memo) {
                    Some(r) if out.text.len() + r.text.len() <= MAX_RENDERED => {
                        out.text.push_str(&r.text);
                        out.secret |= r.secret;
                        for u in r.unresolved {
                            push_unique(&mut out.unresolved, u);
                        }
                    }
                    _ => {
                        out.text.push_str(&s[span]);
                        push_unique(&mut out.unresolved, name.to_string());
                    }
                },
            }
        }
        out
    }

    /// Renders a template for storage or display where secrets must not appear:
    /// placeholders whose resolved value is secret are kept as `{{name}}`.
    pub fn render_masked(&self, s: &str) -> String {
        if !s.contains("{{") {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len());
        for seg in template::parse(s) {
            match seg {
                Segment::Text(t) => out.push_str(t),
                Segment::Var { name, span } => match self.lookup(name) {
                    Some(r) if !r.secret => out.push_str(&r.text),
                    _ => out.push_str(&s[span]),
                },
            }
        }
        out
    }

    /// All unresolved variable names referenced by `s`.
    pub fn unresolved(&self, s: &str) -> Vec<String> {
        self.render(s).unresolved
    }

    /// Every variable name visible in the scope (for completion and listing).
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .layers
            .iter()
            .flat_map(|l| l.vars.keys().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

fn dynamic_known(name: &str) -> bool {
    DYNAMIC_VARS.iter().any(|(n, _)| *n == name)
}

/// Prefix of `{{$processEnv.NAME}}`, which reads `NAME` from the process
/// environment. The value is a secret.
const PROCESS_ENV: &str = "$processEnv.";

/// The variable name of a `$processEnv.NAME` placeholder (not empty).
fn process_env_key(name: &str) -> Option<&str> {
    name.strip_prefix(PROCESS_ENV).filter(|k| !k.is_empty())
}

/// True for a built-in name: a dynamic value or `$processEnv.NAME`.
pub fn is_builtin(name: &str) -> bool {
    dynamic_known(name) || process_env_key(name).is_some()
}

fn generate_dynamic(name: &str) -> Option<String> {
    let now = time::OffsetDateTime::now_utc();
    Some(match name {
        "uuid" => uuid_v4(),
        "timestamp" => now.unix_timestamp().to_string(),
        "timestampMs" => (now.unix_timestamp_nanos() / 1_000_000).to_string(),
        "isoTimestamp" => now
            .format(&time::format_description::well_known::Rfc3339)
            .ok()?,
        "randomInt" => fastrand::u32(0..=1000).to_string(),
        _ => return None,
    })
}

/// A random (version 4) UUID string.
pub fn uuid_v4() -> String {
    funcs::uuid_text(&funcs::with_version(fastrand::u128(..).to_le_bytes(), 4))
}

/// The spellings a secret may come back in: as sent, inside a JSON string, and
/// percent-encoded as a form field or a query component. Secrets shorter than
/// 3 characters are left out (they would mask too much). Longest first.
fn mask_forms(secrets: &[String]) -> Vec<String> {
    let mut forms: Vec<String> = secrets
        .iter()
        .filter(|s| s.len() >= 3)
        .flat_map(|s| {
            let mut forms = vec![s.clone(), funcs::json_escape(s)];
            for encoded in [
                crate::resolve::form_urlencode(s),
                crate::resolve::query_encode(s),
            ] {
                forms.push(encoded.to_ascii_lowercase());
                forms.push(encoded);
            }
            forms
        })
        .collect();
    forms.sort_by_key(|s| std::cmp::Reverse(s.len()));
    forms.dedup();
    forms
}

/// Masks every occurrence of the given secret values in `b`, in any of the
/// spellings a server may echo them in. Returns `None` when nothing matched, so
/// large bodies are not copied needlessly.
pub fn scrub_bytes(b: &[u8], secrets: &[String]) -> Option<Vec<u8>> {
    let mut out: Option<Vec<u8>> = None;
    for form in mask_forms(secrets) {
        if let Some(r) = replace_bytes(
            out.as_deref().unwrap_or(b),
            form.as_bytes(),
            MASK.as_bytes(),
        ) {
            out = Some(r);
        }
    }
    out
}

fn find_bytes(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    let first = *n.first()?;
    let mut i = from;
    while i + n.len() <= h.len() {
        let p = h
            .get(i..h.len() + 1 - n.len())?
            .iter()
            .position(|&c| c == first)?
            + i;
        if h.get(p..p + n.len()) == Some(n) {
            return Some(p);
        }
        i = p + 1;
    }
    None
}

fn replace_bytes(h: &[u8], n: &[u8], with: &[u8]) -> Option<Vec<u8>> {
    let mut at = find_bytes(h, n, 0)?;
    let mut out = Vec::with_capacity(h.len());
    let mut i = 0;
    loop {
        out.extend_from_slice(&h[i..at]);
        out.extend_from_slice(with);
        i = at + n.len();
        match find_bytes(h, n, i) {
            Some(p) => at = p,
            None => break,
        }
    }
    out.extend_from_slice(&h[i..]);
    Some(out)
}

/// Masks every occurrence of the given secret values in `s` (used as a last line
/// of defence when writing free-form text such as error messages).
pub fn scrub(s: &str, secrets: &[String]) -> String {
    let mut out = s.to_string();
    for form in mask_forms(secrets) {
        out = out.replace(form.as_str(), MASK);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_that_reference_each_other_many_times_render_fast_and_bounded() {
        // v0 = "x"; vN = ten references to vN-1: 10^7 copies of "x" in v7.
        let mut vars = BTreeMap::new();
        vars.insert("v0".to_string(), VarValue::plain("x"));
        for n in 1..8 {
            vars.insert(
                format!("v{n}"),
                VarValue::plain(format!("{{{{v{}}}}}", n - 1).repeat(10)),
            );
        }
        let s = Scope::new().with_layer("env", vars);
        let started = std::time::Instant::now();
        let r = s.render("{{v5}}");
        assert_eq!(r.text.len(), 100_000);
        assert!(r.unresolved.is_empty());
        let r = s.render("{{v7}}");
        assert!(r.text.len() <= MAX_RENDERED + 64, "{}", r.text.len());
        assert!(r.unresolved.iter().any(|u| u.starts_with('v')));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn secret_values_and_scrub_bytes() {
        let s = scope();
        assert_eq!(s.secret_values(), vec!["s3cret".to_string()]);
        let body = br#"{"auth":"Bearer s3cret","x":"s3cret!"}"#;
        let out = scrub_bytes(body, &s.secret_values()).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(r#"{{"auth":"Bearer {MASK}","x":"{MASK}!"}}"#)
        );
        assert!(scrub_bytes(b"nothing here", &s.secret_values()).is_none());
        assert!(scrub_bytes(b"s3", &s.secret_values()).is_none());
    }

    fn scope() -> Scope {
        let mut coll = BTreeMap::new();
        coll.insert("host".into(), VarValue::plain("example.com"));
        coll.insert("base".into(), VarValue::plain("https://{{host}}"));
        coll.insert("env".into(), VarValue::plain("collection"));
        let mut env = BTreeMap::new();
        env.insert("env".into(), VarValue::plain("dev"));
        env.insert("token".into(), VarValue::secret("s3cret"));
        env.insert("auth".into(), VarValue::plain("Bearer {{token}}"));
        env.insert("loop".into(), VarValue::plain("{{loop}}"));
        Scope::new()
            .with_layer("collection", coll)
            .with_layer("dev", env)
    }

    #[test]
    fn render_nested_and_override() {
        let s = scope();
        let r = s.render("{{base}}/{{env}}");
        assert_eq!(r.text, "https://example.com/dev");
        assert!(!r.secret);
        assert!(r.unresolved.is_empty());
        assert_eq!(s.raw("env").map(|(_, l)| l), Some("dev"));
    }

    #[test]
    fn secrets_taint() {
        let s = scope();
        let r = s.render("{{auth}}");
        assert_eq!(r.text, "Bearer s3cret");
        assert!(r.secret);
        assert_eq!(
            s.render_masked("x {{auth}} {{host}}"),
            "x {{auth}} example.com"
        );
        assert_eq!(s.render_masked("plain"), "plain");
    }

    #[test]
    fn unresolved_and_cycles() {
        let s = scope();
        let r = s.render("{{nope}}-{{loop}}-{{nope}}");
        assert_eq!(r.text, "{{nope}}-{{loop}}-{{nope}}");
        assert_eq!(r.unresolved, vec!["nope".to_string(), "loop".to_string()]);
        assert_eq!(s.unresolved("{{host}}"), Vec::<String>::new());
        assert!(!s.is_defined("nope"));
        assert!(s.is_defined("host"));
        assert!(s.is_defined("$uuid"));
        assert!(!s.is_defined("$nope"));
        assert_eq!(s.render("{{$nope}}").unresolved, vec!["$nope".to_string()]);
    }

    #[test]
    fn dynamic_stable_within_scope() {
        let s = scope();
        let a = s.render("{{$uuid}}").text;
        let b = s.render("{{$uuid}}").text;
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        let ts: i64 = s.render("{{$timestamp}}").text.parse().unwrap_or(0);
        assert!(ts > 1_600_000_000);
        assert!(s.render("{{$isoTimestamp}}").text.ends_with('Z'));
        let n: u32 = s.render("{{$randomInt}}").text.parse().unwrap_or(9999);
        assert!(n <= 1000);
        assert!(s.render("{{$timestampMs}}").text.len() >= 13);
        let c = s.clone();
        assert_eq!(c.render("{{$uuid}}").text, a);
    }

    #[test]
    fn functions_render() {
        let mut s = scope();
        s.set(
            "dev",
            "id",
            VarValue::plain("1b4e28ba-2fa1-11d2-883f-0016d3cca427"),
        );
        let bytes = "G04oui+hEdKIPwAW08ykJw==";
        assert_eq!(s.render("{{ $uuidBytes(id) }}").text, bytes);
        assert_eq!(
            s.render("{{$uuidBytes('1b4e28ba-2fa1-11d2-883f-0016d3cca427')}}")
                .text,
            bytes
        );
        assert_eq!(s.render("{{ $upper($trim(' a ')) }}-{{env}}").text, "A-dev");
        // The same $uuid as a plain placeholder in the same scope.
        let uuid = s.render("{{$uuid}}").text;
        let want = s.render(&format!("{{{{$uuidBytes('{uuid}')}}}}")).text;
        assert_eq!(s.render("{{$uuidBytes($uuid)}}").text, want);
        assert!(s.is_defined("$uuidBytes(id)"));
        assert!(s.is_defined("$sha256($base64('x'))"));
    }

    #[test]
    fn functions_keep_secrets_and_fail_unresolved() {
        let s = scope();
        let r = s.render("{{ $base64(token) }}");
        assert_eq!(r.text, "czNjcmV0");
        assert!(r.secret);
        assert_eq!(
            s.render_masked("{{ $base64(token) }}"),
            "{{ $base64(token) }}"
        );
        assert_eq!(s.render_masked("{{$base64('x')}}"), "eA==");
        for bad in [
            "$nope(env)",
            "$base64(env, env)",
            "$base64(missing)",
            "$uuidBytes(env)",
            "$base64(loop)",
        ] {
            let t = format!("{{{{{bad}}}}}");
            let r = s.render(&t);
            assert_eq!(r.text, t);
            assert_eq!(r.unresolved, vec![bad.to_string()], "{bad}");
        }
        assert!(!s.is_defined("$nope(env)"));
        assert!(!s.is_defined("$base64(env, env)"));
        assert!(!s.is_defined("$base64(missing)"));
        assert!(!s.is_defined("$base64($nope('x'))"));
    }

    #[test]
    fn functions_default_and_random() {
        let s = scope();
        assert_eq!(s.render("{{ $default(missing, 'x') }}").text, "x");
        assert_eq!(s.render("{{ $default(env, 'x') }}").text, "dev");
        assert_eq!(s.render("{{ $default($uuidBytes(env), 'x') }}").text, "x");
        assert_eq!(
            s.render("{{ $default(missing, other) }}").unresolved.len(),
            1
        );
        assert!(s.is_defined("$default(missing, 'x')"));
        assert!(!s.is_defined("$default('x', missing)"));
        assert_eq!(s.render("{{ $substr('abcdef', 1, 2) }}").text, "bc");
        // A random call keeps its value for the scope, also inside a call.
        let a = s.render("{{$randomHex(16)}}").text;
        assert_eq!(a.len(), 16);
        assert_eq!(s.render("{{$randomHex(16)}}").text, a);
        assert_eq!(
            s.render("{{ $upper($randomHex(16)) }}").text,
            a.to_uppercase()
        );
        assert_eq!(s.clone().render("{{$randomHex(16)}}").text, a);
        assert!(s.secret_values().iter().all(|v| *v != a));
    }

    #[test]
    fn functions_from_the_lua_catalog() {
        let (catalog, errors) = LuaCatalog::new(vec![crate::model::LuaScript {
            origin: "lua/t.lua".into(),
            code: r#"
                function greet(name) return "hi " .. name end
                function num() return 1.5 end
                function tbl() return {b = 1, a = "x"} end
                function none() return nil end
                function boom() error("bad") end
                function setter() vars.set("a", "b") return "x" end
                function tok() return vars.get("token") end
                function host() return render("{{host}}") .. tostring(vars.has("nope")) end
                function again() return render("{{ $lua.again() }}") end
                function rnd() return tostring(math.random(1, 1000000000)) end
                function noisy() print("x") return "ok" end
                function count() n = (n or 0) + 1 return n end
                function leak() error("no " .. vars.get("token")) end
                local function hidden() return 1 end
            "#
            .into(),
        }]);
        assert!(errors.is_empty(), "{errors:?}");
        let mut s = scope();
        s.set_catalog(Arc::new(catalog));
        let text = |t: &str| s.render(t).text;
        assert_eq!(text("{{ $lua.greet(env) }}"), "hi dev");
        assert_eq!(text("{{ $lua.greet($upper('a')) }}"), "hi A");
        assert_eq!(text("{{$lua.num()}}"), "1.5");
        assert_eq!(text("{{$lua.tbl()}}"), r#"{"a":"x","b":1}"#);
        assert_eq!(text("{{$lua.host()}}"), "example.comfalse");
        assert_eq!(text("{{$lua.noisy()}}"), "ok");
        for t in [
            "{{$lua.none()}}",
            "{{$lua.boom()}}",
            "{{$lua.setter()}}",
            "{{$lua.again()}}",
            "{{$lua.hidden()}}",
            "{{$lua.nope()}}",
            "{{$lua.greet(missing)}}",
        ] {
            assert_eq!(s.render(t).unresolved.len(), 1, "{t}");
        }
        // A secret argument or a secret that the function reads is secret.
        for t in ["{{ $lua.greet(token) }}", "{{$lua.tok()}}"] {
            assert!(s.render(t).secret, "{t}");
            assert_eq!(s.render_masked(t), t);
        }
        assert_eq!(text("{{$lua.tok()}}"), "s3cret");
        assert!(!s.render("{{$lua.greet('a')}}").secret);
        // One value per call text, also in a clone.
        let a = text("{{$lua.rnd()}}");
        assert_eq!(text("{{$lua.rnd()}}"), a);
        assert_eq!(s.clone().render("{{$lua.rnd()}}").text, a);
        assert!(s.is_defined("$lua.greet(env)"));
        assert!(!s.is_defined("$lua.greet(missing)"));
        assert!(!s.is_defined("$lua.hidden()"));
        assert!(!s.is_defined("$lua.nope()"));
        // The calls of one scope share a Lua state; a new scope starts again.
        assert_eq!(text("{{$lua.count('a')}}"), "1");
        assert_eq!(text("{{$lua.count('b')}}"), "2");
        let mut other = Scope::new();
        other.set_catalog(s.catalog().clone());
        assert_eq!(other.render("{{$lua.count('a')}}").text, "1");
        // The reasons, with the secrets masked.
        let why = |t: &str| s.call_error(t).unwrap_or_default();
        assert!(
            why("$lua.boom()").starts_with("lua/t.lua script:"),
            "{}",
            why("$lua.boom()")
        );
        assert!(why("$lua.boom()").ends_with("bad"));
        assert_eq!(why("$lua.nope()"), "the Lua catalog has no function `nope`");
        assert_eq!(
            why("$lua.leak()"),
            format!("lua/t.lua script:14: no {MASK}")
        );
        assert_eq!(s.call_error("$lua.greet('a')"), None);
    }

    #[test]
    fn call_errors_give_the_reason() {
        let s = scope();
        let why = |t: &str| s.call_error(t).unwrap_or_default();
        assert_eq!(why("$uuidBytes('abc')"), r#""abc" is not a UUID"#);
        assert_eq!(
            why("$uuidBytes(token)"),
            format!(r#""{MASK}" is not a UUID"#)
        );
        assert_eq!(why("$nope('a')"), "unknown function");
        assert_eq!(why("$upper()"), "it takes 1 argument, not 0");
        assert_eq!(why("$hmacSha256('a')"), "it takes 2 arguments, not 1");
        assert_eq!(why("$upper(missing)"), "`missing` is not defined");
        assert_eq!(
            why("$hexBytes($sha256(missing))"),
            "$sha256: `missing` is not defined"
        );
        assert_eq!(s.call_error("$upper('a')"), None);
        assert_eq!(s.call_error("token"), None);
    }

    #[test]
    fn a_names_only_scope_does_not_run_secret_or_lua_calls() {
        let (catalog, _) = LuaCatalog::new(vec![crate::model::LuaScript {
            origin: "lua/t.lua".into(),
            code: "function boom() error('bad') end".into(),
        }]);
        let mut s = Scope::new().with_layer(
            "env",
            BTreeMap::from([("token".to_string(), VarValue::secret(""))]),
        );
        s.set_catalog(Arc::new(catalog));
        s.set_names_only();
        for t in ["{{ $uuidBytes(token) }}", "{{ $lua.boom() }}"] {
            let r = s.render(t);
            assert!(r.unresolved.is_empty() && r.secret, "{t}");
            assert_eq!(s.render_masked(t), t);
        }
        for t in [
            "{{ $uuidBytes('abc') }}",
            "{{ $lua.nope() }}",
            "{{ $upper(x) }}",
        ] {
            assert_eq!(s.render(t).unresolved.len(), 1, "{t}");
        }
    }

    #[test]
    fn pre_send_calls_need_a_kept_value() {
        let t = "{{ $prompt('OTP') }}";
        let s = scope();
        // A send scope with no value: unresolved, with the reason.
        assert_eq!(s.render(t).unresolved, ["$prompt('OTP')"]);
        assert_eq!(
            s.call_error("$prompt('OTP')").as_deref(),
            Some("it needs a value before the send")
        );
        assert!(!s.is_defined("$prompt('OTP')"));
        // The value the pipeline keeps is used for every render of the send.
        s.keep(
            "$prompt('OTP')",
            Rendered {
                text: "123456".into(),
                ..Default::default()
            },
        );
        assert_eq!(s.render(&format!("a{t}b")).text, "a123456b");
        assert!(s.is_defined("$prompt('OTP')"));
        assert_eq!(s.clone().render(t).text, "123456");
    }

    #[test]
    fn deferred_and_names_only_scopes_list_pre_send_calls() {
        let mut s = scope();
        s.set_pre_send_deferred();
        let t = "{{ $prompt('OTP') }}/{{ $select('Mode', 'a', 'b') }}/{{ $prompt('OTP') }}";
        let r = s.render(t);
        assert!(r.unresolved.is_empty() && r.secret);
        assert!(s.is_defined("$response('Auth/login', '$.token')"));
        assert_eq!(
            s.pre_send_pending(),
            ["$prompt('OTP')", "$select('Mode', 'a', 'b')"]
        );
        let mut n = scope();
        n.set_names_only();
        assert!(n.render(t).unresolved.is_empty());
        // A call that is not valid stays unresolved in every scope.
        for bad in [
            "$prompt()",
            "$prompt('')",
            "$prompt('a', 'x')",
            "$prompt(token)",
            "$select('a')",
            "$response('Auth/login')",
            "$response('a', '$.t', 'soon')",
        ] {
            assert!(!s.is_defined(bad), "{bad}");
            assert!(s.call_error(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn pre_send_parsing() {
        use funcs::{Policy, PreSend};
        let p = |t: &str| funcs::pre_send(&funcs::parse(t).unwrap()).unwrap();
        assert_eq!(
            p("$prompt('Pass', 'secret')"),
            Ok(PreSend::Prompt {
                label: "Pass".into(),
                secret: true
            })
        );
        assert_eq!(
            p("$select('Mode', 'a', 'b')"),
            Ok(PreSend::Select {
                label: "Mode".into(),
                options: vec!["a".into(), "b".into()]
            })
        );
        assert_eq!(
            p("$response('Auth/login.toml', '$.token')"),
            Ok(PreSend::Response {
                path: "Auth/login".into(),
                expr: "$.token".into(),
                policy: Policy::IfMissing
            })
        );
        for (t, policy) in [
            ("never", Policy::Never),
            ("always", Policy::Always),
            ("if_older:300", Policy::IfOlder(300)),
        ] {
            assert!(matches!(
                p(&format!("$response('a', '$.t', '{t}')")),
                Ok(PreSend::Response { policy: q, .. }) if q == policy
            ));
        }
        assert!(funcs::pre_send(&funcs::parse("$upper('a')").unwrap()).is_none());
    }

    #[test]
    fn process_env_reads_a_secret() {
        let path = std::env::var("PATH").unwrap();
        let s = scope();
        let r = s.render("{{$processEnv.PATH}}");
        assert_eq!(r.text, path);
        assert!(r.secret);
        assert!(r.unresolved.is_empty());
        assert_eq!(
            s.render_masked("{{$processEnv.PATH}}"),
            "{{$processEnv.PATH}}"
        );
        assert!(s.secret_values().contains(&path));
        assert!(s.is_defined("$processEnv.PATH"));
        assert!(is_builtin("$processEnv.X"));
        assert!(!is_builtin("$processEnv."));
    }

    #[test]
    fn process_env_unset_is_unresolved() {
        let s = scope();
        let name = "$processEnv.SENDITO_UNSET_7f3a";
        let r = s.render(&format!("{{{{{name}}}}}"));
        assert_eq!(r.text, format!("{{{{{name}}}}}"));
        assert_eq!(r.unresolved, vec![name.to_string()]);
        assert!(!s.is_defined(name));
        assert!(!s.is_defined("$processEnv."));
    }

    #[test]
    fn set_and_names() {
        let mut s = scope();
        s.set("runtime", "x", VarValue::plain("1"));
        s.set("runtime", "y", VarValue::plain("2"));
        s.set("dev", "env", VarValue::plain("dev2"));
        assert_eq!(s.render("{{x}}{{y}}{{env}}").text, "12dev2");
        assert_eq!(s.layers().len(), 3);
        assert!(s.names().contains(&"token".to_string()));
    }

    #[test]
    fn scrub_masks() {
        assert_eq!(
            scrub("tok=abcd", &["abcd".into(), "x".into()]),
            format!("tok={MASK}")
        );
    }

    #[test]
    fn scrub_masks_encoded_echoes() {
        use crate::resolve::{form_urlencode, query_encode};
        let (pw, q) = ("ab\"c\\d", "a+b/c d");
        let secrets = vec![
            "token123".to_string(),
            pw.to_string(),
            q.to_string(),
            "x1".to_string(),
        ];
        let json = format!(
            r#"{{"pw":{},"q":{}}}"#,
            serde_json::to_string(pw).unwrap(),
            serde_json::to_string(q).unwrap()
        );
        let out = scrub_bytes(json.as_bytes(), &secrets).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(r#"{{"pw":"{MASK}","q":"{MASK}"}}"#)
        );
        let masked = format!("pw={MASK}");
        let bearer = format!("Bearer {MASK}");
        for (text, want) in [
            (format!("pw={}", form_urlencode(q)), masked.clone()),
            (format!("pw={}", query_encode(q)), masked.clone()),
            (
                format!("pw={}", form_urlencode(q).to_ascii_lowercase()),
                masked.clone(),
            ),
            (format!("pw={}", query_encode(pw)), masked.clone()),
            (
                format!("Bearer {}", query_encode(pw).to_ascii_lowercase()),
                bearer,
            ),
        ] {
            assert_eq!(scrub(&text, &secrets), want, "{text}");
        }
        assert_eq!(
            scrub(&format!("x {q} y {pw}!"), &secrets),
            format!("x {MASK} y {MASK}!")
        );
        assert_eq!(scrub("Bearer token123", &secrets), format!("Bearer {MASK}"));
        assert_eq!(scrub("x1 nothing here", &secrets), "x1 nothing here");
        assert!(scrub_bytes(b"nothing here", &secrets).is_none());
        assert_eq!(
            mask_forms(&["token123".to_string()]),
            vec!["token123".to_string()]
        );
    }
}
