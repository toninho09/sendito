//! Integration gates that run in CI: Postman import, curl round trip, format
//! stability and secret hygiene.

use std::path::{Path, PathBuf};

use sendito_core::Request;
use sendito_core::vars::Scope;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(p: &Path) -> String {
    match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => panic!("{}: {e}", p.display()),
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    let dir = p.parent().unwrap_or(root);
    if let Err(e) = std::fs::create_dir_all(dir) {
        panic!("{}: {e}", dir.display());
    }
    if let Err(e) = std::fs::write(&p, text) {
        panic!("{}: {e}", p.display());
    }
}

/// Postman v2.1 fixture (32 requests): every request is imported and every
/// feature that is not imported as-is is listed in the report.
#[test]
fn postman_import_has_no_unexplained_losses() {
    let json = read(&root().join("tests/fixtures/acme.postman_collection.json"));
    let r = match sendito_import::postman::import(&json) {
        Ok(r) => r,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(r.root.count_requests(), 32);
    let skipped: Vec<String> = r
        .skipped
        .iter()
        .map(|s| format!("{}: {}", s.item, s.reason))
        .collect();
    let report = skipped.join("\n");
    // Each lossy feature in the fixture is explained.
    for (item, needle) in [
        ("(collection)", "collection's JavaScript scripts"),
        ("(collection)", "variable \"old\" was disabled"),
        ("Users / Get user", ":id in the URL"),
        ("Users / Admin / Ban user", "JavaScript scripts"),
        ("Auth / OAuth token", "\"oauth2\" auth is not supported"),
        ("Random name", "$randomFirstName"),
        ("Dynamic id", "{{$guid}} became {{$uuid}}"),
        ("(collection)", "{{token}} is used by"),
        ("(collection)", "{{admin_pw}} is used by"),
        ("(collection)", "{{password}} is used by"),
        ("(collection)", "{{api_key}} is used by"),
        ("Users / List users", "the host field"),
        ("Users / Get user", "the host field"),
        ("Users / Admin / Audit log", "the host field"),
        ("Search", "the host field"),
        ("Users / User avatar", "absolute path"),
        ("Orders / Upload invoice", "absolute path"),
        ("Orders / Upload invoice", "Content-Type (application/pdf)"),
    ] {
        assert!(
            report.contains(&format!("{item}: "))
                && report
                    .lines()
                    .any(|l| l.starts_with(item) && l.contains(needle)),
            "missing explanation for {item} ({needle}) in:\n{report}"
        );
    }
    assert_eq!(r.skipped.len(), 18, "unexpected report entries:\n{report}");
    // The request description is imported, not reported.
    let cancel = r
        .root
        .folders
        .iter()
        .find(|f| f.name == "Orders")
        .and_then(|f| f.requests.iter().find(|(n, _)| n == "Cancel order"));
    assert_eq!(
        cancel.map(|(_, req)| req.description()),
        Some("Cancels the order")
    );
    // A GraphQL body keeps its query and variables apart.
    let gql = r
        .root
        .requests
        .iter()
        .find(|(n, _)| n == "GraphQL query")
        .map(|(_, req)| req.clone());
    assert!(matches!(
        gql,
        Some(Request::Http(sendito_core::HttpRequest {
            body: sendito_core::Body::GraphQL { query, variables, .. },
            ..
        })) if query == "{ me { id name } }" && variables == "{\"limit\": 5}"
    ));
    // Saved responses are imported as examples.
    let receipt = r
        .root
        .folders
        .iter()
        .find(|f| f.name == "Orders")
        .and_then(|f| {
            let i = f.requests.iter().position(|(n, _)| n == "Order receipt")?;
            f.examples.get(&i)
        });
    assert!(receipt.is_some_and(|e| !e.is_empty()));
    // Digest auth is imported.
    let digest = r
        .root
        .folders
        .iter()
        .find(|f| f.name == "Auth")
        .and_then(|f| f.requests.iter().find(|(n, _)| n == "Digest login"))
        .map(|(_, req)| req.auth().kind());
    assert_eq!(digest, Some(sendito_core::AuthKind::Digest));
    // Spot checks of converted content.
    let users = &r.root.folders[0];
    let Request::Http(list) = &users.requests[0].1 else {
        panic!()
    };
    assert_eq!(list.url, "{{base}}/users?page=1&size=20");
    assert_eq!(list.params.len(), 1);
    assert!(!list.params[0].enabled);
    let admin = &users.folders[0];
    let Request::Http(ban) = &admin.requests[0].1 else {
        panic!()
    };
    // The folder's auth goes to its folder.toml; the request inherits it.
    assert_eq!(ban.auth, sendito_core::Auth::Inherit);
    assert!(
        matches!(&admin.settings.auth, sendito_core::Auth::Basic { username, .. } if username == "admin")
    );
    let Request::Http(dynamic) = &r.root.requests[6].1 else {
        panic!()
    };
    assert_eq!(dynamic.headers[0].value, "{{$uuid}}");
    // A raw body without a language is sent as text/plain, as Postman does.
    let raw_text = r
        .root
        .requests
        .iter()
        .find(|(n, _)| n == "Raw text")
        .map(|(_, r)| r.clone());
    assert!(
        matches!(&raw_text, Some(Request::Http(h)) if matches!(&h.body,
            sendito_core::Body::Raw { content_type, .. } if content_type == "text/plain")),
        "{raw_text:?}"
    );
    assert_eq!(r.variables.len(), 2);
    // Writing produces one file per request.
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    sendito_store::init::init(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let coll = sendito_store::Collection::new(dir.path(), vec![]);
    let rep = sendito_import::write(&r, &coll, None, "").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rep.requests, 32);
    // Review artifacts: the imported files and the report.
    let review = root().join("target/corpus/import/postman");
    let _ = std::fs::remove_dir_all(&review);
    // The collection's name becomes the folder the requests are written to.
    assert_eq!(rep.folder, "Acme API");
    copy_tree(
        &dir.path().join(&rep.folder),
        &review.join("output").join(&rep.folder),
    );
    let mut vars = String::from("[variables]\n");
    for (k, v) in &r.variables {
        vars.push_str(&format!("{k} = {v:?}\n"));
    }
    std::fs::write(review.join("collection-variables.toml"), vars)
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        review.join("report.txt"),
        format!(
            "Collection \"Acme API\" imported into the folder \"{}\" (Sendito keeps the \
             collection name as the folder name).\n\nNot imported or changed:\n{report}\n",
            rep.folder
        ),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        coll.scan()
            .unwrap_or_else(|e| panic!("{e}"))
            .count_requests(),
        32
    );
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|e| panic!("{e}"));
    for e in std::fs::read_dir(from)
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
    {
        let (s, d) = (e.path(), to.join(e.file_name()));
        if s.is_dir() {
            copy_tree(&s, &d);
        } else {
            std::fs::copy(&s, &d).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

/// curl → request → curl → request keeps the request the same, for 20 fixtures.
#[test]
fn curl_round_trip() {
    let dir = root().join("tests/fixtures/curl");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sh"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 20);
    let mut failures = Vec::new();
    for f in &files {
        let cmd = read(f);
        let first = match sendito_import::curl::import(&cmd) {
            Ok(r) => r.request,
            Err(e) => {
                failures.push(format!("{}: {e}", f.display()));
                continue;
            }
        };
        let exported = sendito_import::curl::export(&first, &Scope::new(), true, Path::new(""));
        // Review artifacts: the imported request file and the re-exported command.
        let review = root().join("target/corpus/import/curl");
        std::fs::create_dir_all(&review).unwrap_or_else(|e| panic!("{e}"));
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        std::fs::write(
            review.join(format!("{stem}.toml")),
            sendito_store::request_file::to_toml(&Request::Http(first.clone())),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(
            review.join(format!("{stem}.export.sh")),
            format!("{exported}\n"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let second = match sendito_import::curl::import(&exported) {
            Ok(r) => r.request,
            Err(e) => {
                failures.push(format!(
                    "{}: re-import failed: {e}\n{exported}",
                    f.display()
                ));
                continue;
            }
        };
        // Semantic comparison: what each request puts on the wire.
        let wire = |r: &sendito_core::HttpRequest| {
            let res = sendito_core::resolve::resolve_http(r, &Scope::new(), Path::new("/"));
            let mut headers: Vec<(String, String)> = res
                .headers
                .iter()
                .map(|(n, v)| (n.to_ascii_lowercase(), v.clone()))
                .collect();
            headers.sort();
            let body = match res.body {
                sendito_core::resolve::ResolvedBody::Multipart(parts) => format!("{parts:?}"),
                sendito_core::resolve::ResolvedBody::Invalid(e) => format!("invalid: {e}"),
                b => format!("{b:?}"),
            };
            (
                res.method,
                res.url,
                headers,
                body,
                res.follow_redirects,
                res.verify_tls,
                res.timeout_ms,
            )
        };
        if wire(&first) != wire(&second) {
            failures.push(format!(
                "{}:\n{:#?}\n!=\n{:#?}\nexported:\n{exported}",
                f.display(),
                wire(&first),
                wire(&second)
            ));
        }
    }
    println!(
        "curl round trip: {}/{} passed",
        files.len() - failures.len(),
        files.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn toml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
    {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() && !name.starts_with('.') && name != "proto" {
            toml_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "toml") {
            out.push(p);
        }
    }
}

/// Loading and saving every fixture request reproduces the file byte for byte,
/// both through `Collection::save` and through the canonical writer itself.
#[test]
fn format_is_stable() {
    let mut files = Vec::new();
    toml_files(&root().join("tests/corpus/collection"), &mut files);
    assert!(files.len() >= 59);
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let coll = sendito_store::Collection::new(dir.path(), vec![]);
    for f in &files {
        let original = std::fs::read(f).unwrap_or_else(|e| panic!("{e}"));
        let text = String::from_utf8(original.clone()).unwrap_or_else(|e| panic!("{e}"));
        // Folder settings have their own canonical form.
        if f.file_name()
            .is_some_and(|n| n == sendito_store::folder::FILE)
        {
            let settings = sendito_store::folder::parse(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            assert_eq!(
                sendito_store::folder::to_toml(&settings),
                text,
                "{} is not in canonical form",
                f.display()
            );
            continue;
        }
        let req = sendito_store::request_file::parse(&text)
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let canonical = sendito_store::request_file::to_toml(&req);
        if std::env::var("UPDATE_FORMAT").is_ok() && canonical != text {
            std::fs::write(f, &canonical).unwrap_or_else(|e| panic!("{e}"));
            continue;
        }
        assert_eq!(
            canonical,
            text,
            "{} is not in canonical form (UPDATE_FORMAT=1 rewrites it)",
            f.display()
        );
        let rel = "copy.toml";
        std::fs::write(dir.path().join(rel), &original).unwrap_or_else(|e| panic!("{e}"));
        let loaded = coll.load(rel).unwrap_or_else(|e| panic!("{e}"));
        coll.save(rel, &loaded).unwrap_or_else(|e| panic!("{e}"));
        let after = std::fs::read(dir.path().join(rel)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(after, original, "{} changed on save", f.display());
    }
    // Environment files too.
    let envs = root().join("tests/corpus/collection/.sendito");
    for (name, p) in [
        ("corpus", envs.join("environments/corpus.toml")),
        ("_collection", envs.join("collection.toml")),
    ] {
        let text = read(&p);
        let e = sendito_store::env::parse_env(name, &text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            sendito_store::env::env_to_toml(&e),
            text,
            "{} is not canonical",
            p.display()
        );
    }
}

fn git_tracked_files() -> Vec<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("ls-files")
        .current_dir(root())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| root().join(l))
            .collect(),
        // Not a git checkout: scan the source tree instead.
        _ => {
            let mut v = Vec::new();
            fn walk(d: &Path, v: &mut Vec<PathBuf>) {
                for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                    let p = e.path();
                    let n = e.file_name().to_string_lossy().into_owned();
                    if p.is_dir() && n != "target" && n != ".git" {
                        walk(&p, v);
                    } else if p.is_file() {
                        v.push(p);
                    }
                }
            }
            walk(&root(), &mut v);
            v
        }
    }
}

/// Seeded secret values never reach history, curl exports or tracked files.
#[test]
fn secrets_stay_out_of_history_exports_and_repo() {
    let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| panic!("{e}"));
    let http = rt
        .block_on(sendito_testkit::http_echo::start())
        .unwrap_or_else(|e| panic!("{e}"));
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    sendito_store::init::init(dir.path()).unwrap_or_else(|e| panic!("{e}"));
    let secret_local = format!("local-{:032x}", fastrand::u128(..));
    let secret_kc = format!("kc-{:032x}", fastrand::u128(..));
    let secret_file = format!("file-{:032x}", fastrand::u128(..));
    std::fs::write(
        dir.path().join(".sendito/environments/dev.toml"),
        format!("[variables]\nbase = \"{}\"\ntok = {{ source = \"local\" }}\nkey = {{ source = \"keychain\" }}\nf = {{ source = \"file\", path = \"tok.txt\" }}\n", http.url()),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        dir.path().join(".sendito/secrets.toml"),
        format!("[dev]\ntok = \"{secret_local}\"\n"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(dir.path().join("tok.txt"), &secret_file).unwrap_or_else(|e| panic!("{e}"));
    let kc = std::sync::Arc::new(sendito_store::env::MemoryKeychain::default());
    sendito_store::env::Keychain::set(
        kc.as_ref(),
        &format!(
            "{}/dev/key",
            dir.path()
                .canonicalize()
                .unwrap_or_default()
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        &secret_kc,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        dir.path().join("r.toml"),
        "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base}}/x?t={{tok}}\"\n\n[[headers]]\nname = \"X-K\"\nvalue = \"{{key}}\"\n\n[[headers]]\nname = \"X-F\"\nvalue = \"{{f}}\"\n\n[auth]\ntype = \"basic\"\nusername = \"u\"\npassword = \"{{tok}}\"\n\n[body]\ntype = \"json\"\ntext = \"{\\\"t\\\": \\\"{{tok}}\\\"}\"\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let p = sendito_pipeline::runner::Project::open_with(dir.path(), kc, None);
    let Request::Http(req) = p.coll.load("r.toml").unwrap_or_else(|e| panic!("{e}")) else {
        panic!()
    };
    let (scope, errors) =
        sendito_pipeline::runner::build_scope(&p.envs, Some("dev"), &Default::default());
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = sendito_pipeline::runner::RunCtx {
        root: dir.path().to_path_buf(),
        send: p.send_options(),
        history: Some(sendito_pipeline::runner::HistorySink {
            path: dir.path().join(".sendito/history.jsonl"),
            max_entries: 100,
            max_body: 1024 * 1024,
        }),
        name: "r".into(),
        path: Some("r.toml".into()),
        env: Some("dev".into()),
        ..Default::default()
    };
    let o = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx,
        &req,
        scope.clone(),
        vec![],
    ));
    assert!(o.response.is_ok());
    // The wire carries the real values.
    let cap = http.last().unwrap_or_else(|| panic!("nothing received"));
    assert!(cap.target().contains(&secret_local));
    assert_eq!(cap.header("x-k").as_deref(), Some(secret_kc.as_str()));
    assert_eq!(cap.header("x-f").as_deref(), Some(secret_file.as_str()));
    let mut history = read(&dir.path().join(".sendito/history.jsonl"));
    // The response file holds the echo (target, headers, body) with the
    // secrets masked, also without `mask_echoed_secrets`.
    let responses: Vec<_> = std::fs::read_dir(dir.path().join(".sendito/history"))
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
        .collect();
    assert_eq!(responses.len(), 1);
    let kept = read(&responses[0].path());
    assert!(kept.contains("\"target\""), "{kept}");
    history.push_str(&kept);
    let curl = sendito_import::curl::export(&req, &scope, false, dir.path());
    let secrets = [&secret_local, &secret_kc, &secret_file];
    let mut hits = 0;
    for s in secrets {
        hits += history.matches(s.as_str()).count() + curl.matches(s.as_str()).count();
    }
    for f in git_tracked_files() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            for s in secrets {
                hits += text.matches(s.as_str()).count();
            }
        }
    }
    println!("seeded secret hits in history, curl export and tracked files: {hits}");
    assert_eq!(hits, 0);
    // Explicitly asked: curl with secrets includes them.
    let with = sendito_import::curl::export(&req, &scope, true, dir.path());
    assert!(with.contains(&secret_kc));
}

#[cfg(unix)]
#[test]
fn cookie_jar_fails_closed_for_insecure_file() {
    use std::os::unix::fs::PermissionsExt;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let http = rt.block_on(sendito_testkit::http_echo::start()).unwrap();
    let p = sendito_pipeline::runner::Project::open_with(
        dir.path(),
        sendito_pipeline::runner::default_keychain(),
        None,
    );
    let ctx = sendito_pipeline::runner::RunCtx {
        root: dir.path().to_path_buf(),
        send: p.send_options(),
        name: "test".into(),
        ..Default::default()
    };
    let mut req = sendito_core::HttpRequest {
        url: http.url(),
        ..Default::default()
    };
    req.options.cookies = Some(true);
    std::fs::create_dir_all(dir.path().join(".sendito")).unwrap();
    let jar = dir.path().join(".sendito/cookies.toml");
    std::fs::write(&jar, "[jars]\n").unwrap();
    std::fs::set_permissions(&jar, std::fs::Permissions::from_mode(0o644)).unwrap();
    let result = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx,
        &req,
        Scope::new(),
        vec![],
    ));
    assert!(result.response.is_err());
    assert!(http.take().is_empty());
}

/// The cookie jar: a Set-Cookie response stores cookies per environment, the
/// next request replays them, and `--no-cookies` semantics (cookie_jar off)
/// send nothing.
/// The time, size, hash, regex and cookie sources read a real HTTP
/// response; a cookie value stays masked in the results with the jar off.
#[test]
fn http_assertions_on_time_size_hash_regex_and_cookie() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let http = rt.block_on(sendito_testkit::http_echo::start()).unwrap();
    write(
        root,
        "check.toml",
        &format!(
            r#"kind = "http"
url = "{}/set-cookie"

[[scripts.assert]]
from = "time"
op = "lt"
value = "30000"

[[scripts.assert]]
from = "time"
expr = "ttfb"
op = "exists"

[[scripts.assert]]
from = "size"
expr = "body"
op = "gt"
value = "0"

[[scripts.assert]]
from = "size"
op = "gt"
value = "100"

[[scripts.assert]]
from = "hash"
expr = "sha256"
op = "matches"
value = "^[0-9a-f]{{64}}$"

[[scripts.assert]]
from = "regex"
expr = '"cookies_set": (\d+)'
op = "eq"
value = "2"

[[scripts.assert]]
from = "cookie"
expr = "session"
op = "eq"
value = "corpus-1"

[[scripts.assert]]
from = "cookie"
expr = "none"
op = "not_exists"
"#,
            http.url()
        ),
    );
    let p = sendito_pipeline::runner::Project::open_with(
        root,
        sendito_pipeline::runner::default_keychain(),
        None,
    );
    let Request::Http(req) = p.coll.load("check.toml").unwrap() else {
        panic!("not http")
    };
    let ctx = sendito_pipeline::runner::RunCtx {
        root: root.to_path_buf(),
        send: p.send_options(),
        name: "check".into(),
        path: Some("check.toml".into()),
        ..Default::default()
    };
    let o = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx,
        &req,
        Default::default(),
        vec![],
    ));
    assert!(o.response.is_ok(), "{:?}", o.response);
    let asserts = &o.scripts.asserts;
    assert_eq!(asserts.len(), 8);
    for a in asserts {
        assert!(a.pass, "{} failed: {}", a.label, a.actual);
    }
    let cookie = &asserts[6];
    assert_eq!(cookie.label, "cookie session == ••••••");
    assert!(!cookie.actual.contains("corpus-1"), "{}", cookie.actual);
    assert!(!root.join(".sendito/cookies.toml").exists());
}

#[test]
fn cookie_jar_stores_and_replays() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let http = rt.block_on(sendito_testkit::http_echo::start()).unwrap();
    write(
        root,
        ".sendito/environments/dev.toml",
        &format!("[variables]\nbase = \"{}\"\n", http.url()),
    );
    write(
        root,
        "login.toml",
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base}}/set-cookie\"\n\n[options]\ncookies = true\n",
    );
    write(
        root,
        "echo.toml",
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base}}/echo\"\n\n[options]\ncookies = true\n",
    );
    let p = sendito_pipeline::runner::Project::open_with(
        root,
        sendito_pipeline::runner::default_keychain(),
        None,
    );
    let ctx = |name: &str| sendito_pipeline::runner::RunCtx {
        root: root.to_path_buf(),
        send: p.send_options(),
        name: name.into(),
        path: Some(format!("{name}.toml")),
        env: Some("dev".into()),
        ..Default::default()
    };
    let login = p.coll.load("login.toml").unwrap();
    let echo = p.coll.load("echo.toml").unwrap();
    let (Request::Http(login), Request::Http(echo)) = (login, echo) else {
        panic!("not http")
    };
    let (scope, errors) =
        sendito_pipeline::runner::build_scope(&p.envs, Some("dev"), &Default::default());
    assert!(errors.is_empty(), "{errors:?}");
    // 1. The login response sets two cookies.
    let o = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx("login"),
        &login,
        scope.clone(),
        vec![],
    ));
    let resp = o
        .response
        .clone()
        .unwrap_or_else(|e| panic!("login failed: {e}"));
    assert_eq!(resp.status, 200);
    let text = read(&root.join(".sendito/cookies.toml"));
    assert!(text.contains("session"));
    assert!(text.contains("theme"));
    // 2. The next request replays them.
    let o = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx("echo"),
        &echo,
        scope.clone(),
        vec![],
    ));
    assert!(o.response.is_ok());
    let caps = http.take();
    let last = caps.last().unwrap();
    assert_eq!(
        last.header("cookie").as_deref(),
        Some("session=corpus-1; theme=dark"),
        "{}",
        last.head()
    );
    // The TUI must not expose Set-Cookie or its extracted value.
    let mut masked_ctx = ctx("login");
    masked_ctx.mask_echoed_secrets = true;
    let mut with_extract = login.clone();
    with_extract
        .common
        .scripts
        .extract
        .push(sendito_core::model::Extract {
            var: "cookie".into(),
            source: sendito_core::model::Source::Header,
            expr: "set-cookie".into(),
            secret: false,
        });
    let masked = rt.block_on(sendito_pipeline::runner::exec_http(
        &masked_ctx,
        &with_extract,
        scope.clone(),
        vec![],
    ));
    let extracted = masked.scripts.extracts[0].value.as_ref().unwrap();
    assert!(extracted.secret);
    let response = masked.response.unwrap();
    assert!(
        response
            .headers
            .iter()
            .filter(|(n, _)| n == "set-cookie")
            .all(|(_, v)| !v.contains("corpus-1") && !v.contains("dark"))
    );
    // A redirect is followed: each hop gets the jar's Cookie for its own URL,
    // and a cookie set by the redirect response goes to the next hop.
    http.take();
    let mut redirected = echo.clone();
    redirected.url = "{{base}}/login-redirect".into();
    let o = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx("redirect"),
        &redirected,
        scope.clone(),
        vec![],
    ));
    assert_eq!(o.response.unwrap().status, 200);
    let hops = http.take();
    assert_eq!(hops.len(), 2);
    let first = hops[0].header("cookie").unwrap_or_default();
    assert!(!first.contains("login="), "{first}");
    let second = hops[1].header("cookie").unwrap_or_default();
    assert!(second.contains("login=corpus-2"), "{second}");
    assert!(second.contains("session=corpus-1"), "{second}");
    // 3. Without `cookies = true` nothing is sent or stored.
    let mut plain = echo.clone();
    plain.options.cookies = Some(false);
    let o = rt.block_on(sendito_pipeline::runner::exec_http(
        &ctx("echo"),
        &plain,
        scope,
        vec![],
    ));
    assert!(o.response.is_ok());
    let last = http.take().last().unwrap().clone();
    assert!(last.header("cookie").is_none(), "{}", last.head());
}

/// `sendito run <folder>` runs every request in tree order, chains extracted
/// variables into the following requests and aggregates the exit code.
#[test]
fn cli_runs_a_folder_and_chains_variables() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("Users")).unwrap();
        std::fs::create_dir_all(dir.path().join("Health")).unwrap();
        std::fs::write(
            dir.path().join(".sendito/environments/local.toml"),
            format!("[variables]\nbase_url = \"{}\"\n", echo.url()),
        )
        .unwrap();
        // Extracts `user_id` from the echo of its own target.
        std::fs::write(
            dir.path().join("Users/create user.toml"),
            "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base_url}}/users\"\n        \n[body]\ntype = \"json\"\ntext = '''{\"name\":\"ada\"}'''\n\n[[scripts.extract]]\nvar = \"user_id\"\nfrom = \"jsonpath\"\nexpr = \"$.target\"\n",
        )
        .unwrap();
        // Uses the value the previous request extracted.
        std::fs::write(
            dir.path().join("Users/list users.toml"),
            "kind = \"http\"\nurl = \"{{base_url}}/users?id={{user_id}}\"\n\n[[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"200\"\n",
        )
        .unwrap();
        // A failing assertion: recorded as a failure, the run continues.
        std::fs::write(
            dir.path().join("Health/boom.toml"),
            "kind = \"http\"\nurl = \"{{base_url}}/status/500\"\n\n[[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"200\"\n",
        )
        .unwrap();

        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(dir.path())
                .args(args)
                .env("SENDITO_KEYCHAIN", "memory")
                .output()
                .unwrap()
        };

        // The folder run: both requests pass, in tree order, and the second
        // one sees `{{user_id}}` set by the first.
        let out = run(&["run", "Users", "--env", "local", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        let first = stderr.find("PASS  Users/create user.toml").unwrap();
        let second = stderr.find("PASS  Users/list users.toml").unwrap();
        assert!(first < second, "tree order expected:\n{stderr}");
        assert!(stderr.contains("2 passed, 0 failed, 0 error(s)"), "{stderr}");
        // Bodies are not printed in a folder run; the status lines are.
        assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
        let sent: Vec<String> = echo.take().iter().map(|c| c.target()).collect();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[1], "/users?id=/users");

        // A failed assertion exits 1 and names the request.
        let out = run(&["run", ".", "--env", "local", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("FAIL  Health/boom.toml"), "{stderr}");
        assert!(
            stderr.contains("status == 200  (actual: 500)"),
            "{stderr}"
        );
        assert!(stderr.contains("2 passed, 1 failed"), "{stderr}");

        // The JSON report goes to stdout and carries one entry per request.
        let out = run(&[
            "run",
            ".",
            "--env",
            "local",
            "--no-history",
            "--report",
            "json",
        ]);
        let report: serde_json::Value =
            serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report["total"], 3);
        assert_eq!(report["passed"], 2);
        assert_eq!(report["failed"], 1);
        assert_eq!(report["errors"], 0);
        assert_eq!(report["environment"], "local");
        let paths: Vec<&str> = report["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            vec!["Health/boom.toml", "Users/create user.toml", "Users/list users.toml"],
            "{report}"
        );

        // --report-file writes several formats in one run; stdout stays empty.
        let out = run(&[
            "run",
            ".",
            "--env",
            "local",
            "--no-history",
            "--report-file",
            "junit=out/results.xml",
            "--report-file",
            "html=report.html",
        ]);
        // A missing directory is a write error: exit 3, the other file is written.
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(stderr.contains("cannot write out/results.xml"), "{stderr}");
        assert!(out.stdout.is_empty());
        let html = std::fs::read_to_string(dir.path().join("report.html")).unwrap();
        assert!(html.contains("2 passed, 1 failed, 0 error(s)"), "{html}");
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        let out = run(&[
            "run",
            ".",
            "--env",
            "local",
            "--no-history",
            "--report-file",
            "junit=out/results.xml",
            "--report",
            "tap",
        ]);
        assert_eq!(out.status.code(), Some(1));
        let junit = std::fs::read_to_string(dir.path().join("out/results.xml")).unwrap();
        assert!(junit.contains(r#"tests="3" failures="1" errors="0""#), "{junit}");
        assert!(junit.contains(r#"<testcase classname="Health" name="Health/boom.toml""#));
        assert!(junit.contains("status == 200  (actual: 500)"), "{junit}");
        let tap = String::from_utf8_lossy(&out.stdout);
        assert!(tap.starts_with("TAP version 13\n1..3\nnot ok 1 - Health/boom.toml"), "{tap}");
        // A bad format is a usage error from the argument parser.
        let out = run(&["run", ".", "--report-file", "text=x"]);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown report format"));

        // --fail-fast stops at the first failure.
        let out = run(&[
            "run",
            ".",
            "--env",
            "local",
            "--no-history",
            "--fail-fast",
        ]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        // Health/boom is first in tree order: the run stops right there.
        assert!(stderr.contains("FAIL  Health/boom.toml"), "{stderr}");
        assert!(stderr.contains("0 passed, 1 failed, 0 error(s)"), "{stderr}");
        assert!(!stderr.contains("Users/list users.toml"), "{stderr}");
        assert_eq!(out.status.code(), Some(1), "{stderr}");

        // Unresolved variables make a folder run a usage error (3).
        let out = run(&["run", "Users", "--no-history"]);
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(stderr.contains("unresolved variables"), "{stderr}");

        // An unknown environment on a folder reports the environment, not a
        // missing request.
        let out = run(&["run", "Users", "--env", "nope"]);
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            stderr.contains("no environment named \"nope\""),
            "{stderr}"
        );
        assert!(!stderr.contains("request Users not found"), "{stderr}");

        // A folder without requests and an unknown folder are usage errors.
        std::fs::create_dir_all(dir.path().join("Empty")).unwrap();
        let out = run(&["run", "Empty", "--env", "local"]);
        assert_eq!(out.status.code(), Some(3));
        let out = run(&["run", "Nope", "--env", "local"]);
        assert_eq!(out.status.code(), Some(3));

        // A directory outside the collection is a usage error, never a
        // whole-collection run. A lexical `..` used to make `find_root` pick
        // the current collection and run everything under it.
        let outside = tempfile::tempdir().unwrap();
        for arg in [
            outside.path().to_path_buf(),
            std::path::Path::new("..").join(outside.path().file_name().unwrap()),
        ] {
            let arg = arg.to_str().unwrap();
            let out = run(&["run", arg, "--no-history"]);
            assert_eq!(out.status.code(), Some(3), "{arg}");
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(
                stderr.contains("is not inside a Sendito collection"),
                "{arg}: {stderr}"
            );
            assert!(!stderr.contains("PASS "), "{arg}: {stderr}");
        }
    });
}

/// `[options] retry` sends again after a failed assertion; `--retry` replaces
/// it; `--repeat` runs the folder again with the shared scope; a single
/// request writes only the output of the attempt that counts.
#[test]
fn cli_retries_repeats_and_delays() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("F")).unwrap();
        let req = |key: &str, extra: &str| {
            format!(
                "kind = \"http\"\nurl = \"{}/flaky/{key}/2\"\n{extra}\n[[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"200\"\n",
                echo.url()
            )
        };
        let opts = "\n[options]\nretry = 2\nretry_interval_ms = 10\n";
        std::fs::write(dir.path().join("F/a.toml"), req("a", opts)).unwrap();
        // The file keeps the keys in canonical form.
        let parsed = sendito_store::request_file::parse(&req("a", opts)).unwrap();
        assert_eq!(parsed.common().retry, 2);
        assert_eq!(parsed.common().retry_interval_ms, Some(10));
        assert!(sendito_store::request_file::to_toml(&parsed).contains("[options]\nretry = 2\nretry_interval_ms = 10\n"));

        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(dir.path())
                .args(args)
                .env("SENDITO_KEYCHAIN", "memory")
                .output()
                .unwrap()
        };

        // Two 500s, then 200 on the third attempt.
        let out = run(&["run", "F", "--no-history", "--report", "json"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("RETRY F/a.toml  (500 Internal Server Error, attempt 1/3"), "{stderr}");
        assert!(stderr.contains("PASS  F/a.toml  (200 OK"), "{stderr}");
        assert!(stderr.contains("attempt 3/3)"), "{stderr}");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["requests"][0]["attempts"], 3);

        // --retry 0 replaces the file: one attempt, a failure.
        std::fs::write(dir.path().join("F/a.toml"), req("b", opts)).unwrap();
        let out = run(&["run", "F", "--no-history", "--retry", "0"]);
        assert_eq!(out.status.code(), Some(1));

        // --repeat 2 runs the folder twice and names each round.
        std::fs::write(dir.path().join("F/a.toml"), req("c", "")).unwrap();
        let out = run(&["run", "F", "--no-history", "--repeat", "3", "--delay", "5", "--report", "tap"]);
        let tap = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(out.status.code(), Some(1), "{tap}");
        assert!(tap.contains("1..3\nnot ok 1 - F/a.toml\\#1\n"), "{tap}");
        assert!(tap.contains("not ok 2 - F/a.toml\\#2\n"), "{tap}");
        assert!(tap.contains("ok 3 - F/a.toml\\#3\n"), "{tap}");

        // A single request: the body of the passing attempt only.
        std::fs::write(dir.path().join("F/a.toml"), req("d", opts)).unwrap();
        let out = run(&["run", "F/a", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("attempt 2/3 did not pass; retrying in 10 ms"), "{stderr}");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(stdout.matches("\"status\"").count(), 1, "{stdout}");
        assert!(stdout.contains("\"hit\": 3"), "{stdout}");
    });
}

/// A timeout that a setting causes: stderr and the JSON report name the
/// setting to change (`hint`).
#[test]
fn cli_hint_for_a_timeout() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("F")).unwrap();
        std::fs::write(
            dir.path().join("F/slow.toml"),
            format!(
                "kind = \"http\"\nurl = \"{}/delay/800\"\n[options]\ntimeout_ms = 50\n",
                echo.url()
            ),
        )
        .unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .current_dir(dir.path())
            .args(["run", "F", "--no-history", "--report", "json"])
            .env("SENDITO_KEYCHAIN", "memory")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        let fix = "The timeout of 50 ms is too short. Set a larger `[options] timeout_ms` in the request, or remove it to use `[http] timeout_ms` in the config.";
        assert!(stderr.contains(&format!("hint: {fix}")), "{stderr}");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["requests"][0]["outcome"], "error");
        assert_eq!(report["requests"][0]["hint"], fix);
    });
}

/// `--debug=FILE` writes the start line, one line per send (also with
/// `--no-history`), the error and the exit code; stderr names the file.
#[test]
fn cli_debug_writes_a_log_file() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let down = format!("http://{}/down", closed.local_addr().unwrap());
        drop(closed);
        sendito_store::init::init(dir.path()).unwrap();
        write(
            dir.path(),
            "F/a_ok.toml",
            &format!("kind = \"http\"\nurl = \"{}/get\"\n", echo.url()),
        );
        write(
            dir.path(),
            "F/b_down.toml",
            &format!("kind = \"http\"\nurl = \"{down}\"\n"),
        );
        // Two sends that stop before they go out and write no history.
        write(
            dir.path(),
            "F/c_script.toml",
            &format!(
                "kind = \"http\"\nurl = \"{}/c\"\n[scripts.lua]\npre = 'error(\"boom\")'\n",
                echo.url()
            ),
        );
        write(
            dir.path(),
            "F/d_file.toml",
            &format!(
                "kind = \"http\"\nmethod = \"POST\"\nurl = \"{}/d\"\n[body]\ntype = \"file\"\npath = \"missing.bin\"\n",
                echo.url()
            ),
        );
        let log = dir.path().join("logs/debug.log");
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .current_dir(dir.path())
            .args(["run", "F", "--no-history"])
            .arg(format!("--debug={}", log.display()))
            .env("SENDITO_KEYCHAIN", "memory")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        assert!(
            stderr.contains(&format!("sendito: debug log: {}", log.display())),
            "{stderr}"
        );
        let text = read(&log);
        assert!(text.contains(" START sendito "), "{text}");
        assert!(
            text.contains(&format!(" SEND GET {}/get -> 200 (", echo.url())),
            "{text}"
        );
        assert!(
            text.contains(&format!(" ERROR GET {down} -> error: ")),
            "{text}"
        );
        assert!(text.contains(" path=F/b_down.toml"), "{text}");
        assert!(
            text.contains(&format!(" ERROR GET {}/c -> error: request script:1: boom", echo.url())),
            "{text}"
        );
        assert!(
            text.contains(&format!(" ERROR POST {}/d -> error: ", echo.url())),
            "{text}"
        );
        assert!(text.contains("missing.bin"), "{text}");
        assert!(text.contains(" EXIT exit code 2"), "{text}");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_grpc_not_found_hint() {
    let grpc = sendito_testkit::grpc_echo::start().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    sendito_store::init::init(dir.path()).unwrap();
    for (name, message, has_hint) in [
        ("route", "unknown method: Unary", true),
        ("entity", "user 42 not found", false),
    ] {
        write(
            dir.path(),
            &format!("{name}/call.toml"),
            &format!(
                "kind = \"grpc\"\ntarget = \"{}\"\nservice = \"sendito.test.v1.Echo\"\nmethod = \"Unary\"\nmessage = '''{{\"fail_code\":5,\"fail_message\":\"{message}\"}}'''\n",
                grpc.target()
            ),
        );
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .current_dir(dir.path())
            .args(["run", name, "--no-history", "--report", "json"])
            .env("SENDITO_KEYCHAIN", "memory")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let entry = &report["requests"][0];
        assert_eq!(entry["status"], format!("NOT_FOUND: {message}"));
        assert_eq!(entry.get("hint").is_some(), has_hint, "{report}");
        assert_eq!(
            stderr.contains("hint: Check `service` and `method`"),
            has_hint
        );
    }
}

/// Lua scripts in `sendito run`: the folder script runs first, a variable
/// only a pre script sets is not unresolved, post-script variables chain into
/// the next request, a failed `test` exits 1 and a pre-script error exits 2.
#[test]
fn cli_runs_lua_scripts() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::write(
            dir.path().join(".sendito/environments/local.toml"),
            format!("[variables]\nbase_url = \"{}\"\n", echo.url()),
        )
        .unwrap();
        write(
            dir.path(),
            "Api/folder.toml",
            "[scripts.lua]\npre = '''\nrequest:set_header(\"X-Folder\", \"f\")\nvars.set(\"uid\", \"u-1\")\n'''\n",
        );
        write(
            dir.path(),
            "Api/1 create.toml",
            r#"kind = "http"
method = "POST"
url = "{{base_url}}/users/{{uid}}"

[scripts.lua]
pre = '''
request:set_header("X-Uid", "{{uid}}" .. "-" .. vars.get("uid"))
print("sending " .. vars.get("uid"))
'''
post = '''
test("ok", response.status == 200)
vars.set("seen", response:json().target)
'''
"#,
        );
        write(
            dir.path(),
            "Api/2 next.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/next?t={{seen}}\"\n",
        );
        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(dir.path())
                .args(args)
                .env("SENDITO_KEYCHAIN", "memory")
                .output()
                .unwrap()
        };

        let out = run(&["run", "Api", "--env", "local", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("2 passed, 0 failed, 0 error(s)"), "{stderr}");
        let sent = echo.take();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].target(), "/users/u-1");
        assert_eq!(sent[0].header("x-folder").as_deref(), Some("f"));
        assert_eq!(sent[0].header("x-uid").as_deref(), Some("u-1-u-1"));
        assert_eq!(sent[1].target(), "/next?t=/users/u-1");

        // One request: the log and the test results go to stderr.
        let out = run(&["run", "Api/1 create.toml", "--env", "local", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("log: sending u-1"), "{stderr}");
        assert!(stderr.contains("✓ test: ok"), "{stderr}");
        echo.take();

        // A failed test exits 1.
        write(
            dir.path(),
            "fail.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/x\"\n\n[scripts.lua]\npost = 'test(\"never\", false)'\n",
        );
        let out = run(&["run", "fail.toml", "--env", "local", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("✗ test: never"), "{stderr}");
        echo.take();

        // A pre-script error stops the send and exits 2.
        write(
            dir.path(),
            "stop.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/x\"\n\n[scripts.lua]\npre = 'error(\"no token\")'\n",
        );
        let out = run(&["run", "stop.toml", "--env", "local", "--no-history"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains("request script:1: no token"), "{stderr}");
        assert!(echo.take().is_empty(), "the request must not be sent");
    });
}

/// Folder runs also drive gRPC and MCP calls. The update channel is only fed
/// while the call future is polled, so awaiting the updates first hangs the
/// whole run.
#[test]
fn cli_runs_grpc_and_mcp_in_a_folder() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let grpc = sendito_testkit::grpc_echo::start().await.unwrap();
        let mcp = sendito_testkit::mcp_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("Rpc")).unwrap();
        std::fs::write(
            dir.path().join(".sendito/environments/local.toml"),
            format!(
                "[variables]\ngrpc_target = \"{}\"\nmcp_url = \"{}\"\n",
                grpc.target(),
                mcp.url()
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".sendito/config.toml"),
            "[collection]\nname = \"suite\"\n\n[mcp.servers.echo]\nurl = \"{{mcp_url}}/mcp\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Rpc/call.toml"),
            "kind = \"grpc\"\ntarget = \"{{grpc_target}}\"\nservice = \"sendito.test.v1.Echo\"\nmethod = \"Unary\"\nmessage = '''{\"text\":\"hello\"}'''\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Rpc/tool.toml"),
            "kind = \"mcp\"\nserver = \"echo\"\ntool = \"greet\"\narguments = '''{\"name\":\"ada\"}'''\n",
        )
        .unwrap();

        let out = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .current_dir(dir.path())
            .args(["run", "Rpc", "--env", "local", "--no-history"])
            .env("SENDITO_KEYCHAIN", "memory")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("PASS  Rpc/call.toml"), "{stderr}");
        assert!(stderr.contains("PASS  Rpc/tool.toml"), "{stderr}");
        assert!(stderr.contains("2 passed, 0 failed, 0 error(s)"), "{stderr}");
    });
}

#[test]
fn cli_import_outside_a_collection_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
        .current_dir(dir.path())
        .arg("import")
        .arg(root().join("tests/fixtures/insomnia.json"))
        .env("SENDITO_KEYCHAIN", "memory")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no collection here"), "{stderr}");
    assert!(!dir.path().join(".sendito").exists());
}

/// `sendito code` and `sendito har` export with the folder settings applied
/// and secrets masked; a HAR file imports back into a collection.
#[test]
fn cli_exports_code_and_har_and_imports_har() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    sendito_store::init::init(root).unwrap();
    std::fs::write(
        root.join(".sendito/environments/local.toml"),
        "[variables]\nbase = \"https://api.test\"\ntoken = { source = \"local\" }\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".sendito/secrets.toml"),
        "[local]\ntoken = \"s3cr3t\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("Users")).unwrap();
    std::fs::write(
        root.join("Users/folder.toml"),
        "[[headers]]\nname = \"X-Tenant\"\nvalue = \"acme\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Users/create.toml"),
        "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base}}/users\"\n\n[[headers]]\nname = \"Authorization\"\nvalue = \"Bearer {{token}}\"\n\n[body]\ntype = \"json\"\ntext = '{\"a\":1}'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Users/call.toml"),
        "kind = \"grpc\"\ntarget = \"localhost:1\"\nservice = \"a.B\"\nmethod = \"C\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .current_dir(root)
            .args(args)
            .env("SENDITO_KEYCHAIN", "memory")
            .output()
            .unwrap()
    };

    let out = run(&["code", "Users/create", "--lang", "python", "--env", "local"]);
    let py = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(py.contains("\"https://api.test/users\""), "{py}");
    assert!(py.contains("\"X-Tenant\": \"acme\""), "{py}");
    assert!(
        py.contains("Bearer {{token}}") && !py.contains("s3cr3t"),
        "{py}"
    );
    let out = run(&[
        "code",
        "Users/create",
        "-l",
        "go",
        "-e",
        "local",
        "--include-secrets",
    ]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Bearer s3cr3t"));
    let out = run(&["code", "Users/call", "--lang", "go"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("only available for HTTP requests, not GRPC")
    );
    let out = run(&["code", "Users/create", "--lang", "ruby"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown language"));

    // The folder: the gRPC request is left out with a note.
    let out = run(&["har", "Users", "--env", "local"]);
    let har = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("a GRPC request is not in a HAR file"));
    let doc: serde_json::Value = serde_json::from_str(&har).unwrap();
    assert_eq!(doc["log"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(doc["log"]["entries"][0]["comment"], "create");
    assert!(!har.contains("s3cr3t"), "{har}");

    // The HAR file imports back.
    std::fs::write(root.join("export.har"), &har).unwrap();
    let out = run(&["import", "export.har", "--into", "Back"]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("imported 1 requests") && stdout.contains("; HAR)"),
        "{stdout}"
    );
    let mut files = Vec::new();
    toml_files(&root.join("Back"), &mut files);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with("create.toml"), "{files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("url = \"https://api.test/users\""), "{text}");
}

#[test]
fn cli_prints_completions_and_the_man_page() {
    for args in [&["completions", "zsh"][..], &["man"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("run"), "{args:?}: {stdout}");
    }
}

#[test]
fn cli_imports_bruno_and_insomnia_without_overwriting_environments() {
    let dir = tempfile::tempdir().unwrap();
    sendito_store::init::init(dir.path()).unwrap();
    for (fixture, label) in [
        ("tests/fixtures/bruno", "Bruno"),
        ("tests/fixtures/insomnia.json", "Insomnia"),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
            .current_dir(dir.path())
            .arg("import")
            .arg(root().join(fixture))
            .env("SENDITO_KEYCHAIN", "memory")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(label), "{stdout}");
        assert!(!stdout.contains("private-placeholder"));
    }
    let coll = sendito_store::Collection::new(dir.path(), vec![]);
    let Request::Http(bruno) = coll.load("Bruno demo/Users/List.toml").unwrap() else {
        panic!("expected http")
    };
    assert!(!bruno.headers[1].enabled);
    let Request::Http(insomnia) = coll.load("Insomnia demo/Users/Create.toml").unwrap() else {
        panic!("expected http")
    };
    assert_eq!(insomnia.url, "{{base}}/users");
    assert!(!insomnia.headers[1].enabled);
    let envs = sendito_store::env::Envs::new(
        dir.path(),
        "test",
        std::sync::Arc::new(sendito_store::env::MemoryKeychain::default()),
    );
    assert!(envs.list().iter().any(|e| e == "dev"));
    assert!(envs.list().iter().any(|e| e == "dev (2)"));
    for name in envs.list() {
        let text = std::fs::read_to_string(envs.env_dir().join(format!("{name}.toml"))).unwrap();
        assert!(!text.contains("private-placeholder"));
    }
}

#[test]
fn http_oauth_login_refresh_and_send() {
    use sendito_core::model::{Auth, HttpRequest, KeyValue};
    use sendito_core::resolve::{ResolvedBody, ResolvedHttp};
    use sendito_store::env::{Keychain, MemoryKeychain};
    use std::sync::Arc;
    use std::time::Duration;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let srv = sendito_testkit::mcp_auth::start().await.unwrap();
        let mut settings = sendito_mcp::oauth::OAuthSettings {
            client_id: Some("cli-1".into()),
            client_secret: Some("test-client-secret".into()),
            authorize_url: Some(format!("{}/authorize-http", srv.url())),
            token_url: Some(format!("{}/token-http", srv.url())),
            ..Default::default()
        };
        let opener = |url: &str| {
            let url = url.to_owned();
            tokio::spawn(async move {
                let request = ResolvedHttp {
                    method: "GET".into(),
                    url,
                    headers: vec![],
                    header_origins: vec![],
                    body: ResolvedBody::None,
                    timeout_ms: Some(5000),
                    follow_redirects: true,
                    verify_tls: true,
                    unresolved: vec![],
                    notes: vec![],
                    signed: None,
                };
                let _ = sendito_http::send(&request, &Default::default()).await;
            });
            true
        };
        let mut tokens =
            sendito_mcp::oauth::login_http(&settings, Duration::from_secs(5), &opener, &|_| {})
                .await
                .unwrap();
        // Force a 401 to exercise the one-retry path, rather than a pre-send refresh.
        tokens.expires_at = None;
        let kc = Arc::new(MemoryKeychain::default());
        let mut p = sendito_pipeline::runner::Project::open_with(dir.path(), kc.clone(), None);
        p.config.oauth.insert(
            "service".into(),
            sendito_store::config::OAuthIdentity {
                authorize_url: settings.authorize_url.take().unwrap(),
                token_url: settings.token_url.take().unwrap(),
                client_id: "cli-1".into(),
                client_secret: Some("test-client-secret".into()),
                ..Default::default()
            },
        );
        let account =
            sendito_pipeline::runner::oauth_token_account(&p.name, Some("dev"), "service");
        kc.set(&account, &tokens.to_json()).unwrap();
        let ctx = sendito_pipeline::runner::RunCtx {
            root: dir.path().to_path_buf(),
            send: p.send_options(),
            name: "oauth".into(),
            env: Some("dev".into()),
            mask_echoed_secrets: true,
            oauth: Some(sendito_pipeline::runner::OAuthRunInfo::of(&p)),
            ..Default::default()
        };
        let req = HttpRequest {
            url: format!("{}/http-api", srv.url()),
            auth: Auth::OAuth {
                name: "service".into(),
            },
            ..Default::default()
        };
        let mut req = req;
        // The jar is on: the 401 sets a cookie and the retry must replay it.
        req.options.cookies = Some(true);
        req.common
            .scripts
            .extract
            .push(sendito_core::model::Extract {
                var: "reflected_token".into(),
                source: sendito_core::model::Source::Body,
                expr: "$.token".into(),
                secret: false,
            });
        let o = sendito_pipeline::runner::exec_http(&ctx, &req, Scope::new(), vec![]).await;
        assert_eq!(o.response.as_ref().unwrap().status, 200);
        assert!(o.scripts.extracts[0].value.as_ref().unwrap().secret);
        assert_eq!(srv.http_requests(), 2);
        assert_eq!(srv.refreshes(), 1);
        let refreshed = kc.get(&account).unwrap().unwrap();
        let token = sendito_mcp::oauth::Tokens::from_json(&refreshed).unwrap();
        assert!(!o.doc.raw.text.contains(&token.access));
        assert_ne!(refreshed, tokens.to_json());
        // Export never includes the keychain token, even with include-secrets.
        let curl = sendito_import::curl::export(&req, &Scope::new(), true, dir.path());
        assert!(curl.contains("{{oauth.service}}"));
        assert!(!curl.contains(&tokens.access));
        let file =
            sendito_import::http_file::export("test", &req, &Scope::new(), false, dir.path());
        assert!(file.contains("{{oauth.service}}"));
        // An explicitly configured Authorization wins without fetching a token.
        let mut explicit = req.clone();
        explicit
            .headers
            .push(KeyValue::new("Authorization", "Bearer explicit"));
        let o = sendito_pipeline::runner::exec_http(&ctx, &explicit, Scope::new(), vec![]).await;
        assert_eq!(o.response.unwrap().status, 401);
        assert_eq!(srv.http_requests(), 3);
        assert_eq!(srv.refreshes(), 1);
        // Different environments cannot use each other's stored tokens.
        let mut other = ctx;
        other.env = Some("other".into());
        let o = sendito_pipeline::runner::exec_http(&other, &req, Scope::new(), vec![]).await;
        assert!(o.response.is_err());
        assert_eq!(srv.http_requests(), 3);
    });
}

/// A client_credentials identity needs no sign-in: the first send gets a
/// token, and the 401 retry gets a new one (no refresh token).
#[test]
fn http_oauth_client_credentials_needs_no_login() {
    use sendito_core::model::{Auth, HttpRequest};
    use sendito_store::env::MemoryKeychain;
    use std::sync::Arc;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let srv = sendito_testkit::mcp_auth::start().await.unwrap();
        let mut p = sendito_pipeline::runner::Project::open_with(
            dir.path(),
            Arc::new(MemoryKeychain::default()),
            None,
        );
        p.config.oauth.insert(
            "machine".into(),
            sendito_store::config::OAuthIdentity {
                grant: sendito_store::config::OAuthGrant::ClientCredentials,
                token_url: format!("{}/token-http", srv.url()),
                client_id: "cli-1".into(),
                client_secret: Some("test-client-secret".into()),
                ..Default::default()
            },
        );
        let ctx = sendito_pipeline::runner::RunCtx {
            root: dir.path().to_path_buf(),
            send: p.send_options(),
            name: "machine".into(),
            mask_echoed_secrets: true,
            oauth: Some(sendito_pipeline::runner::OAuthRunInfo::of(&p)),
            ..Default::default()
        };
        let mut req = HttpRequest {
            url: format!("{}/http-api", srv.url()),
            auth: Auth::OAuth {
                name: "machine".into(),
            },
            ..Default::default()
        };
        req.options.cookies = Some(true);
        let o = sendito_pipeline::runner::exec_http(&ctx, &req, Scope::new(), vec![]).await;
        assert_eq!(
            o.response.as_ref().map(|r| r.status).ok(),
            Some(200),
            "{:?}",
            o.response.err()
        );
        assert_eq!(srv.http_requests(), 2);
        assert_eq!(srv.grants(), 2);
        assert_eq!(srv.logins(), 0);
    });
}

/// A WebSocket session replays the collection cookie jar: the handshake carries
/// one `Cookie` header. The jar only speaks HTTP, so a ws:// URL must be mapped
/// to its http:// equivalent instead of being refused.
#[test]
fn ws_session_replays_the_cookie_jar() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ws = rt.block_on(sendito_testkit::ws_echo::start()).unwrap();
    write(
        root,
        ".sendito/environments/dev.toml",
        &format!("[variables]\nbase = \"{}\"\n", ws.url("")),
    );
    write(
        root,
        "chat.toml",
        "kind = \"websocket\"\nurl = \"{{base}}/echo\"\nmessages = '''\"hi\"'''\n\n[options]\ncookies = true\n",
    );
    let p = sendito_pipeline::runner::Project::open_with(
        root,
        sendito_pipeline::runner::default_keychain(),
        None,
    );
    let ctx = sendito_pipeline::runner::RunCtx {
        root: root.to_path_buf(),
        send: p.send_options(),
        name: "chat".into(),
        path: Some("chat.toml".into()),
        env: Some("dev".into()),
        ..Default::default()
    };
    let Request::Ws(req) = p.coll.load("chat.toml").unwrap() else {
        panic!("not websocket")
    };
    // Seed the jar with a cookie for the ws host (stored under http://).
    let http_url = ws.url("/").replacen("ws://", "http://", 1);
    sendito_store::cookies::store_response(root, "dev", &http_url, &["sid=corpus-1".into()])
        .unwrap();
    let (scope, errors) =
        sendito_pipeline::runner::build_scope(&p.envs, Some("dev"), &Default::default());
    assert!(errors.is_empty(), "{errors:?}");
    let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
    let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(ctl_tx);
    rt.block_on(async {
        let run = sendito_pipeline::runner::exec_ws(&ctx, &req, scope, vec![], utx, ctl_rx);
        let collect = async { while urx.recv().await.is_some() {} };
        tokio::join!(run, collect);
    });
    let caps = ws.take();
    let cap = caps.first().expect("a session reached the server");
    let head = cap.head();
    assert!(
        head.to_ascii_lowercase().contains("cookie: sid=corpus-1"),
        "{head}"
    );
}

/// An OpenAPI YAML file imports through the same path as the other formats:
/// the document's security goes to the root `folder.toml`, the response
/// examples next to their requests, the servers to environments.
#[test]
fn openapi_yaml_file_imports_into_the_collection() {
    let dir = tempfile::tempdir().unwrap();
    sendito_store::init::init(dir.path()).unwrap();
    let spec = dir.path().join("spec.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.1.0\ninfo: {title: Shop}\nservers:\n  - {url: 'https://shop.example.com', description: prod}\n  - {url: 'http://localhost:3000', description: local}\nsecurity: [{bearer: []}]\npaths:\n  /orders/{id}:\n    get:\n      operationId: getOrder\n      tags: [orders]\n      parameters: [{name: id, in: path, required: true}]\n      responses:\n        '200':\n          description: ok\n          content: {application/json: {example: {id: 1}}}\ncomponents:\n  securitySchemes:\n    bearer: {type: http, scheme: bearer}\n",
    )
    .unwrap();
    let (format, result) = sendito_import::import_path(&spec).unwrap();
    assert_eq!(format, sendito_import::ImportFormat::OpenApi);
    let coll = sendito_store::Collection::new(dir.path(), vec![]);
    let envs = sendito_store::env::Envs::new(
        dir.path(),
        "t",
        std::sync::Arc::new(sendito_store::env::MemoryKeychain::default()),
    );
    let rep = sendito_import::write(&result, &coll, Some(&envs), "").unwrap();
    assert_eq!((rep.requests, rep.environments), (1, 2));
    let chain = coll.chain("Shop/orders/getOrder.toml").unwrap();
    assert!(matches!(chain[0].1.auth, sendito_core::Auth::Bearer { .. }));
    assert_eq!(
        sendito_store::examples::list(&coll, "Shop/orders/getOrder.toml").unwrap(),
        ["200"]
    );
    let Request::Http(req) = coll.load("Shop/orders/getOrder.toml").unwrap() else {
        panic!()
    };
    assert_eq!(req.url, "{{baseUrl}}/orders/{{id}}");
    assert!(envs.list().iter().any(|e| e == "prod"));
}

/// `$prompt` and `$select` in `sendito run`: `--prompt LABEL=VALUE` answers;
/// with no flag and no terminal the send stops (exit 3) and names the flag; a
/// value that is not an option of its `$select` is refused.
#[test]
fn cli_prompt_flags_answer_prompt_and_select() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        write(
            dir.path(),
            "ask.toml",
            &format!(
                "kind = \"http\"\nurl = \"{}/o/{{{{ $prompt('Order ID') }}}}?m={{{{ $select('Mode', 'a', 'b') }}}}\"\n",
                echo.url()
            ),
        );
        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(dir.path())
                .args(args)
                .env("SENDITO_KEYCHAIN", "memory")
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap()
        };
        let out = run(&[
            "run",
            "ask",
            "--no-history",
            "--prompt",
            "Order ID=42",
            "--prompt",
            "Mode=b",
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(echo.last().unwrap().target(), "/o/42?m=b");

        let out = run(&["run", "ask", "--no-history", "--prompt", "Mode=a"]);
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("--prompt 'Order ID=…'"), "{stderr}");

        let out = run(&[
            "run",
            "ask",
            "--no-history",
            "--prompt",
            "Order ID=1",
            "--prompt",
            "Mode=c",
        ]);
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("\"c\" is not an option of \"Mode\" (a, b)"),
            "{stderr}"
        );

        // A folder run reports a missing value as an error of the request.
        let out = run(&["run", ".", "--no-history", "--prompt", "Mode=a"]);
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("ERROR ask.toml  no value to send for --prompt 'Order ID=…'"),
            "{stderr}"
        );
    });
}

/// `$response` in `sendito run`: the dependency is sent first and its value
/// is used; the next process reads the history (`if_missing`); `always`
/// sends again; `never` with no response, a cycle, a dependency that is not
/// HTTP and a JSONPath with no match stop the send with the reason.
#[test]
fn cli_response_sends_reads_history_and_reports_errors() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::write(
            dir.path().join(".sendito/environments/local.toml"),
            format!("[variables]\nbase_url = \"{}\"\n", echo.url()),
        )
        .unwrap();
        write(
            dir.path(),
            "Auth/login.toml",
            "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base_url}}/login\"\n",
        );
        let use_with = |policy: &str| {
            format!(
                "kind = \"http\"\nurl = \"{{{{base_url}}}}/use?t={{{{ $response('Auth/login', '$.target'{policy}) }}}}\"\n"
            )
        };
        write(dir.path(), "use.toml", &use_with(""));
        write(dir.path(), "always.toml", &use_with(", 'always'"));
        write(dir.path(), "never.toml", &use_with(", 'never'"));
        write(
            dir.path(),
            "nomatch.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/x?t={{ $response('Auth/login', '$.nope') }}\"\n",
        );
        write(
            dir.path(),
            "Loop/a.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/a?t={{ $response('Loop/b', '$.target') }}\"\n",
        );
        write(
            dir.path(),
            "Loop/b.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/b?t={{ $response('Loop/a', '$.target') }}\"\n",
        );
        write(
            dir.path(),
            "ws.toml",
            "kind = \"websocket\"\nurl = \"ws://127.0.0.1:9/\"\n",
        );
        write(
            dir.path(),
            "wsdep.toml",
            "kind = \"http\"\nurl = \"{{base_url}}/w?t={{ $response('ws', '$.x') }}\"\n",
        );
        let run = |args: &[&str]| {
            let out = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(dir.path())
                .args(args)
                .env("SENDITO_KEYCHAIN", "memory")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            (out.status.code(), stderr)
        };
        let targets = || -> Vec<String> { echo.take().iter().map(|c| c.target()).collect() };

        // `never` before any response: nothing is sent.
        let (code, stderr) = run(&["run", "never", "--env", "local"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("Auth/login has no response yet"), "{stderr}");
        assert!(targets().is_empty());

        // The dependency goes first; the value is a secret in the history.
        let (code, stderr) = run(&["run", "use", "--env", "local"]);
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stderr.contains("dependency Auth/login: sent (200 OK"), "{stderr}");
        assert_eq!(targets(), ["/login", "/use?t=/login"]);
        let history = std::fs::read_to_string(dir.path().join(".sendito/history.jsonl")).unwrap();
        let used = history.lines().last().unwrap();
        assert!(!used.contains("t=/login"), "{used}");

        // A new process reads the history; `always` sends again.
        let (code, stderr) = run(&["run", "use", "--env", "local"]);
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stderr.contains("dependency Auth/login: the response of the history entry"), "{stderr}");
        assert_eq!(targets(), ["/use?t=/login"]);
        let (code, _) = run(&["run", "always", "--env", "local"]);
        assert_eq!(code, Some(0));
        assert_eq!(targets(), ["/login", "/use?t=/login"]);

        // Another environment has no response yet: the dependency is sent.
        std::fs::write(
            dir.path().join(".sendito/environments/other.toml"),
            format!("[variables]\nbase_url = \"{}\"\n", echo.url()),
        )
        .unwrap();
        let (code, _) = run(&["run", "use", "--env", "other", "--no-history"]);
        assert_eq!(code, Some(0));
        assert_eq!(targets(), ["/login", "/use?t=/login"]);

        for (req, reason) in [
            ("nomatch", "$.nope has no match in the response of Auth/login (200 OK)"),
            ("Loop/a", "$response cycle: Loop/b.toml → Loop/a.toml → Loop/b.toml"),
            ("wsdep", "ws is not an HTTP request"),
        ] {
            let (code, stderr) = run(&["run", req, "--env", "local", "--no-history"]);
            assert_eq!(code, Some(2), "{req}: {stderr}");
            assert!(stderr.contains(reason), "{req}: {stderr}");
        }
    });
}

/// `--data`: the folder runs once per row, each row from a clean runtime;
/// the report names each row; `--repeat` is refused; a single request runs
/// once per row too.
#[test]
fn cli_data_runs_the_folder_once_per_row() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    rt.block_on(async {
        let echo = sendito_testkit::http_echo::start().await.unwrap();
        sendito_store::init::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("F")).unwrap();
        let url = echo.url();
        std::fs::write(
            dir.path().join("F/1-a.toml"),
            format!("kind = \"http\"\nurl = \"{url}/{{{{user}}}}\"\n\n[[scripts.extract]]\nvar = \"got\"\nfrom = \"jsonpath\"\nexpr = \"$.target\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("F/2-b.toml"),
            format!("kind = \"http\"\nurl = \"{url}/b{{{{got}}}}\"\n"),
        )
        .unwrap();
        std::fs::write(dir.path().join("users.csv"), "user\nana\nbob\n").unwrap();
        std::fs::write(dir.path().join("users.json"), r#"[{"user": "cy"}]"#).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(dir.path())
                .args(args)
                .env("SENDITO_KEYCHAIN", "memory")
                .output()
                .unwrap()
        };
        let targets = || echo.take().iter().map(|c| c.target()).collect::<Vec<_>>();

        let out = run(&["run", "F", "--no-history", "--data", "users.csv", "--report", "junit"]);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{stderr}");
        assert!(stderr.contains("once per data row (2)"), "{stderr}");
        assert!(stderr.contains("PASS  F/1-a.toml[1]"), "{stderr}");
        assert!(stderr.contains("PASS  F/2-b.toml[2]"), "{stderr}");
        assert_eq!(targets(), ["/ana", "/b/ana", "/bob", "/b/bob"]);
        let junit = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(junit.contains("tests=\"4\""), "{junit}");
        assert!(junit.contains("2-b.toml[2]"), "{junit}");

        // A single request, from a JSON file.
        let out = run(&["run", "F/1-a", "--no-history", "--data", "users.json"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(targets(), ["/cy"]);

        // The second request alone: `got` comes from no row.
        let out = run(&["run", "F/2-b", "--no-history", "--data", "users.json"]);
        assert_eq!(out.status.code(), Some(3));
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(stderr.contains("ERROR F/2-b.toml[1]  unresolved variables: {{got}}"), "{stderr}");

        let out = run(&["run", "F", "--data", "users.csv", "--repeat", "2"]);
        assert_eq!(out.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&out.stderr).contains("--data and --repeat"));
        let out = run(&["run", "F", "--data", "missing.csv"]);
        assert_eq!(out.status.code(), Some(3));
        assert!(targets().is_empty());
    });
}

/// A GraphQL subscription over WebSocket in `sendito run`: the session ends
/// when the server completes it, and the scripts read the last payload.
#[test]
fn cli_ws_graphql_subscription_runs_to_complete() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let ws = rt.block_on(sendito_testkit::ws_echo::start()).unwrap();
    sendito_store::init::init(root).unwrap();
    for (file, protocol) in [("a.toml", ""), ("b.toml", "protocol = \"graphql-ws\"\n")] {
        write(
            root,
            &format!("F/{file}"),
            &format!(
                "kind = \"websocket\"\nurl = \"{}\"\n\n[graphql]\nquery = \"subscription {{ tick }}\"\n{protocol}\n[[scripts.assert]]\nfrom = \"jsonpath\"\nexpr = \"$.data.tick\"\nop = \"eq\"\nvalue = \"2\"\n",
                ws.url("/graphql")
            ),
        );
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
        .current_dir(root)
        .args(["run", "F", "--no-history"])
        .env("SENDITO_KEYCHAIN", "memory")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("PASS  F/a.toml  (close 1000"), "{stderr}");
    assert!(stderr.contains("PASS  F/b.toml  (close 1000"), "{stderr}");
}
