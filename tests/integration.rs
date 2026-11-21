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
