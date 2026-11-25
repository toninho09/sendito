//! Folder settings: `folder.toml` in a collection folder (or the root). The
//! headers, auth, variables and scripts it holds apply to every request below
//! it; `sendito_core::inherit` merges them. Its description documents the
//! folder only. Schema in FORMAT.md.

use serde::Deserialize;

use sendito_core::model::FolderSettings;

use crate::request_file::{
    RawAuth, RawKv, RawScripts, auth, kvs, scripts, write_auth, write_kvs, write_scripts,
};
use crate::tomlfmt::Writer;

/// The file name of the folder settings, in each folder.
pub const FILE: &str = "folder.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFolder {
    description: Option<String>,
    #[serde(default)]
    headers: Vec<RawKv>,
    auth: Option<RawAuth>,
    #[serde(default)]
    vars: Vec<RawKv>,
    #[serde(default)]
    scripts: RawScripts,
}

/// Parses a `folder.toml`.
pub fn parse(text: &str) -> Result<FolderSettings, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    if table.contains_key("kind") {
        return Err(format!(
            "this is a request file; {FILE} holds the folder settings, so rename the request"
        ));
    }
    let r: RawFolder = table.try_into().map_err(|e| e.to_string())?;
    Ok(FolderSettings {
        headers: kvs(r.headers),
        auth: auth(r.auth)?,
        vars: kvs(r.vars),
        scripts: scripts(r.scripts)?,
        description: r.description.unwrap_or_default(),
    })
}

/// Serializes folder settings in canonical form.
pub fn to_toml(f: &FolderSettings) -> String {
    let mut w = Writer::new();
    if !f.description.is_empty() {
        w.str("description", &f.description);
    }
    write_kvs(&mut w, "headers", &f.headers);
    write_auth(&mut w, &f.auth);
    write_kvs(&mut w, "vars", &f.vars);
    write_scripts(&mut w, &f.scripts);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::model::{Auth, KeyValue, SetVar};

    #[test]
    fn roundtrip() {
        let mut f = FolderSettings {
            headers: vec![
                KeyValue::new("X-Tenant", "{{tenant}}"),
                KeyValue::new("Off", "1").disabled(),
            ],
            auth: Auth::Bearer {
                token: "{{token}}".into(),
            },
            vars: vec![KeyValue::new("base", "https://api.example.com")],
            description: "# Users\n\nAll *user* endpoints.\n".into(),
            ..Default::default()
        };
        f.scripts.pre.push(SetVar {
            var: "a".into(),
            value: "1".into(),
        });
        let t = to_toml(&f);
        assert_eq!(parse(&t).unwrap_or_else(|e| panic!("{t}\n{e}")), f);
        assert_eq!(parse("").unwrap(), FolderSettings::default());
        assert!(to_toml(&FolderSettings::default()).trim().is_empty());
        let none = FolderSettings {
            auth: Auth::None,
            ..Default::default()
        };
        assert_eq!(parse(&to_toml(&none)).unwrap(), none);
    }

    #[test]
    fn a_request_file_is_refused_by_name() {
        let e = parse("kind = \"http\"\nurl = \"x\"\n").unwrap_err();
        assert!(e.contains("request file"), "{e}");
        assert!(parse("method = \"GET\"\n").is_err());
    }
}
