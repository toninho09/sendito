//! Import from Postman Collection v2.1 and curl; export to curl.

pub mod bruno;
pub mod curl;
pub mod http_file;
pub mod insomnia;
pub mod openapi;
pub mod postman;

use sendito_core::Request;
use sendito_store::collection::{
    Collection, join_rel, validate_folder_name, validate_name, validate_request_name,
};
use sendito_store::env::{COLLECTION_ENV, EnvVar, Envs, VarSource};

/// An imported tree, not yet written to disk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportFolder {
    pub name: String,
    /// What the folder gives the requests below it (its `folder.toml`).
    pub settings: sendito_core::model::FolderSettings,
    pub folders: Vec<ImportFolder>,
    pub requests: Vec<(String, Request)>,
    /// Saved example responses, by the index of their request in `requests`.
    pub examples: std::collections::BTreeMap<usize, Vec<sendito_store::examples::Example>>,
}

impl ImportFolder {
    pub fn count_requests(&self) -> usize {
        self.requests.len()
            + self
                .folders
                .iter()
                .map(ImportFolder::count_requests)
                .sum::<usize>()
    }
}

/// Something that was not imported, or imported differently, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Where it was found, e.g. `Users / Create user`.
    pub item: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportResult {
    pub root: ImportFolder,
    /// Collection-level variables (written to the collection environment).
    pub variables: Vec<(String, String)>,
    /// Selectable environments imported alongside the collection.
    pub environments: Vec<ImportEnvironment>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportEnvironment {
    pub name: String,
    pub variables: Vec<(String, String)>,
}

/// Recognized collection formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Postman,
    Insomnia,
    Bruno,
    OpenApi,
    Swagger,
    HttpFile,
}

impl ImportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Postman => "Postman",
            Self::Insomnia => "Insomnia",
            Self::Bruno => "Bruno",
            Self::OpenApi => "OpenAPI",
            Self::Swagger => "Swagger",
            Self::HttpFile => ".http file",
        }
    }
}

/// An OpenAPI or Swagger document as an import.
fn import_openapi(doc: &serde_json::Value) -> Result<(ImportFormat, ImportResult), String> {
    let format = if openapi::is_swagger(doc) {
        ImportFormat::Swagger
    } else {
        ImportFormat::OpenApi
    };
    openapi::import(doc).map(|r| (format, r))
}

/// Detect a JSON export without falling back after a recognized format fails.
pub fn import_json(text: &str) -> Result<(ImportFormat, ImportResult), String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    if openapi::is_openapi(&value) {
        return import_openapi(&value);
    }
    if value.get("__export_format").is_some() {
        insomnia::import(text).map(|result| (ImportFormat::Insomnia, result))
    } else {
        postman::import(text)
            .map(|result| (ImportFormat::Postman, result))
            .map_err(|e| e.to_string())
    }
}

/// Detect a Bruno directory, a `.http` file, an OpenAPI / Swagger document
/// (JSON or YAML) or a Postman/Insomnia JSON file.
pub fn import_path(path: &std::path::Path) -> Result<(ImportFormat, ImportResult), String> {
    if path.is_dir() {
        return bruno::import(path).map(|result| (ImportFormat::Bruno, result));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "http" | "rest" => http_file::import_path(path, &text).map(|r| (ImportFormat::HttpFile, r)),
        "yaml" | "yml" => {
            let value: serde_json::Value =
                serde_norway::from_str(&text).map_err(|e| format!("not valid YAML: {e}"))?;
            if !openapi::is_openapi(&value) {
                return Err("a YAML file must be an OpenAPI or Swagger document".into());
            }
            import_openapi(&value)
        }
        _ => import_json(&text),
    }
}

/// Render URL query and form fields while keeping unresolved placeholders
/// readable instead of percent-encoding their braces in exported commands.
pub(crate) fn preserve_export_templates(
    req: &sendito_core::HttpRequest,
    scope: &sendito_core::Scope,
    resolved: &mut sendito_core::resolve::ResolvedHttp,
) {
    use sendito_core::model::{ApiKeyLocation, Auth, Body};
    use sendito_core::resolve::{ResolvedBody, form_urlencode, normalize_url, query_encode};
    use sendito_core::template::Segment;
    fn encode(text: &str, f: fn(&str) -> String) -> String {
        let mut out = String::new();
        for part in sendito_core::template::parse(text) {
            match part {
                Segment::Text(s) => out.push_str(&f(s)),
                Segment::Var { span, .. } => out.push_str(&text[span]),
            }
        }
        out
    }
    let mut pairs: Vec<(String, String)> = req
        .params
        .iter()
        .filter(|p| p.enabled && !p.name.is_empty())
        .map(|p| (scope.render(&p.name).text, scope.render(&p.value).text))
        .collect();
    if let Auth::ApiKey {
        key,
        value,
        location: ApiKeyLocation::Query,
    } = &req.auth
    {
        pairs.push((scope.render(key).text, scope.render(value).text));
    }
    if !pairs.is_empty() {
        let base = normalize_url(&scope.render(&req.url).text);
        let (before, fragment) = base
            .find('#')
            .map(|i| base.split_at(i))
            .unwrap_or((&base, ""));
        let mut url = before.to_string();
        if !url.contains('?') {
            url.push('?');
        } else if !url.ends_with('?') && !url.ends_with('&') {
            url.push('&');
        }
        url.push_str(
            &pairs
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k, query_encode), encode(v, query_encode)))
                .collect::<Vec<_>>()
                .join("&"),
        );
        url.push_str(fragment);
        resolved.url = url;
    }
    if let Body::Form { fields } = &req.body {
        let body = fields
            .iter()
            .filter(|f| f.enabled && !f.name.is_empty())
            .map(|f| {
                format!(
                    "{}={}",
                    encode(&scope.render(&f.name).text, form_urlencode),
                    encode(&scope.render(&f.value).text, form_urlencode)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        resolved.body = ResolvedBody::Bytes(body.into_bytes());
    }
}

/// Placeholder for a JWT in exports: Sendito signs it when it sends.
pub(crate) const JWT_PLACEHOLDER: &str = "{{sendito_jwt}}";

/// Placeholder for an OAuth token in formats that cannot access the keychain.
pub(crate) fn oauth_placeholder(identity: &str) -> String {
    if identity.is_empty()
        || !identity
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        "{{oauth_token}}".into()
    } else {
        format!("{{{{oauth.{identity}}}}}")
    }
}

/// Makes a file-system-safe, unique name.
pub fn safe_name(name: &str, taken: &mut Vec<String>) -> String {
    unique_name(name, taken, validate_name)
}

/// [`safe_name`] for a folder: a name ending in `.examples` is reserved for a
/// request's saved examples.
pub fn safe_folder_name(name: &str, taken: &mut Vec<String>) -> String {
    unique_name(name, taken, validate_folder_name)
}

/// [`safe_name`] for a request: `folder` is the folder settings file.
pub fn safe_request_name(name: &str, taken: &mut Vec<String>) -> String {
    unique_name(name, taken, validate_request_name)
}

fn unique_name(
    name: &str,
    taken: &mut Vec<String>,
    valid: fn(&str) -> sendito_store::Result<()>,
) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    while s.starts_with('.') {
        s.remove(0);
    }
    if s.is_empty() {
        s = "untitled".into();
    }
    if s.len() > 120 {
        let mut cut = 120;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    let base = s.trim().to_string();
    let mut candidate = base.clone();
    let mut i = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate)) || valid(&candidate).is_err() {
        candidate = format!("{base} ({i})");
        i += 1;
        if i > 10_000 {
            candidate = format!("untitled {i}");
            break;
        }
    }
    taken.push(candidate.clone());
    candidate
}

/// What [`write`] created.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteReport {
    pub folder: String,
    pub requests: usize,
    pub folders: usize,
    pub variables: usize,
    pub environments: usize,
    pub variable_conflicts: Vec<String>,
    pub renamed_environments: Vec<(String, String)>,
}

/// Writes an import under `parent` in a new folder named after the root.
pub fn write(
    result: &ImportResult,
    coll: &Collection,
    envs: Option<&Envs>,
    parent: &str,
) -> Result<WriteReport, sendito_store::StoreError> {
    sendito_store::collection::validate_rel(parent)?;
    let mut rep = WriteReport::default();
    let mut taken: Vec<String> = std::fs::read_dir(coll.abs(parent))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let name = safe_folder_name(&result.root.name, &mut taken);
    let rel = coll.create_folder(parent, &name)?;
    rep.folder = rel.clone();
    write_folder(&result.root, coll, &rel, &mut rep)?;
    if let Some(envs) = envs
        && !result.variables.is_empty()
    {
        let mut env = envs.load(COLLECTION_ENV)?;
        for (k, v) in &result.variables {
            if env.get(k).is_none() {
                env.set(EnvVar {
                    name: k.clone(),
                    source: VarSource::Plain(v.clone()),
                });
                rep.variables += 1;
            } else {
                rep.variable_conflicts.push(k.clone());
            }
        }
        envs.save(&env)?;
    }
    if let Some(envs) = envs {
        let mut taken = envs.list();
        for imported in &result.environments {
            // `_…` names and `runtime` are reserved in a collection.
            let raw_name = if imported.name.starts_with('_')
                || imported.name == sendito_core::script::RUNTIME_LAYER
            {
                format!("env-{}", imported.name)
            } else {
                imported.name.clone()
            };
            let name = safe_name(&raw_name, &mut taken);
            if name != imported.name {
                rep.renamed_environments
                    .push((imported.name.clone(), name.clone()));
            }
            let mut env = sendito_store::env::EnvFile::new(&name);
            for (name, value) in &imported.variables {
                env.set(EnvVar {
                    name: name.clone(),
                    source: VarSource::Plain(value.clone()),
                });
            }
            envs.save(&env)?;
            rep.environments += 1;
        }
    }
    Ok(rep)
}

fn write_folder(
    f: &ImportFolder,
    coll: &Collection,
    rel: &str,
    rep: &mut WriteReport,
) -> Result<(), sendito_store::StoreError> {
    coll.save_folder(rel, &f.settings)?;
    let mut taken = Vec::new();
    for sub in &f.folders {
        let name = safe_folder_name(&sub.name, &mut taken);
        let r = coll.create_folder(rel, &name)?;
        rep.folders += 1;
        write_folder(sub, coll, &r, rep)?;
    }
    for (i, (name, req)) in f.requests.iter().enumerate() {
        let name = safe_request_name(name, &mut taken);
        let request_rel = coll.create_request(rel, &name, req)?;
        rep.requests += 1;
        for ex in f.examples.get(&i).into_iter().flatten() {
            let mut ex = ex.clone();
            ex.name = safe_name(&ex.name, &mut Vec::new());
            sendito_store::examples::save(coll, &request_rel, &ex)?;
        }
    }
    let _ = join_rel;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_tree_and_variables() {
        use sendito_core::model::{Auth, FolderSettings};
        let d = tempfile::tempdir().unwrap();
        sendito_store::init::init(d.path()).unwrap();
        let coll = Collection::new(d.path(), vec![]);
        let envs = Envs::new(
            d.path(),
            "t",
            std::sync::Arc::new(sendito_store::env::MemoryKeychain::default()),
        );
        let result = ImportResult {
            root: ImportFolder {
                name: "API/v1".into(),
                settings: FolderSettings {
                    auth: Auth::Bearer {
                        token: "{{t}}".into(),
                    },
                    ..Default::default()
                },
                folders: vec![ImportFolder {
                    name: "Users".into(),
                    settings: FolderSettings::default(),
                    folders: vec![],
                    requests: vec![
                        ("list".into(), Request::new_http()),
                        ("list".into(), Request::new_grpc()),
                        ("folder".into(), Request::new_http()),
                    ],
                    examples: [(
                        0,
                        vec![sendito_store::examples::Example {
                            name: "a/b".into(),
                            status: 201,
                            ..Default::default()
                        }],
                    )]
                    .into(),
                }],
                requests: vec![("ping".into(), Request::new_http())],
                examples: Default::default(),
            },
            variables: vec![("base".into(), "http://x".into())],
            environments: vec![],
            skipped: vec![],
        };
        let rep = write(&result, &coll, Some(&envs), "").unwrap();
        assert_eq!(rep.folder, "API-v1");
        assert_eq!((rep.requests, rep.folders, rep.variables), (4, 1, 1));
        assert!(d.path().join("API-v1/Users/list (2).toml").is_file());
        // The examples go next to their request, with a file-safe name.
        assert_eq!(
            sendito_store::examples::list(&coll, "API-v1/Users/list.toml").unwrap(),
            ["a-b"]
        );
        // A request named "folder" does not take the folder settings file.
        assert!(d.path().join("API-v1/Users/folder (2).toml").is_file());
        assert!(!d.path().join("API-v1/Users/folder.toml").exists());
        assert_eq!(
            coll.chain("API-v1/Users/list.toml").unwrap()[0].1,
            result.root.settings
        );
        // A second import gets a fresh folder and does not duplicate variables.
        let rep2 = write(&result, &coll, Some(&envs), "").unwrap();
        assert_eq!(rep2.folder, "API-v1 (2)");
        assert_eq!(rep2.variables, 0);
        assert_eq!(result.root.count_requests(), 4);
    }

    #[test]
    fn folder_names_ending_in_examples_are_renamed() {
        let d = tempfile::tempdir().unwrap();
        sendito_store::init::init(d.path()).unwrap();
        let coll = Collection::new(d.path(), vec![]);
        let result = ImportResult {
            root: ImportFolder {
                name: "Shots.examples".into(),
                folders: vec![ImportFolder {
                    name: "Nested.examples".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let rep = write(&result, &coll, None, "").unwrap();
        assert_eq!(rep.folders, 1);
        assert!(
            d.path()
                .join("Shots.examples (2)/Nested.examples (2)")
                .is_dir()
        );
        assert!(!d.path().join("Shots.examples").exists());
    }

    #[test]
    fn exports_keep_secret_placeholders_in_query_and_form() {
        use sendito_core::model::{ApiKeyLocation, Auth, Body, HttpRequest, KeyValue};
        use sendito_core::vars::{Scope, VarValue};
        let mut scope = Scope::new();
        scope.set("env", "token", VarValue::secret("value & secret"));
        let req = HttpRequest {
            method: "POST".into(),
            url: "http://example.test/a?literal=%7B%7Bx%7D%7D#fragment".into(),
            params: vec![KeyValue::new("q", "prefix {{token}}")],
            auth: Auth::ApiKey {
                key: "api_key".into(),
                value: "{{token}}".into(),
                location: ApiKeyLocation::Query,
            },
            body: Body::Form {
                fields: vec![KeyValue::new("value", "{{token}}")],
            },
            ..Default::default()
        };
        let curl = super::curl::export(&req, &scope, false, std::path::Path::new("."));
        let file = super::http_file::export("test", &req, &scope, false, std::path::Path::new("."));
        for output in [&curl, &file] {
            assert!(
                output.contains(
                    "literal=%7B%7Bx%7D%7D&q=prefix%20{{token}}&api_key={{token}}#fragment"
                ),
                "{output}"
            );
            assert!(output.contains("value={{token}}"), "{output}");
            assert!(!output.contains("value & secret"));
        }
        let with_secret =
            super::http_file::export("test", &req, &scope, true, std::path::Path::new("."));
        assert!(with_secret.contains("api_key=value%20%26%20secret"));
        assert!(with_secret.ends_with("value=value+%26+secret"));
    }

    #[test]
    fn names() {
        let mut t = vec![];
        assert_eq!(safe_name("a/b", &mut t), "a-b");
        assert_eq!(safe_name("a/b", &mut t), "a-b (2)");
        assert_eq!(safe_name("..hidden", &mut t), "hidden");
        assert_eq!(safe_name("", &mut t), "untitled");
        assert_eq!(safe_name(&"x".repeat(300), &mut t).len(), 120);
    }
}
