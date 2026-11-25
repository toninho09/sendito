//! GraphQL schema loading through the HTTP authentication pipeline.

use super::*;
use sendito_http::graphql::{GraphqlSchema, INTROSPECTION_QUERY};

pub type GraphqlSchemaCache = Arc<Mutex<HashMap<String, GraphqlSchema>>>;

pub fn graphql_schema_key(request: &ResolvedHttp) -> String {
    format!("graphql:{} {}", request.method, request.url)
}

/// Fetches introspection without request scripts, history or response masking.
/// Authentication, cookies, redirects and transport options use the HTTP pipeline.
pub async fn fetch_schema(
    ctx: &RunCtx,
    request: &HttpRequest,
    scope: Scope,
    cache: &GraphqlSchemaCache,
    refresh: bool,
) -> Result<GraphqlSchema, String> {
    let original = resolve_http(request, &scope, &ctx.root);
    let key = graphql_schema_key(&original);
    if !refresh && let Some(schema) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(schema);
    }
    // A failed refresh must not leave an old schema available.
    if let Ok(mut cache) = cache.lock() {
        cache.remove(&key);
    }
    let mut request = request.clone();
    request.method = "POST".into();
    request.body = Body::GraphQL {
        query: INTROSPECTION_QUERY.into(),
        variables: String::new(),
        operation_name: "IntrospectionQuery".into(),
    };
    request.scripts = Scripts::default();
    if !resolve_http(&request, &scope, &ctx.root)
        .unresolved
        .is_empty()
    {
        return Err("GraphQL introspection has unresolved variables".into());
    }
    let mut ctx = ctx.clone();
    ctx.history = None;
    ctx.mask_echoed_secrets = false;
    let outcome = exec_http(&ctx, &request, scope, Vec::new()).await;
    let response = outcome.response?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "GraphQL introspection returned HTTP {}",
            response.status
        ));
    }
    if response.truncated {
        return Err("GraphQL introspection response is truncated".into());
    }
    let schema = GraphqlSchema::from_json(outcome.doc.bytes())?;
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, schema.clone());
    }
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    const SCHEMA: &str = r#"{"data":{"__schema":{"queryType":{"name":"Query"},"types":[{"name":"Query","kind":"OBJECT","fields":[]}]}}}"#;

    fn context() -> RunCtx {
        RunCtx {
            root: std::env::temp_dir(),
            send: SendOptions::default(),
            history: None,
            name: String::new(),
            path: None,
            env: None,
            schemas: SchemaCache::default(),
            grpc_deadline_ms: 0,
            ws_timeout_ms: 0,
            sse_timeout_ms: 30_000,
            mqtt_timeout_ms: 30_000,
            mask_echoed_secrets: true,
            cookie_jar: false,
            oauth: None,
        }
    }

    fn server(
        responses: Vec<(u16, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (url, handle)
    }

    #[tokio::test]
    async fn graphql_fetch_cache_refresh_and_auth() {
        let (url, server) = server(vec![(200, SCHEMA), (200, SCHEMA)]);
        let request = HttpRequest {
            url,
            method: "GET".into(),
            auth: Auth::Bearer {
                token: "{{token}}".into(),
            },
            body: Body::GraphQL {
                query: "draft".into(),
                variables: "invalid draft".into(),
                operation_name: String::new(),
            },
            ..Default::default()
        };
        let mut scope = Scope::new();
        scope.set("test", "token", VarValue::secret("test-token"));
        let cache = GraphqlSchemaCache::default();
        let ctx = context();
        for refresh in [false, false, true] {
            let schema = fetch_schema(&ctx, &request, scope.clone(), &cache, refresh)
                .await
                .unwrap();
            assert_eq!(schema.query_type, "Query");
        }
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(request.method, "GET");
        for wire in requests {
            assert!(wire.starts_with("POST /graphql HTTP/1.1"));
            assert!(wire.contains("Authorization: Bearer test-token"));
            let body: serde_json::Value =
                serde_json::from_str(wire.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["query"], INTROSPECTION_QUERY);
            assert_eq!(body["operationName"], "IntrospectionQuery");
            assert!(body.get("variables").is_none());
        }
        assert!(
            cache
                .lock()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .starts_with("graphql:GET ")
        );
    }

    #[tokio::test]
    async fn graphql_failed_refresh_removes_old_schema() {
        let (url, server) = server(vec![(200, SCHEMA), (403, "secret-response")]);
        let request = HttpRequest {
            url,
            ..Default::default()
        };
        let cache = GraphqlSchemaCache::default();
        let ctx = context();
        fetch_schema(&ctx, &request, Scope::new(), &cache, false)
            .await
            .unwrap();
        let error = fetch_schema(&ctx, &request, Scope::new(), &cache, true)
            .await
            .unwrap_err();
        assert_eq!(error, "GraphQL introspection returned HTTP 403");
        assert!(cache.lock().unwrap().is_empty());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn graphql_unresolved_introspection_does_not_connect() {
        let request = HttpRequest {
            url: "http://127.0.0.1:1/{{missing}}".into(),
            ..Default::default()
        };
        let error = fetch_schema(
            &context(),
            &request,
            Scope::new(),
            &GraphqlSchemaCache::default(),
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(error, "GraphQL introspection has unresolved variables");
    }
}
