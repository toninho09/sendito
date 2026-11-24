//! gRPC schema loading and the method picker.

use super::*;

impl App {
    // ----- gRPC schema --------------------------------------------------

    pub fn load_schema(&mut self, open_picker: bool, template: bool, refresh: bool) {
        let Some(t) = self.tab_mut() else { return };
        if t.ed.kind != Kind::Grpc {
            return;
        }
        t.refresh_model();
        let Request::Grpc(g) = t.model.clone() else {
            return;
        };
        let id = t.id;
        let resolved = sendito_core::resolve::resolve_grpc(&g, &self.names);
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
