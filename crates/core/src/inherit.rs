//! Folder inheritance: the headers, auth, variables and scripts that the
//! `folder.toml` files above a request give it.
//!
//! The chain goes from the collection root to the request's own folder. A
//! closer folder wins over an outer one, and the request wins over all of
//! them. [`apply`] returns a flattened request, so resolution does not know
//! about folders.

use std::collections::{BTreeMap, HashSet};

use crate::model::{Auth, FolderSettings, KeyValue, LuaScript, Request, Scripts};
use crate::vars::{Layer, VarValue};

/// One folder of the chain: its relative path (`""` for the root) and its settings.
pub type ChainItem = (String, FolderSettings);

/// The prefix of the variable layer of each folder (`folder:Users/Admin`).
pub const FOLDER_LAYER: &str = "folder:";

/// The request with the chain applied: the auth it inherits, the headers of
/// every level, and the folder scripts before its own.
pub fn apply(req: &Request, chain: &[ChainItem]) -> Request {
    if chain.is_empty() {
        return resolve_auth(req.clone(), chain);
    }
    let mut out = req.clone();
    let folder_headers = || chain.iter().map(|(_, f)| f.headers.as_slice());
    match &mut out {
        Request::Http(h) => {
            h.headers = merge_rows(folder_headers(), &h.headers, false);
            h.common.scripts = merge_scripts(chain, &h.common.scripts);
        }
        Request::Sse(s) => {
            s.headers = merge_rows(folder_headers(), &s.headers, false);
            s.common.scripts = merge_scripts(chain, &s.common.scripts);
        }
        Request::Ws(w) => {
            w.headers = merge_rows(folder_headers(), &w.headers, false);
            w.common.scripts = merge_scripts(chain, &w.common.scripts);
        }
        Request::Grpc(g) => {
            g.metadata = merge_rows(folder_headers(), &g.metadata, true);
            g.common.scripts = merge_scripts(chain, &g.common.scripts);
        }
        // Folder headers do not apply to MQTT: the protocol has no headers.
        Request::Mqtt(m) => m.common.scripts = merge_scripts(chain, &m.common.scripts),
        Request::Mcp(m) => m.common.scripts = merge_scripts(chain, &m.common.scripts),
    }
    resolve_auth(out, chain)
}

/// A note for an MQTT request when a folder of the chain sets headers: MQTT
/// has no headers, so they are not sent. Other kinds get `None`.
pub fn ignored_headers_note(req: &Request, chain: &[ChainItem]) -> Option<String> {
    if !matches!(req, Request::Mqtt(_)) {
        return None;
    }
    let folders: Vec<&str> = chain
        .iter()
        .filter(|(_, f)| {
            f.headers
                .iter()
                .any(|h| h.enabled && !h.name.trim().is_empty())
        })
        .map(|(rel, _)| {
            if rel.is_empty() {
                "the collection root"
            } else {
                rel.as_str()
            }
        })
        .collect();
    (!folders.is_empty()).then(|| {
        format!(
            "folder headers do not apply to MQTT and are not sent (set in {})",
            folders.join(", ")
        )
    })
}

/// Replaces `Auth::Inherit` with the auth it stands for.
fn resolve_auth(mut req: Request, chain: &[ChainItem]) -> Request {
    if let Some(auth) = req.auth_mut()
        && *auth == Auth::Inherit
    {
        *auth = auth_source(chain)
            .map(|(_, a)| a.clone())
            .unwrap_or(Auth::None);
    }
    req
}

/// The nearest folder that defines an auth, and that auth.
pub fn auth_source(chain: &[ChainItem]) -> Option<(&str, &Auth)> {
    chain
        .iter()
        .rev()
        .find(|(_, f)| f.auth != Auth::Inherit)
        .map(|(rel, f)| (rel.as_str(), &f.auth))
}

/// The rows a request inherits, in the order they are sent, without its own.
/// A row whose name a closer level also has is left out.
pub fn inherited_rows(chain: &[ChainItem], own: &[KeyValue]) -> Vec<KeyValue> {
    let own_names = names(own);
    merge_rows(chain.iter().map(|(_, f)| f.headers.as_slice()), &[], false)
        .into_iter()
        .filter(|r| !own_names.contains(&key(&r.name)))
        .collect()
}

/// Merges the rows of each level, outer first. A closer level that has a
/// row with the same name (case-insensitive) removes the outer rows of that
/// name; when the closer row is disabled, the header is not sent at all.
/// `lower` lower-cases the inherited names (gRPC metadata).
fn merge_rows<'a>(
    levels: impl Iterator<Item = &'a [KeyValue]>,
    own: &[KeyValue],
    lower: bool,
) -> Vec<KeyValue> {
    let mut out: Vec<KeyValue> = Vec::new();
    for level in levels {
        let level: Vec<KeyValue> = level
            .iter()
            .map(|r| KeyValue {
                name: if lower {
                    r.name.to_ascii_lowercase()
                } else {
                    r.name.clone()
                },
                ..r.clone()
            })
            .collect();
        push_level(&mut out, &level);
    }
    push_level(&mut out, own);
    out
}

fn push_level(out: &mut Vec<KeyValue>, level: &[KeyValue]) {
    let taken = names(level);
    out.retain(|r| !taken.contains(&key(&r.name)));
    out.extend(level.iter().cloned());
}

fn key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn names(rows: &[KeyValue]) -> HashSet<String> {
    rows.iter()
        .map(|r| key(&r.name))
        .filter(|n| !n.is_empty())
        .collect()
}

fn merge_scripts(chain: &[ChainItem], own: &Scripts) -> Scripts {
    let mut out = Scripts::default();
    let levels = chain
        .iter()
        .map(|(rel, f)| (Some(rel.as_str()), &f.scripts))
        .chain([(None, own)]);
    for (rel, s) in levels {
        out.pre.extend(s.pre.iter().cloned());
        out.extract.extend(s.extract.iter().cloned());
        out.asserts.extend(s.asserts.iter().cloned());
        // A folder's script is named after the folder in error messages.
        let named = |r: &LuaScript| LuaScript {
            origin: match rel {
                Some("") => "folder /".to_string(),
                Some(rel) => format!("folder {rel}"),
                None => r.origin.clone(),
            },
            code: r.code.clone(),
        };
        out.lua_pre.extend(s.lua_pre.iter().map(named));
        out.lua_post.extend(s.lua_post.iter().map(named));
    }
    out
}

/// One variable layer per folder that defines variables, root first. They go
/// above the environments and below the runtime variables.
pub fn var_layers(chain: &[ChainItem]) -> Vec<Layer> {
    chain
        .iter()
        .filter(|(_, f)| !f.vars.is_empty())
        .map(|(rel, f)| Layer {
            name: format!("{FOLDER_LAYER}{rel}"),
            vars: f
                .vars
                .iter()
                .filter(|v| v.enabled && !v.name.trim().is_empty())
                .map(|v| (v.name.trim().to_string(), VarValue::plain(v.value.clone())))
                .collect::<BTreeMap<_, _>>(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GrpcRequest, HttpRequest, McpRequest, SetVar};

    fn folder(headers: &[(&str, &str)], auth: Auth) -> FolderSettings {
        FolderSettings {
            headers: headers.iter().map(|(n, v)| KeyValue::new(*n, *v)).collect(),
            auth,
            ..Default::default()
        }
    }

    fn bearer(t: &str) -> Auth {
        Auth::Bearer { token: t.into() }
    }

    fn http(req: HttpRequest) -> Request {
        Request::Http(req)
    }

    #[test]
    fn auth_comes_from_the_nearest_folder() {
        let chain = vec![
            (String::new(), folder(&[], bearer("root"))),
            ("A".into(), folder(&[], Auth::Inherit)),
            ("A/B".into(), folder(&[], bearer("b"))),
        ];
        let out = apply(&http(HttpRequest::default()), &chain);
        assert_eq!(out.auth(), &bearer("b"));
        assert_eq!(auth_source(&chain).map(|(r, _)| r), Some("A/B"));
        let out = apply(&http(HttpRequest::default()), &chain[..2]);
        assert_eq!(out.auth(), &bearer("root"));
    }

    #[test]
    fn own_auth_and_explicit_none_win() {
        let chain = vec![(String::new(), folder(&[], bearer("root")))];
        let mut req = HttpRequest {
            auth: Auth::None,
            ..Default::default()
        };
        assert_eq!(apply(&http(req.clone()), &chain).auth(), &Auth::None);
        req.auth = bearer("own");
        assert_eq!(apply(&http(req), &chain).auth(), &bearer("own"));
    }

    #[test]
    fn no_folder_auth_means_none() {
        let out = apply(&http(HttpRequest::default()), &[]);
        assert_eq!(out.auth(), &Auth::None);
    }

    #[test]
    fn closer_rows_replace_outer_rows_and_a_disabled_one_removes_them() {
        let chain = vec![
            (
                String::new(),
                folder(
                    &[("X-A", "root"), ("X-B", "root"), ("X-C", "root")],
                    Auth::Inherit,
                ),
            ),
            ("A".into(), folder(&[("x-a", "a")], Auth::Inherit)),
        ];
        let req = HttpRequest {
            headers: vec![
                KeyValue::new("X-B", "own").disabled(),
                KeyValue::new("X-D", "own"),
            ],
            ..Default::default()
        };
        let Request::Http(out) = apply(&http(req.clone()), &chain) else {
            panic!()
        };
        let sent: Vec<(&str, &str)> = out
            .headers
            .iter()
            .filter(|h| h.enabled)
            .map(|h| (h.name.as_str(), h.value.as_str()))
            .collect();
        assert_eq!(sent, [("X-C", "root"), ("x-a", "a"), ("X-D", "own")]);
        let inherited: Vec<String> = inherited_rows(&chain, &req.headers)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(inherited, ["X-C", "x-a"]);
    }

    #[test]
    fn grpc_metadata_names_are_lower_cased() {
        let chain = vec![(String::new(), folder(&[("X-Tenant", "t")], Auth::Inherit))];
        let Request::Grpc(g) = apply(&Request::Grpc(GrpcRequest::default()), &chain) else {
            panic!()
        };
        assert_eq!(g.metadata, [KeyValue::new("x-tenant", "t")]);
    }

    #[test]
    fn folder_scripts_run_first_and_mcp_keeps_no_headers() {
        let set = |v: &str| SetVar {
            var: v.into(),
            value: String::new(),
        };
        let mut root = FolderSettings::default();
        root.scripts.pre.push(set("root"));
        let lua = |code: &str| LuaScript::own(code).into_iter().collect::<Vec<_>>();
        root.scripts.lua_post = lua("root()");
        let mut a = FolderSettings::default();
        a.scripts.pre.push(set("a"));
        a.scripts.lua_pre = lua("a()");
        let chain = vec![(String::new(), root), ("A".into(), a)];
        let mut mcp = McpRequest::default();
        mcp.common.scripts.pre.push(set("own"));
        mcp.common.scripts.lua_pre = lua("own()");
        mcp.common.scripts.lua_post = lua("own()");
        let out = apply(&Request::Mcp(mcp), &chain);
        let order: Vec<&str> = out.scripts().pre.iter().map(|s| s.var.as_str()).collect();
        assert_eq!(order, ["root", "a", "own"]);
        // Folder Lua scripts run first and are named after their folder.
        let named = |v: &[LuaScript]| {
            v.iter()
                .map(|r| (r.origin.clone(), r.code.clone()))
                .collect::<Vec<_>>()
        };
        let pair = |o: &str, c: &str| (o.to_string(), c.to_string());
        assert_eq!(
            named(&out.scripts().lua_pre),
            [pair("folder A", "a()"), pair("", "own()")]
        );
        assert_eq!(
            named(&out.scripts().lua_post),
            [pair("folder /", "root()"), pair("", "own()")]
        );
    }

    #[test]
    fn var_layers_skip_empty_folders_and_disabled_rows() {
        let a = FolderSettings {
            vars: vec![
                KeyValue::new("host", "a"),
                KeyValue::new("off", "x").disabled(),
            ],
            ..Default::default()
        };
        let chain = vec![(String::new(), FolderSettings::default()), ("A".into(), a)];
        let layers = var_layers(&chain);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].name, "folder:A");
        assert_eq!(layers[0].vars.keys().collect::<Vec<_>>(), ["host"]);
    }
}
