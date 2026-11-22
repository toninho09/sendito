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
    std::fs::create_dir_all(p.parent().unwrap_or(root)).unwrap();
    std::fs::write(p, text).unwrap();
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
        ("Orders / Order receipt", "saved example responses"),
        ("Orders / Cancel order", "description"),
        ("Auth / OAuth token", "\"oauth2\" auth is not supported"),
        ("Auth / Digest login", "\"digest\" auth is not supported"),
        ("GraphQL query", "GraphQL body became a JSON body"),
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
    assert_eq!(r.skipped.len(), 22, "unexpected report entries:\n{report}");
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
    assert!(matches!(&ban.auth, sendito_core::Auth::Basic { username, .. } if username == "admin"));
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
    let p = sendito_ui::runner::Project::open_with(dir.path(), kc, None);
    let Request::Http(req) = p.coll.load("r.toml").unwrap_or_else(|e| panic!("{e}")) else {
        panic!()
    };
    let (scope, errors) =
        sendito_ui::runner::build_scope(&p.envs, Some("dev"), &Default::default());
    assert!(errors.is_empty(), "{errors:?}");
    let ctx = sendito_ui::runner::RunCtx {
        root: dir.path().to_path_buf(),
        send: p.send_options(),
        history: Some((dir.path().join(".sendito/history.jsonl"), 100)),
        name: "r".into(),
        path: Some("r.toml".into()),
        env: Some("dev".into()),
        schemas: Default::default(),
        grpc_deadline_ms: 0,
        mask_echoed_secrets: false,
        cookie_jar: false,
        oauth: None,
    };
    let o = rt.block_on(sendito_ui::runner::exec_http(
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
    let history = read(&dir.path().join(".sendito/history.jsonl"));
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
    let p = sendito_ui::runner::Project::open_with(
        dir.path(),
        sendito_ui::runner::default_keychain(),
        None,
    );
    let ctx = sendito_ui::runner::RunCtx {
        root: dir.path().to_path_buf(),
        send: p.send_options(),
        history: None,
        name: "test".into(),
        path: None,
        env: None,
        schemas: Default::default(),
        grpc_deadline_ms: 0,
        mask_echoed_secrets: false,
        cookie_jar: false,
        oauth: None,
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
    let result = rt.block_on(sendito_ui::runner::exec_http(
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
    let p =
        sendito_ui::runner::Project::open_with(root, sendito_ui::runner::default_keychain(), None);
    let ctx = |name: &str| sendito_ui::runner::RunCtx {
        root: root.to_path_buf(),
        send: p.send_options(),
        history: None,
        name: name.into(),
        path: Some(format!("{name}.toml")),
        env: Some("dev".into()),
        schemas: Default::default(),
        grpc_deadline_ms: 0,
        mask_echoed_secrets: false,
        cookie_jar: false,
        oauth: None,
    };
    let login = p.coll.load("login.toml").unwrap();
    let echo = p.coll.load("echo.toml").unwrap();
    let (Request::Http(login), Request::Http(echo)) = (login, echo) else {
        panic!("not http")
    };
    let (scope, errors) =
        sendito_ui::runner::build_scope(&p.envs, Some("dev"), &Default::default());
    assert!(errors.is_empty(), "{errors:?}");
    // 1. The login response sets two cookies.
    let o = rt.block_on(sendito_ui::runner::exec_http(
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
    let o = rt.block_on(sendito_ui::runner::exec_http(
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
        .scripts
        .extract
        .push(sendito_core::model::Extract {
            var: "cookie".into(),
            source: sendito_core::model::Source::Header,
            expr: "set-cookie".into(),
            secret: false,
        });
    let masked = rt.block_on(sendito_ui::runner::exec_http(
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
    // Do not forward cookies across a redirect even on the same host.
    let mut redirected = echo.clone();
    redirected.url = "{{base}}/redirect/1".into();
    let o = rt.block_on(sendito_ui::runner::exec_http(
        &ctx("redirect"),
        &redirected,
        scope.clone(),
        vec![],
    ));
    assert_eq!(o.response.unwrap().status, 302);
    assert!(
        o.resolved
            .notes
            .iter()
            .any(|n| n.contains("redirects are not followed"))
    );
    http.take();
    // 3. Without `cookies = true` nothing is sent or stored.
    let mut plain = echo.clone();
    plain.options.cookies = Some(false);
    let o = rt.block_on(sendito_ui::runner::exec_http(
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
        let mut p = sendito_ui::runner::Project::open_with(dir.path(), kc.clone(), None);
        p.config.oauth.insert(
            "service".into(),
            sendito_store::config::OAuthIdentity {
                authorize_url: settings.authorize_url.take().unwrap(),
                token_url: settings.token_url.take().unwrap(),
                client_id: "cli-1".into(),
                client_secret: Some("test-client-secret".into()),
                scopes: vec![],
            },
        );
        let account = sendito_ui::runner::oauth_token_account(&p.name, Some("dev"), "service");
        kc.set(&account, &tokens.to_json()).unwrap();
        let ctx = sendito_ui::runner::RunCtx {
            root: dir.path().to_path_buf(),
            send: p.send_options(),
            history: None,
            name: "oauth".into(),
            path: None,
            env: Some("dev".into()),
            schemas: Default::default(),
            grpc_deadline_ms: 0,
            mask_echoed_secrets: true,
            cookie_jar: false,
            oauth: Some(sendito_ui::runner::OAuthRunInfo::of(&p)),
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
        req.scripts.extract.push(sendito_core::model::Extract {
            var: "reflected_token".into(),
            source: sendito_core::model::Source::Body,
            expr: "$.token".into(),
            secret: false,
        });
        let o = sendito_ui::runner::exec_http(&ctx, &req, Scope::new(), vec![]).await;
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
        let o = sendito_ui::runner::exec_http(&ctx, &explicit, Scope::new(), vec![]).await;
        assert_eq!(o.response.unwrap().status, 401);
        assert_eq!(srv.http_requests(), 3);
        assert_eq!(srv.refreshes(), 1);
        // Different environments cannot use each other's stored tokens.
        let mut other = ctx;
        other.env = Some("other".into());
        let o = sendito_ui::runner::exec_http(&other, &req, Scope::new(), vec![]).await;
        assert!(o.response.is_err());
        assert_eq!(srv.http_requests(), 3);
    });
}
