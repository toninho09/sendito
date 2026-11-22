//! Schemas: from `.proto` files (compiled in-process by protox) or from server
//! reflection. Also generates JSON templates for request messages.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor,
    SerializeOptions,
};
use prost_types::FileDescriptorProto;
use sendito_core::complete::FieldNode;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

use crate::GrpcError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Unary,
    ServerStreaming,
    ClientStreaming,
    Bidi,
}

impl CallKind {
    pub fn of(m: &MethodDescriptor) -> Self {
        match (m.is_client_streaming(), m.is_server_streaming()) {
            (false, false) => CallKind::Unary,
            (false, true) => CallKind::ServerStreaming,
            (true, false) => CallKind::ClientStreaming,
            (true, true) => CallKind::Bidi,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CallKind::Unary => "unary",
            CallKind::ServerStreaming => "server stream",
            CallKind::ClientStreaming => "client stream",
            CallKind::Bidi => "bidi stream",
        }
    }

    pub fn client_streams(self) -> bool {
        matches!(self, CallKind::ClientStreaming | CallKind::Bidi)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodInfo {
    pub name: String,
    pub kind: CallKind,
    pub input: String,
    pub output: String,
    /// The fields of the input message, for completion in the message editor.
    pub input_fields: FieldNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

/// A loaded set of descriptors.
#[derive(Debug, Clone)]
pub struct Schema {
    pub pool: DescriptorPool,
}

fn resolve(base: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() { pb } else { base.join(pb) }
}

impl Schema {
    pub fn from_pool(pool: DescriptorPool) -> Self {
        Self { pool }
    }

    /// Compiles `.proto` files. Relative paths are anchored at `base`. When no
    /// import path is given, each file's directory is used.
    pub fn from_files(
        files: &[String],
        import_paths: &[String],
        base: &Path,
    ) -> Result<Self, GrpcError> {
        if files.is_empty() {
            return Err(GrpcError::Schema("no .proto files configured".into()));
        }
        let files: Vec<PathBuf> = files.iter().map(|f| resolve(base, f)).collect();
        let mut includes: Vec<PathBuf> = import_paths.iter().map(|p| resolve(base, p)).collect();
        if includes.is_empty() {
            for f in &files {
                if let Some(d) = f.parent()
                    && !includes.iter().any(|i| i == d)
                {
                    includes.push(d.to_path_buf());
                }
            }
        }
        let fds =
            protox::compile(&files, &includes).map_err(|e| GrpcError::Schema(e.to_string()))?;
        let pool = DescriptorPool::from_file_descriptor_set(fds)
            .map_err(|e| GrpcError::Schema(e.to_string()))?;
        Ok(Self { pool })
    }

    /// Lists services and methods, sorted by name. Reflection services are hidden.
    pub fn services(&self) -> Vec<ServiceInfo> {
        let mut v: Vec<ServiceInfo> = self
            .pool
            .services()
            .filter(|s| !s.full_name().starts_with("grpc.reflection."))
            .map(|s| ServiceInfo {
                name: s.full_name().to_string(),
                methods: s
                    .methods()
                    .map(|m| MethodInfo {
                        name: m.name().to_string(),
                        kind: CallKind::of(&m),
                        input: m.input().full_name().to_string(),
                        output: m.output().full_name().to_string(),
                        input_fields: field_tree(&m.input()),
                    })
                    .collect(),
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn method(&self, service: &str, method: &str) -> Result<MethodDescriptor, GrpcError> {
        let svc = self
            .pool
            .get_service_by_name(service)
            .ok_or_else(|| GrpcError::Schema(format!("service {service:?} not found in schema")))?;
        svc.methods()
            .find(|m| m.name() == method)
            .ok_or_else(|| GrpcError::Schema(format!("method {method:?} not found in {service}")))
    }

    /// JSON template for a method's input message.
    pub fn template(&self, service: &str, method: &str) -> Result<String, GrpcError> {
        let m = self.method(service, method)?;
        Ok(template_for(&m.input()))
    }
}

/// Generates a JSON object with every field set to a sample default value.
pub fn template_for(desc: &MessageDescriptor) -> String {
    let mut seen = HashSet::new();
    let v = message_template(desc, &mut seen, 0);
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
}

/// The fields of a message, for completion in the message editor. Unlike the
/// template it keeps every `oneof` member (the editor decides which are still
/// available) and stops descending into types that are already open.
pub fn field_tree(desc: &MessageDescriptor) -> FieldNode {
    let mut seen = HashSet::new();
    FieldNode {
        name: desc.full_name().to_string(),
        detail: String::new(),
        map: false,
        example: "{}".into(),
        children: message_fields(desc, &mut seen, 0),
        oneof: None,
    }
}

fn message_fields(
    desc: &MessageDescriptor,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Vec<FieldNode> {
    if depth > 6 || seen.contains(desc.full_name()) {
        return Vec::new();
    }
    seen.insert(desc.full_name().to_string());
    let out: Vec<FieldNode> = desc
        .fields()
        .map(|f| {
            let kind = f.kind();
            let (detail, example) = match (&kind, f.is_map(), f.is_list()) {
                (_, true, _) => (map_detail(&f), "{}".to_string()),
                (_, false, true) => (format!("repeated {}", kind_detail(&kind)), "[]".to_string()),
                _ => (kind_detail(&kind), value_example(&kind)),
            };
            let children = if f.is_map() {
                // The value shape, not the synthetic map-entry message.
                match f.kind() {
                    Kind::Message(entry) => match entry.map_entry_value_field().kind() {
                        Kind::Message(m) if !is_wkt(m.full_name()) => {
                            message_fields(&m, seen, depth + 1)
                        }
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                }
            } else {
                match kind {
                    Kind::Message(m) if !is_wkt(m.full_name()) => {
                        message_fields(&m, seen, depth + 1)
                    }
                    Kind::Message(m) => wkt_fields(m.full_name()),
                    Kind::Enum(e) => e
                        .values()
                        .map(|v| FieldNode::new(v.name(), format!("enum {}", e.name()), "\"\""))
                        .collect(),
                    _ => Vec::new(),
                }
            };
            FieldNode {
                name: f.name().to_string(),
                detail,
                map: f.is_map(),
                example,
                children,
                // A proto3 `optional` field gets a synthetic oneof, which is not a
                // real choice; only keep the name for a declared group.
                oneof: f
                    .containing_oneof()
                    .filter(|o| !o.is_synthetic())
                    .map(|o| o.name().to_string()),
            }
        })
        .collect();
    seen.remove(desc.full_name());
    out
}

fn is_wkt(full: &str) -> bool {
    wkt_template(full).is_some()
}

fn wkt_fields(full: &str) -> Vec<FieldNode> {
    match full {
        "google.protobuf.Struct" => vec![FieldNode::new(
            "fields",
            "map<string, google.protobuf.Value>",
            "{}",
        )],
        "google.protobuf.Struct.FieldsEntry" => Vec::new(),
        _ => Vec::new(),
    }
}

/// JSON type of a well-known type, which differs from its proto shape.
fn wkt_detail(full: &str) -> Option<&'static str> {
    Some(match full {
        "google.protobuf.Timestamp" => "string (RFC 3339)",
        "google.protobuf.Duration" => "string (e.g. 1.5s)",
        "google.protobuf.FieldMask" => "string",
        "google.protobuf.StringValue" | "google.protobuf.BytesValue" => "string",
        "google.protobuf.BoolValue" => "boolean",
        "google.protobuf.Int64Value" | "google.protobuf.UInt64Value" => "string (integer)",
        "google.protobuf.Int32Value"
        | "google.protobuf.UInt32Value"
        | "google.protobuf.FloatValue"
        | "google.protobuf.DoubleValue" => "number",
        "google.protobuf.Struct" => "object",
        "google.protobuf.Value" => "any",
        "google.protobuf.ListValue" => "array",
        "google.protobuf.Empty" => "object",
        "google.protobuf.Any" => "object",
        _ => return None,
    })
}

/// A minimal snippet that completes `"name": ` with a valid value.
fn value_example(kind: &Kind) -> String {
    match kind {
        Kind::Bool => "false".into(),
        Kind::String | Kind::Bytes => "\"\"".into(),
        Kind::Enum(e) => e
            .values()
            .next()
            .map(|v| format!("\"{}\"", v.name()))
            .unwrap_or_else(|| "0".into()),
        Kind::Message(m) => match m.full_name() {
            "google.protobuf.Timestamp"
            | "google.protobuf.Duration"
            | "google.protobuf.FieldMask"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue" => "\"\"".into(),
            "google.protobuf.Int64Value" | "google.protobuf.UInt64Value" => "\"0\"".into(),
            "google.protobuf.BoolValue" => "false".into(),
            "google.protobuf.Value" => "null".into(),
            "google.protobuf.ListValue" => "[]".into(),
            "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.FloatValue"
            | "google.protobuf.DoubleValue" => "0".into(),
            _ => "{}".into(),
        },
        _ => "0".into(),
    }
}

fn kind_detail(kind: &Kind) -> String {
    match kind {
        Kind::Message(m) => wkt_detail(m.full_name())
            .map(str::to_string)
            .unwrap_or_else(|| m.full_name().to_string()),
        Kind::Enum(e) => format!("enum {}", e.full_name()),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn map_detail(f: &FieldDescriptor) -> String {
    let Kind::Message(entry) = f.kind() else {
        return "map".to_string();
    };
    let key_f = entry.map_entry_key_field();
    let val_f = entry.map_entry_value_field();
    format!(
        "map<{}, {}>",
        kind_detail(&key_f.kind()),
        kind_detail(&val_f.kind())
    )
}

fn wkt_template(full: &str) -> Option<serde_json::Value> {
    use serde_json::Value;
    Some(match full {
        "google.protobuf.Timestamp" => Value::String("1970-01-01T00:00:00Z".into()),
        "google.protobuf.Duration" => Value::String("0s".into()),
        "google.protobuf.Struct" => Value::Object(Default::default()),
        "google.protobuf.Value" => Value::Null,
        "google.protobuf.ListValue" => Value::Array(vec![]),
        "google.protobuf.Empty" => Value::Object(Default::default()),
        "google.protobuf.FieldMask" => Value::String(String::new()),
        "google.protobuf.StringValue" | "google.protobuf.BytesValue" => {
            Value::String(String::new())
        }
        "google.protobuf.BoolValue" => Value::Bool(false),
        "google.protobuf.Int64Value" | "google.protobuf.UInt64Value" => Value::String("0".into()),
        "google.protobuf.Int32Value"
        | "google.protobuf.UInt32Value"
        | "google.protobuf.FloatValue"
        | "google.protobuf.DoubleValue" => Value::from(0),
        _ => return None,
    })
}

fn scalar_template(
    f: &FieldDescriptor,
    kind: &Kind,
    seen: &mut HashSet<String>,
    depth: usize,
) -> serde_json::Value {
    use serde_json::Value;
    match kind {
        Kind::Double
        | Kind::Float
        | Kind::Int32
        | Kind::Sint32
        | Kind::Sfixed32
        | Kind::Uint32
        | Kind::Fixed32 => Value::from(0),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 | Kind::Uint64 | Kind::Fixed64 => {
            Value::String("0".into())
        }
        Kind::Bool => Value::Bool(false),
        Kind::String | Kind::Bytes => Value::String(String::new()),
        Kind::Enum(e) => e
            .values()
            .next()
            .map(|v| Value::String(v.name().to_string()))
            .unwrap_or(Value::from(0)),
        Kind::Message(m) => {
            let _ = f;
            message_template(m, seen, depth + 1)
        }
    }
}

fn message_template(
    desc: &MessageDescriptor,
    seen: &mut HashSet<String>,
    depth: usize,
) -> serde_json::Value {
    use serde_json::Value;
    if let Some(v) = wkt_template(desc.full_name()) {
        return v;
    }
    if desc.full_name() == "google.protobuf.Any" {
        // Any needs a concrete type; Empty keeps the sample valid and minimal.
        let mut m = serde_json::Map::new();
        if desc
            .parent_pool()
            .get_message_by_name("google.protobuf.Empty")
            .is_some()
        {
            m.insert(
                "@type".into(),
                Value::String("type.googleapis.com/google.protobuf.Empty".into()),
            );
            m.insert("value".into(), Value::Object(Default::default()));
        } else {
            m.insert(
                "@type".into(),
                Value::String(format!("type.googleapis.com/{}", "google.protobuf.Any")),
            );
        }
        return Value::Object(m);
    }
    if depth > 4 || seen.contains(desc.full_name()) {
        return Value::Object(Default::default());
    }
    seen.insert(desc.full_name().to_string());
    let mut obj = serde_json::Map::new();
    let mut used_oneofs = HashSet::new();
    for f in desc.fields() {
        if let Some(o) = f.containing_oneof()
            && !o.is_synthetic()
            && !used_oneofs.insert(o.name().to_string())
        {
            continue;
        }
        let v = if f.is_map() {
            let Kind::Message(entry) = f.kind() else {
                continue;
            };
            let key_f = entry.map_entry_key_field();
            let val_f = entry.map_entry_value_field();
            let key = match key_f.kind() {
                Kind::String => "key".to_string(),
                Kind::Bool => "false".to_string(),
                _ => "0".to_string(),
            };
            let mut m = serde_json::Map::new();
            m.insert(key, scalar_template(&val_f, &val_f.kind(), seen, depth));
            Value::Object(m)
        } else if f.is_list() {
            Value::Array(vec![scalar_template(&f, &f.kind(), seen, depth)])
        } else {
            scalar_template(&f, &f.kind(), seen, depth)
        };
        obj.insert(f.name().to_string(), v);
    }
    seen.remove(desc.full_name());
    Value::Object(obj)
}

/// Options used when rendering messages as JSON: proto field names, defaults included.
pub fn display_options() -> SerializeOptions {
    SerializeOptions::new()
        .use_proto_field_name(true)
        .skip_default_fields(false)
        .stringify_64_bit_integers(true)
}

/// Options for showing a received message as it was on the wire: proto field
/// names, default values omitted (standard proto3 JSON).
pub fn wire_options() -> SerializeOptions {
    SerializeOptions::new()
        .use_proto_field_name(true)
        .skip_default_fields(true)
        .stringify_64_bit_integers(true)
}

/// Pretty JSON for a message.
pub fn to_json(msg: &DynamicMessage, opts: &SerializeOptions) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut buf,
        serde_json::ser::PrettyFormatter::with_indent(b"  "),
    );
    match msg.serialize_with_options(&mut ser, opts) {
        Ok(()) => String::from_utf8(buf).unwrap_or_default(),
        Err(e) => format!("{{\"error\": \"cannot render message: {e}\"}}"),
    }
}

/// One-line JSON without default values (proto field names).
pub fn to_compact(msg: &DynamicMessage) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::new(&mut buf);
    let opts = SerializeOptions::new()
        .use_proto_field_name(true)
        .stringify_64_bit_integers(true);
    match msg.serialize_with_options(&mut ser, &opts) {
        Ok(()) => String::from_utf8(buf).unwrap_or_default(),
        Err(e) => format!("{{\"error\": \"cannot render message: {e}\"}}"),
    }
}

/// Parses request JSON. With `multi`, a top-level array yields one message per element.
pub fn parse_messages(
    desc: &MessageDescriptor,
    json: &str,
    multi: bool,
) -> Result<Vec<DynamicMessage>, String> {
    let trimmed = json.trim();
    let text = if trimmed.is_empty() { "{}" } else { trimmed };
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("message is not valid JSON: {e}"))?;
    let items = match value {
        serde_json::Value::Array(items) if multi => items,
        serde_json::Value::Array(_) => {
            return Err("this method takes a single message; arrays are only allowed for client-streaming and bidi calls".into());
        }
        v => vec![v],
    };
    items
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            DynamicMessage::deserialize(desc.clone(), v).map_err(|e| {
                if multi {
                    format!("message {}: {e}", i + 1)
                } else {
                    e.to_string()
                }
            })
        })
        .collect()
}

macro_rules! reflect_impl {
    ($name:ident, $pb:path) => {
        async fn $name(channel: Channel) -> Result<Vec<FileDescriptorProto>, tonic::Status> {
            use pb::ServerReflectionRequest;
            use pb::server_reflection_client::ServerReflectionClient;
            use pb::server_reflection_request::MessageRequest;
            use pb::server_reflection_response::MessageResponse;
            use $pb as pb;

            let mut client = ServerReflectionClient::new(channel);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ServerReflectionRequest>();
            let req = |m: MessageRequest| ServerReflectionRequest {
                host: String::new(),
                message_request: Some(m),
            };
            tx.send(req(MessageRequest::ListServices(String::new())))
                .map_err(|e| tonic::Status::internal(e.to_string()))?;
            let mut stream = client
                .server_reflection_info(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
                .await?
                .into_inner();

            let mut files: BTreeMap<String, FileDescriptorProto> = BTreeMap::new();
            let mut requested: HashSet<String> = HashSet::new();
            let mut pending = 1usize;
            while pending > 0 {
                let Some(resp) = stream.next().await else {
                    break;
                };
                let resp = resp?;
                pending -= 1;
                match resp.message_response {
                    Some(MessageResponse::ListServicesResponse(l)) => {
                        for s in l.service {
                            if s.name.starts_with("grpc.reflection.") {
                                continue;
                            }
                            if requested.insert(format!("sym:{}", s.name)) {
                                pending += 1;
                                tx.send(req(MessageRequest::FileContainingSymbol(s.name)))
                                    .map_err(|e| tonic::Status::internal(e.to_string()))?;
                            }
                        }
                    }
                    Some(MessageResponse::FileDescriptorResponse(f)) => {
                        for raw in f.file_descriptor_proto {
                            let fd = FileDescriptorProto::decode(raw.as_slice()).map_err(|e| {
                                tonic::Status::internal(format!("bad descriptor: {e}"))
                            })?;
                            let name = fd.name().to_string();
                            for dep in &fd.dependency {
                                if !files.contains_key(dep)
                                    && requested.insert(format!("file:{dep}"))
                                {
                                    pending += 1;
                                    tx.send(req(MessageRequest::FileByFilename(dep.clone())))
                                        .map_err(|e| tonic::Status::internal(e.to_string()))?;
                                }
                            }
                            files.insert(name, fd);
                        }
                    }
                    Some(MessageResponse::ErrorResponse(e)) => {
                        // A missing well-known import is filled in locally below.
                        if !e.error_message.contains("google/protobuf/") {
                            return Err(tonic::Status::new(
                                tonic::Code::from_i32(e.error_code),
                                e.error_message,
                            ));
                        }
                    }
                    _ => {}
                }
            }
            drop(tx);
            Ok(files.into_values().collect())
        }
    };
}

reflect_impl!(reflect_v1, tonic_reflection::pb::v1);
reflect_impl!(reflect_v1alpha, tonic_reflection::pb::v1alpha);

/// Well-known type files compiled from protox's bundled sources, used when a
/// reflection server does not return them.
fn well_known(name: &str) -> Option<FileDescriptorProto> {
    if !name.starts_with("google/protobuf/") {
        return None;
    }
    let mut c = protox::Compiler::with_file_resolver(protox::file::GoogleFileResolver::new());
    c.include_imports(true);
    c.open_file(name).ok()?;
    c.file_descriptor_set()
        .file
        .into_iter()
        .find(|f| f.name() == name)
}

/// Loads the schema of a server through gRPC reflection (v1, then v1alpha).
pub async fn from_reflection(channel: Channel) -> Result<Schema, GrpcError> {
    let files = match reflect_v1(channel.clone()).await {
        Ok(f) => f,
        Err(s) if s.code() == tonic::Code::Unimplemented => {
            reflect_v1alpha(channel).await.map_err(|s| {
                GrpcError::Reflection(format!(
                    "{}: {}",
                    crate::code_name(s.code() as i32),
                    s.message()
                ))
            })?
        }
        Err(s) => {
            return Err(GrpcError::Reflection(format!(
                "{}: {}",
                crate::code_name(s.code() as i32),
                s.message()
            )));
        }
    };
    let mut names: HashSet<String> = files.iter().map(|f| f.name().to_string()).collect();
    let mut all = files;
    let missing: Vec<String> = all
        .iter()
        .flat_map(|f| f.dependency.clone())
        .filter(|d| !names.contains(d))
        .collect();
    for m in missing {
        if names.contains(&m) {
            continue;
        }
        if let Some(fd) = well_known(&m) {
            names.insert(m);
            all.push(fd);
        }
    }
    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_protos(all)
        .map_err(|e| GrpcError::Reflection(format!("server returned an incomplete schema: {e}")))?;
    Ok(Schema { pool })
}
