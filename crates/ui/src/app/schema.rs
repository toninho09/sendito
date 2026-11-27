//! gRPC schema loading and the method picker.

use super::*;

impl App {
    pub(super) fn graphql_source(&self, tab: &Tab) -> String {
        let (request, scope) = self.effective(tab);
        let request = match request {
            Request::Http(h) => h,
            Request::Ws(w) if w.graphql.is_some() => runner::ws_graphql_http(&w, &scope),
            _ => return String::new(),
        };
        let resolved = sendito_core::resolve::resolve_http(&request, &scope, &self.project.root);
        format!(
            "{:?}:{}",
            self.active_env,
            runner::graphql_schema_key(&resolved)
        )
    }

    pub fn load_graphql_schema(&mut self, refresh: bool) {
        let Some(tab) = self.tab_mut() else { return };
        tab.refresh_model();
        if !matches!(tab.ed.kind, Kind::Http | Kind::Ws) || tab.ed.body_kind != BodyKind::GraphQL {
            self.set_status(
                "GraphQL schema requires a GraphQL body or a GraphQL WebSocket session",
                StatusKind::Info,
            );
            return;
        }
        let id = tab.id;
        let request = tab.model.clone();
        let path = tab.path.clone();
        let name = tab.name.clone();
        let ctx = self.run_ctx(name, path.clone());
        let source = self
            .tab()
            .map(|t| self.graphql_source(t))
            .unwrap_or_default();
        if let Some(t) = self.tab_mut() {
            t.graphql_source = source.clone();
            t.graphql_schema = None;
            t.schema_error = None;
        }
        let envs = self.project.envs.clone();
        let active = self.active_env.clone();
        let runtime = self.runtime.clone();
        let root = self.project.root.clone();
        let cache = self.graphql_schemas.clone();
        let tx = self.tx.clone();
        if let Some((_, task)) = self.schema_job.take() {
            task.abort();
        }
        self.schema_seq = self.schema_seq.wrapping_add(1);
        let job = self.schema_seq;
        let task = self.rt.spawn(async move {
            let result = async {
                let (request, scope) = tokio::task::spawn_blocking(move || {
                    let (mut scope, _, failed) =
                        runner::build_scope_checked(&envs, active.as_deref(), &runtime);
                    let chain = runner::folder_chain(&root, path.as_deref())?;
                    let request = match runner::with_folders(&request, &chain, &mut scope) {
                        // A GraphQL WebSocket session asks the HTTP equivalent of its URL.
                        Request::Ws(w) => Request::Http(runner::ws_graphql_http(&w, &scope)),
                        r => r,
                    };
                    let mut check = request.clone();
                    if let Request::Http(http) = &mut check {
                        http.body = Body::None;
                        http.common.scripts = Scripts::default();
                    }
                    if !runner::unresolved_loaded(&check, &scope, &failed).is_empty() {
                        return Err("GraphQL introspection has unresolved variables".to_string());
                    }
                    Ok((request, scope))
                })
                .await
                .map_err(|_| "GraphQL scope task failed".to_string())??;
                let Request::Http(request) = request else {
                    return Err("GraphQL introspection requires HTTP".into());
                };
                runner::fetch_schema(&ctx, &request, scope, &cache, refresh).await
            }
            .await;
            let _ = tx.send(AppMsg::GraphqlSchema {
                tab: id,
                job,
                source,
                result,
            });
        });
        self.schema_job = Some((job, task));
        self.set_status("loading GraphQL schema", StatusKind::Info);
    }

    // ----- gRPC schema --------------------------------------------------

    pub fn load_schema(&mut self, open_picker: bool, template: bool, refresh: bool) {
        let Some(t) = self.tab_mut() else { return };
        if t.ed.kind != Kind::Grpc {
            return;
        }
        t.refresh_model();
        let id = t.id;
        // Reflection goes out with the metadata and auth the folders add.
        let (req, names) = match self.tab() {
            Some(t) => self.effective(t),
            None => return,
        };
        let Request::Grpc(g) = req else {
            return;
        };
        let resolved = sendito_core::resolve::resolve_grpc(&g, &names);
        let root = self.project.root.clone();
        let tls = self.project.tls.clone();
        let cache = self.schemas.clone();
        let tx = self.tx.clone();
        let svc = g.service.clone();
        let method = g.method.clone();
        if open_picker || self.complete.is_none() {
            self.set_status(
                format!(
                    "loading schema from {}…",
                    if matches!(g.schema, SchemaSource::Reflection) {
                        resolved.uri.clone()
                    } else {
                        "proto files".into()
                    }
                ),
                StatusKind::Info,
            );
        }
        if let Some((_, task)) = self.schema_job.take() {
            task.abort();
        }
        self.schema_seq = self.schema_seq.wrapping_add(1);
        let job = self.schema_seq;
        let task = self.rt.spawn(async move {
            let r = runner::load_schema(&root, &resolved, &g.schema, &cache, refresh, &tls).await;
            let template = match (&r, template) {
                (Ok(s), true) => s.template(&svc, &method).ok(),
                _ => None,
            };
            let _ = tx.send(AppMsg::Schema {
                tab: id,
                job,
                result: r.map(|s| s.services()),
                open_picker,
                template,
            });
        });
        self.schema_job = Some((job, task));
    }

    pub(super) fn open_grpc_method_picker(&mut self) {
        let Some(t) = self.tab() else { return };
        let Some(svcs) = t.services.clone() else {
            self.load_schema(true, false, false);
            return;
        };
        let current = format!("{}/{}", t.ed.service(), t.ed.method());
        let mut items = Vec::new();
        let mut details = Vec::new();
        for s in &svcs {
            for m in &s.methods {
                items.push(format!("{}/{}", s.name, m.name));
                details.push(format!("{} · {} → {}", m.kind.label(), m.input, m.output));
            }
        }
        if items.is_empty() {
            self.set_status("the schema has no services", StatusKind::Warn);
            return;
        }
        let cur = items.iter().position(|i| *i == current);
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Choose a gRPC method",
            items,
            details,
            PickAction::GrpcMethod,
            cur,
        )));
    }
}
