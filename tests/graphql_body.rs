//! GraphQL body validation through the CLI.

#[test]
fn graphql_exports_keep_secret_placeholders() {
    use sendito_core::{Body, HttpRequest, Scope, VarValue};
    let request = HttpRequest {
        method: "POST".into(),
        url: "https://example.test/graphql".into(),
        body: Body::GraphQL {
            query: "query Lookup { hero { name } }".into(),
            variables: "{\"token\":\"{{token}}\"}".into(),
            operation_name: "Lookup".into(),
        },
        ..Default::default()
    };
    let mut scope = Scope::new();
    scope.set("test", "token", VarValue::secret("secret-value"));
    let base = std::path::Path::new(".");
    let curl = sendito_import::curl::export(&request, &scope, false, base);
    let http = sendito_import::http_file::export("Lookup", &request, &scope, false, base);
    for export in [curl, http] {
        assert!(export.contains("operationName"), "{export}");
        assert!(export.contains("{{token}}"), "{export}");
        assert!(!export.contains("secret-value"), "{export}");
    }
}

#[test]
fn graphql_variables_not_object_exits_three_without_connecting() {
    let dir = tempfile::tempdir().unwrap();
    sendito_store::init::init(dir.path()).unwrap();
    let file = dir.path().join("invalid.toml");
    std::fs::write(&file, "kind = \"http\"\nmethod = \"POST\"\nurl = \"http://127.0.0.1:1/graphql\"\n[body]\ntype = \"graphql\"\nquery = \"{ hero { name } }\"\nvariables = \"[]\"\n").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
        .args(["run", "--no-history"])
        .arg(&file)
        .env("SENDITO_KEYCHAIN", "memory")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("variables must be a JSON object"), "{error}");
    assert!(!error.contains("connection refused"), "{error}");
}
