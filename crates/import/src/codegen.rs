//! Code that sends a request with another tool: Python `requests`,
//! JavaScript `fetch` (Node.js 18 or later), Go `net/http`, HTTPie and wget.
//!
//! The code sends what the request file defines. Each tool can add its own
//! headers (a `User-Agent`, for example), so the bytes on the wire can differ
//! from what Sendito sends. Secret variables stay `{{placeholders}}` unless
//! the caller asks for their values.

use std::path::Path;

use sendito_core::model::{Auth, HttpRequest};
use sendito_core::resolve::{PartData, ResolvedBody, ResolvedPart};
use sendito_core::vars::Scope;

use crate::Prepared;
use crate::curl::shell_quote;

/// A target of the code export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Python,
    Fetch,
    Go,
    Httpie,
    Wget,
}

impl Lang {
    pub const ALL: [Lang; 5] = [
        Lang::Python,
        Lang::Fetch,
        Lang::Go,
        Lang::Httpie,
        Lang::Wget,
    ];

    /// The name the CLI and the `:code` command take.
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Python => "python",
            Lang::Fetch => "fetch",
            Lang::Go => "go",
            Lang::Httpie => "httpie",
            Lang::Wget => "wget",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|l| l.as_str() == s)
    }

    /// The names, for a usage message: `python, fetch, go, httpie, wget`.
    pub fn names() -> String {
        Self::ALL.map(Lang::as_str).join(", ")
    }

    fn comment(self) -> &'static str {
        match self {
            Lang::Python | Lang::Httpie | Lang::Wget => "#",
            Lang::Fetch | Lang::Go => "//",
        }
    }
}

/// The credentials the tool sends itself (`None` when a header carries the
/// auth, or there is none).
enum Creds {
    None,
    Basic(String, String),
    Digest(String, String),
    /// AWS Signature V4: no tool here signs a request.
    AwsSigV4,
}

fn creds(p: &Prepared) -> Creds {
    if p.own_authorization {
        return Creds::None;
    }
    match &p.src.auth {
        Auth::Basic { username, password } => Creds::Basic(p.render(username), p.render(password)),
        Auth::Digest { username, password } => {
            Creds::Digest(p.render(username), p.render(password))
        }
        Auth::AwsSigV4 { .. } => Creds::AwsSigV4,
        _ => Creds::None,
    }
}

/// The code for `lang` that sends `req`.
pub fn export(
    lang: Lang,
    req: &HttpRequest,
    scope: &Scope,
    include_secrets: bool,
    base_dir: &Path,
) -> String {
    let p = match crate::prepare(req, scope, include_secrets, base_dir) {
        Ok(p) => p,
        Err(message) => return format!("{} cannot export: {message}\n", lang.comment()),
    };
    let mut notes = Vec::new();
    let creds = creds(&p);
    if let Creds::AwsSigV4 = creds {
        notes.push("AWS Signature V4 is not generated: sign the request with the AWS SDK.".into());
    }
    let mut code = match lang {
        Lang::Python => python(&p, &creds, &mut notes),
        Lang::Fetch => fetch(&p, &creds, &mut notes),
        Lang::Go => go(&p, &creds, &mut notes),
        Lang::Httpie => httpie(&p, &creds, &mut notes),
        Lang::Wget => wget(&p, &creds, &mut notes),
    };
    if !notes.is_empty() {
        let mut head = String::new();
        for n in notes {
            head.push_str(&format!("{} {n}\n", lang.comment()));
        }
        code = head + &code;
    }
    code
}

/// A string literal for Python, JavaScript and Go: a JSON string is valid in
/// the three languages.
fn lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn path_text(p: &Path) -> String {
    p.display().to_string()
}

fn body_text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn secs(ms: u64) -> String {
    let s = format!("{:.3}", ms as f64 / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn python(p: &Prepared, creds: &Creds, notes: &mut Vec<String>) -> String {
    let r = &p.resolved;
    let mut args = vec![lit(&r.method), lit(&r.url)];
    let mut seen = std::collections::HashSet::new();
    if !r.headers.is_empty() {
        let mut h = String::from("headers={\n");
        for (n, v) in &r.headers {
            if !seen.insert(n.to_ascii_lowercase()) {
                notes.push(format!(
                    "The header {n} appears more than once: a Python dict keeps one value."
                ));
            }
            h.push_str(&format!("        {}: {},\n", lit(n), lit(v)));
        }
        h.push_str("    }");
        args.push(h);
    }
    match &r.body {
        ResolvedBody::None | ResolvedBody::Invalid(_) => {}
        ResolvedBody::Bytes(b) => args.push(format!("data={}.encode()", lit(&body_text(b)))),
        ResolvedBody::File(f) => args.push(format!("data=open({}, \"rb\")", lit(&path_text(f)))),
        ResolvedBody::Multipart(parts) => {
            let mut s = String::from("files=[\n");
            for part in parts {
                let ct = part
                    .content_type
                    .as_deref()
                    .map(|c| format!(", {}", lit(c)))
                    .unwrap_or_default();
                let value = match &part.data {
                    PartData::Text(t) => format!("(None, {}{ct})", lit(t)),
                    PartData::File { path, filename } => format!(
                        "({}, open({}, \"rb\"){ct})",
                        lit(filename),
                        lit(&path_text(path))
                    ),
                };
                s.push_str(&format!("        ({}, {value}),\n", lit(&part.name)));
            }
            s.push_str("    ]");
            args.push(s);
        }
    }
    match creds {
        Creds::Basic(u, pw) => args.push(format!("auth=({}, {})", lit(u), lit(pw))),
        Creds::Digest(u, pw) => args.push(format!(
            "auth=requests.auth.HTTPDigestAuth({}, {})",
            lit(u),
            lit(pw)
        )),
        Creds::None | Creds::AwsSigV4 => {}
    }
    if let Some(t) = r.timeout_ms {
        args.push(format!("timeout={}", secs(t)));
    }
    if !r.follow_redirects {
        args.push("allow_redirects=False".into());
    }
    if !r.verify_tls {
        args.push("verify=False".into());
    }
    let mut out = String::from("import requests\n\nresponse = requests.request(\n");
    for a in args {
        out.push_str(&format!("    {a},\n"));
    }
    out.push_str(")\nprint(response.status_code)\nprint(response.text)\n");
    out
}

fn fetch(p: &Prepared, creds: &Creds, notes: &mut Vec<String>) -> String {
    let r = &p.resolved;
    let mut fs = false;
    let mut pre = String::new();
    let mut headers: Vec<(String, String)> =
        r.headers.iter().map(|(n, v)| (lit(n), lit(v))).collect();
    match creds {
        Creds::Basic(u, pw) => headers.push((
            lit("Authorization"),
            format!(
                "\"Basic \" + Buffer.from({}).toString(\"base64\")",
                lit(&format!("{u}:{pw}"))
            ),
        )),
        Creds::Digest(..) => notes
            .push("Digest auth is not generated: fetch cannot answer a Digest challenge.".into()),
        Creds::None | Creds::AwsSigV4 => {}
    }
    let body = match &r.body {
        ResolvedBody::None | ResolvedBody::Invalid(_) => None,
        ResolvedBody::Bytes(b) => Some(lit(&body_text(b))),
        ResolvedBody::File(f) => {
            fs = true;
            Some(format!("readFileSync({})", lit(&path_text(f))))
        }
        ResolvedBody::Multipart(parts) => {
            pre.push_str("const form = new FormData();\n");
            for part in parts {
                match &part.data {
                    PartData::Text(t) => {
                        if let Some(ct) = &part.content_type {
                            notes.push(format!(
                                "The part {} has the type {ct}: FormData sends a text part without one.",
                                part.name
                            ));
                        }
                        pre.push_str(&format!("form.append({}, {});\n", lit(&part.name), lit(t)));
                    }
                    PartData::File { path, filename } => {
                        fs = true;
                        let ty = part
                            .content_type
                            .as_deref()
                            .map(|c| format!(", {{ type: {} }}", lit(c)))
                            .unwrap_or_default();
                        pre.push_str(&format!(
                            "form.append({}, new Blob([readFileSync({})]{ty}), {});\n",
                            lit(&part.name),
                            lit(&path_text(path)),
                            lit(filename)
                        ));
                    }
                }
            }
            pre.push('\n');
            Some("form".to_string())
        }
    };
    if !r.verify_tls {
        notes.push(
            "TLS verification is off for this request: fetch has no option for it (Node.js reads NODE_TLS_REJECT_UNAUTHORIZED=0, unsafe)."
                .into(),
        );
    }
    let mut out = String::from("// Node.js 18 or later\n");
    if fs {
        out.push_str("import { readFileSync } from \"node:fs\";\n");
    }
    out.push('\n');
    out.push_str(&pre);
    out.push_str(&format!(
        "const response = await fetch({}, {{\n",
        lit(&r.url)
    ));
    out.push_str(&format!("  method: {},\n", lit(&r.method)));
    if !headers.is_empty() {
        out.push_str("  headers: [\n");
        for (n, v) in headers {
            out.push_str(&format!("    [{n}, {v}],\n"));
        }
        out.push_str("  ],\n");
    }
    if let Some(b) = body {
        out.push_str(&format!("  body: {b},\n"));
    }
    if !r.follow_redirects {
        out.push_str("  redirect: \"manual\",\n");
    }
    if let Some(t) = r.timeout_ms {
        out.push_str(&format!("  signal: AbortSignal.timeout({t}),\n"));
    }
    out.push_str("});\nconsole.log(response.status);\nconsole.log(await response.text());\n");
    out
}

/// The `Content-Disposition` value of a multipart part, quoted as Go's
/// `mime/multipart` writes it.
fn disposition(part: &ResolvedPart) -> String {
    let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    match &part.data {
        PartData::Text(_) => format!("form-data; name=\"{}\"", q(&part.name)),
        PartData::File { filename, .. } => format!(
            "form-data; name=\"{}\"; filename=\"{}\"",
            q(&part.name),
            q(filename)
        ),
    }
}

fn go(p: &Prepared, creds: &Creds, notes: &mut Vec<String>) -> String {
    let r = &p.resolved;
    let mut imports = std::collections::BTreeSet::from(["fmt", "io", "net/http"]);
    let mut pre = String::new();
    let body = match &r.body {
        ResolvedBody::None | ResolvedBody::Invalid(_) => "nil".to_string(),
        ResolvedBody::Bytes(b) => {
            imports.insert("strings");
            format!("strings.NewReader({})", lit(&body_text(b)))
        }
        ResolvedBody::File(f) => {
            imports.insert("os");
            pre.push_str(&format!(
                "\tbody, err := os.Open({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer body.Close()\n",
                lit(&path_text(f))
            ));
            "body".to_string()
        }
        ResolvedBody::Multipart(parts) => {
            imports.extend(["bytes", "mime/multipart", "net/textproto"]);
            pre.push_str("\tvar buf bytes.Buffer\n\tform := multipart.NewWriter(&buf)\n");
            for part in parts {
                pre.push_str("\t{\n\t\th := make(textproto.MIMEHeader)\n");
                pre.push_str(&format!(
                    "\t\th.Set(\"Content-Disposition\", {})\n",
                    lit(&disposition(part))
                ));
                let ct = match (&part.content_type, &part.data) {
                    (Some(ct), _) => Some(ct.as_str()),
                    (None, PartData::File { .. }) => Some("application/octet-stream"),
                    (None, PartData::Text(_)) => None,
                };
                if let Some(ct) = ct {
                    pre.push_str(&format!("\t\th.Set(\"Content-Type\", {})\n", lit(ct)));
                }
                pre.push_str(
                    "\t\tpart, err := form.CreatePart(h)\n\t\tif err != nil {\n\t\t\tpanic(err)\n\t\t}\n",
                );
                match &part.data {
                    PartData::Text(t) => {
                        pre.push_str(&format!("\t\tio.WriteString(part, {})\n", lit(t)));
                    }
                    PartData::File { path, .. } => {
                        imports.insert("os");
                        pre.push_str(&format!(
                            "\t\tdata, err := os.ReadFile({})\n\t\tif err != nil {{\n\t\t\tpanic(err)\n\t\t}}\n\t\tpart.Write(data)\n",
                            lit(&path_text(path))
                        ));
                    }
                }
                pre.push_str("\t}\n");
            }
            pre.push_str("\tform.Close()\n");
            "&buf".to_string()
        }
    };
    let mut out_main = pre;
    out_main.push_str(&format!(
        "\treq, err := http.NewRequest({}, {}, {body})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n",
        lit(&r.method),
        lit(&r.url)
    ));
    for (n, v) in &r.headers {
        out_main.push_str(&format!("\treq.Header.Add({}, {})\n", lit(n), lit(v)));
    }
    if matches!(r.body, ResolvedBody::Multipart(_)) {
        out_main.push_str("\treq.Header.Set(\"Content-Type\", form.FormDataContentType())\n");
    }
    match creds {
        Creds::Basic(u, pw) => {
            out_main.push_str(&format!("\treq.SetBasicAuth({}, {})\n", lit(u), lit(pw)));
        }
        Creds::Digest(..) => notes.push(
            "Digest auth is not generated: net/http cannot answer a Digest challenge.".into(),
        ),
        Creds::None | Creds::AwsSigV4 => {}
    }
    out_main.push_str("\tclient := &http.Client{\n");
    if let Some(t) = r.timeout_ms {
        imports.insert("time");
        out_main.push_str(&format!("\t\tTimeout: {t} * time.Millisecond,\n"));
    }
    if !r.follow_redirects {
        out_main.push_str(
            "\t\tCheckRedirect: func(*http.Request, []*http.Request) error {\n\t\t\treturn http.ErrUseLastResponse\n\t\t},\n",
        );
    }
    if !r.verify_tls {
        imports.insert("crypto/tls");
        out_main.push_str(
            "\t\tTransport: &http.Transport{TLSClientConfig: &tls.Config{InsecureSkipVerify: true}},\n",
        );
    }
    out_main.push_str("\t}\n");
    out_main.push_str(
        "\tresp, err := client.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer resp.Body.Close()\n\tout, err := io.ReadAll(resp.Body)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tfmt.Println(resp.Status)\n\tfmt.Println(string(out))\n",
    );
    let mut out = String::from("package main\n\nimport (\n");
    for i in imports {
        out.push_str(&format!("\t\"{i}\"\n"));
    }
    out.push_str(")\n\nfunc main() {\n");
    out.push_str(&out_main);
    out.push_str("}\n");
    out
}

/// Shell words, one option with its value per line.
fn shell_lines(words: Vec<String>) -> String {
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 1 {
            out.push_str(" \\\n  ");
        } else if i == 1 {
            out.push(' ');
        }
        out.push_str(w);
    }
    out.push('\n');
    out
}

fn httpie(p: &Prepared, creds: &Creds, notes: &mut Vec<String>) -> String {
    let r = &p.resolved;
    let mut words = vec!["http".to_string()];
    let mut stdin = None;
    match creds {
        Creds::Basic(u, pw) => words.push(format!("--auth={}", shell_quote(&format!("{u}:{pw}")))),
        Creds::Digest(u, pw) => {
            words.push("--auth-type=digest".into());
            words.push(format!("--auth={}", shell_quote(&format!("{u}:{pw}"))));
        }
        Creds::None | Creds::AwsSigV4 => {}
    }
    match &r.body {
        ResolvedBody::None | ResolvedBody::Invalid(_) => {}
        ResolvedBody::Bytes(b) => words.push(format!("--raw={}", shell_quote(&body_text(b)))),
        ResolvedBody::File(f) => stdin = Some(path_text(f)),
        ResolvedBody::Multipart(_) => words.push("--multipart".into()),
    }
    if r.follow_redirects {
        words.push("--follow".into());
        words.push("--max-redirects=10".into());
    }
    if !r.verify_tls {
        words.push("--verify=no".into());
    }
    if let Some(t) = r.timeout_ms {
        words.push(format!("--timeout={}", secs(t)));
    }
    words.push(shell_quote(&r.method));
    words.push(shell_quote(&r.url));
    for (n, v) in &r.headers {
        // `Name;` sends the header with an empty value; `Name:` would drop it.
        if v.is_empty() {
            words.push(shell_quote(&format!("{n};")));
        } else {
            words.push(shell_quote(&format!("{n}:{v}")));
        }
    }
    if let ResolvedBody::Multipart(parts) = &r.body {
        for part in parts {
            match &part.data {
                PartData::Text(t) => {
                    if let Some(ct) = &part.content_type {
                        notes.push(format!(
                            "The part {} has the type {ct}: HTTPie sends a text field without one.",
                            part.name
                        ));
                    }
                    words.push(shell_quote(&format!("{}={t}", part.name)));
                }
                PartData::File { path, filename } => {
                    let default_name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if *filename != default_name {
                        notes.push(format!(
                            "The part {} has the file name {filename}: HTTPie sends the name of the file on disk.",
                            part.name
                        ));
                    }
                    let ty = part
                        .content_type
                        .as_deref()
                        .map(|c| format!(";type={c}"))
                        .unwrap_or_default();
                    words.push(shell_quote(&format!(
                        "{}@{}{ty}",
                        part.name,
                        path_text(path)
                    )));
                }
            }
        }
    }
    let mut out = shell_lines(words);
    if let Some(f) = stdin {
        out.pop();
        out.push_str(&format!(" \\\n  < {}\n", shell_quote(&f)));
    }
    out
}

fn wget(p: &Prepared, creds: &Creds, notes: &mut Vec<String>) -> String {
    let r = &p.resolved;
    let mut words = vec![
        "wget".to_string(),
        "--quiet".into(),
        "--output-document=-".into(),
        "--content-on-error".into(),
        format!("--method={}", shell_quote(&r.method)),
    ];
    for (n, v) in &r.headers {
        words.push(format!("--header={}", shell_quote(&format!("{n}: {v}"))));
    }
    match creds {
        Creds::Basic(u, pw) | Creds::Digest(u, pw) => {
            words.push(format!("--user={}", shell_quote(u)));
            words.push(format!("--password={}", shell_quote(pw)));
            if matches!(creds, Creds::Basic(..)) {
                // Send Basic credentials at once, not after a 401.
                words.push("--auth-no-challenge".into());
            }
        }
        Creds::None | Creds::AwsSigV4 => {}
    }
    match &r.body {
        ResolvedBody::None | ResolvedBody::Invalid(_) => {}
        ResolvedBody::Bytes(b) => {
            words.push(format!("--body-data={}", shell_quote(&body_text(b))));
        }
        ResolvedBody::File(f) => words.push(format!("--body-file={}", shell_quote(&path_text(f)))),
        ResolvedBody::Multipart(_) => {
            notes
                .push("wget cannot send a multipart body: the command leaves the body out.".into());
        }
    }
    words.push(format!(
        "--max-redirect={}",
        if r.follow_redirects { 10 } else { 0 }
    ));
    if !r.verify_tls {
        words.push("--no-check-certificate".into());
    }
    if let Some(t) = r.timeout_ms {
        words.push(format!("--timeout={}", secs(t)));
    }
    words.push(shell_quote(&r.url));
    shell_lines(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::model::{Body, HttpOptions, KeyValue, MultipartPart, PartKind};
    use sendito_core::vars::VarValue;
    use std::collections::BTreeMap;

    fn scope() -> Scope {
        let mut m = BTreeMap::new();
        m.insert("token".into(), VarValue::secret("s3cr3t"));
        m.insert("host".into(), VarValue::plain("api.test"));
        Scope::new().with_layer("env", m)
    }

    fn json_post() -> HttpRequest {
        HttpRequest {
            method: "POST".into(),
            url: "https://{{host}}/users".into(),
            params: vec![KeyValue::new("q", "a b")],
            headers: vec![
                KeyValue::new("Authorization", "Bearer {{token}}"),
                KeyValue::new("X-Quote", "say \"hi\""),
            ],
            body: Body::Json {
                text: "{\"name\":\"ana\"}".into(),
            },
            options: HttpOptions {
                timeout_ms: Some(2500),
                follow_redirects: false,
                verify_tls: false,
                cookies: None,
            },
            ..Default::default()
        }
    }

    fn multipart() -> HttpRequest {
        HttpRequest {
            method: "POST".into(),
            url: "http://h/upload".into(),
            body: Body::Multipart {
                parts: vec![
                    MultipartPart {
                        name: "title".into(),
                        kind: PartKind::Text {
                            value: "hello".into(),
                        },
                        content_type: String::new(),
                        enabled: true,
                    },
                    MultipartPart {
                        name: "file".into(),
                        kind: PartKind::File {
                            path: "img/a.png".into(),
                            filename: None,
                        },
                        content_type: "image/png".into(),
                        enabled: true,
                    },
                ],
            },
            ..Default::default()
        }
    }

    fn code(lang: Lang, req: &HttpRequest) -> String {
        export(lang, req, &scope(), false, Path::new("/c"))
    }

    #[test]
    fn names_parse_back() {
        for l in Lang::ALL {
            assert_eq!(Lang::parse(l.as_str()), Some(l));
        }
        assert_eq!(Lang::parse(" Python "), Some(Lang::Python));
        assert_eq!(Lang::parse("ruby"), None);
        assert_eq!(Lang::names(), "python, fetch, go, httpie, wget");
    }

    #[test]
    fn secrets_stay_placeholders_unless_asked() {
        for l in Lang::ALL {
            let c = code(l, &json_post());
            assert!(c.contains("{{token}}"), "{l:?}: {c}");
            assert!(!c.contains("s3cr3t"), "{l:?}: {c}");
            assert!(c.contains("https://api.test/users?q=a%20b"), "{l:?}: {c}");
            let shown = export(l, &json_post(), &scope(), true, Path::new("/c"));
            assert!(shown.contains("Bearer s3cr3t"), "{l:?}: {shown}");
        }
    }

    #[test]
    fn python_request() {
        assert_eq!(
            code(Lang::Python, &json_post()),
            r#"import requests

response = requests.request(
    "POST",
    "https://api.test/users?q=a%20b",
    headers={
        "Authorization": "Bearer {{token}}",
        "X-Quote": "say \"hi\"",
        "Content-Type": "application/json",
    },
    data="{\"name\":\"ana\"}".encode(),
    timeout=2.5,
    allow_redirects=False,
    verify=False,
)
print(response.status_code)
print(response.text)
"#
        );
    }

    #[test]
    fn fetch_request() {
        let c = code(Lang::Fetch, &json_post());
        assert!(c.starts_with("// TLS verification is off"), "{c}");
        assert!(c.contains("const response = await fetch(\"https://api.test/users?q=a%20b\", {\n  method: \"POST\",\n"), "{c}");
        assert!(
            c.contains("    [\"X-Quote\", \"say \\\"hi\\\"\"],\n"),
            "{c}"
        );
        assert!(c.contains("  body: \"{\\\"name\\\":\\\"ana\\\"}\",\n  redirect: \"manual\",\n  signal: AbortSignal.timeout(2500),\n"), "{c}");
        assert!(!c.contains("node:fs"));
    }

    #[test]
    fn go_request_imports_only_what_it_uses() {
        let c = code(Lang::Go, &json_post());
        assert!(c.contains("import (\n\t\"crypto/tls\"\n\t\"fmt\"\n\t\"io\"\n\t\"net/http\"\n\t\"strings\"\n\t\"time\"\n)"), "{c}");
        assert!(c.contains("req, err := http.NewRequest(\"POST\", \"https://api.test/users?q=a%20b\", strings.NewReader("), "{c}");
        assert!(c.contains("\t\tTimeout: 2500 * time.Millisecond,\n"));
        assert!(c.contains("http.ErrUseLastResponse"));
        let get = code(
            Lang::Go,
            &HttpRequest {
                url: "http://h/".into(),
                ..Default::default()
            },
        );
        assert!(
            get.contains("import (\n\t\"fmt\"\n\t\"io\"\n\t\"net/http\"\n)"),
            "{get}"
        );
        assert!(get.contains("http.NewRequest(\"GET\", \"http://h/\", nil)"));
    }

    #[test]
    fn shell_commands() {
        assert_eq!(
            code(Lang::Httpie, &json_post()),
            "http --raw='{\"name\":\"ana\"}' \\\n  --verify=no \\\n  --timeout=2.5 \\\n  POST \\\n  'https://api.test/users?q=a%20b' \\\n  'Authorization:Bearer {{token}}' \\\n  'X-Quote:say \"hi\"' \\\n  Content-Type:application/json\n"
        );
        let w = code(Lang::Wget, &json_post());
        assert!(w.starts_with("wget --quiet \\\n  --output-document=- \\\n  --content-on-error \\\n  --method=POST \\\n"), "{w}");
        assert!(
            w.contains("--header='Content-Type: application/json'"),
            "{w}"
        );
        assert!(w.contains("--max-redirect=0 \\\n  --no-check-certificate \\\n  --timeout=2.5 \\\n  'https://api.test/users?q=a%20b'\n"), "{w}");
    }

    #[test]
    fn multipart_bodies() {
        let req = multipart();
        let py = code(Lang::Python, &req);
        assert!(py.contains("(\"title\", (None, \"hello\")),"), "{py}");
        assert!(
            py.contains("(\"file\", (\"a.png\", open(\"/c/img/a.png\", \"rb\"), \"image/png\")),"),
            "{py}"
        );
        let js = code(Lang::Fetch, &req);
        assert!(
            js.contains("import { readFileSync } from \"node:fs\";"),
            "{js}"
        );
        assert!(js.contains("form.append(\"file\", new Blob([readFileSync(\"/c/img/a.png\")], { type: \"image/png\" }), \"a.png\");"), "{js}");
        assert!(js.contains("  body: form,\n"));
        let go = code(Lang::Go, &req);
        assert!(go.contains("\"mime/multipart\""), "{go}");
        assert!(go.contains("h.Set(\"Content-Disposition\", \"form-data; name=\\\"file\\\"; filename=\\\"a.png\\\"\")"), "{go}");
        assert!(go.contains("req.Header.Set(\"Content-Type\", form.FormDataContentType())"));
        let hp = code(Lang::Httpie, &req);
        assert!(hp.contains("--multipart"), "{hp}");
        assert!(hp.contains("'file@/c/img/a.png;type=image/png'"), "{hp}");
        let wg = code(Lang::Wget, &req);
        assert!(
            wg.starts_with("# wget cannot send a multipart body"),
            "{wg}"
        );
    }

    #[test]
    fn auth_kinds() {
        let basic = HttpRequest {
            url: "http://h/".into(),
            auth: Auth::Basic {
                username: "ana".into(),
                password: "{{token}}".into(),
            },
            ..Default::default()
        };
        assert!(code(Lang::Python, &basic).contains("auth=(\"ana\", \"{{token}}\"),"));
        assert!(
            code(Lang::Fetch, &basic)
                .contains("\"Basic \" + Buffer.from(\"ana:{{token}}\").toString(\"base64\")")
        );
        assert!(code(Lang::Go, &basic).contains("req.SetBasicAuth(\"ana\", \"{{token}}\")"));
        assert!(code(Lang::Httpie, &basic).contains("--auth='ana:{{token}}'"));
        assert!(code(Lang::Wget, &basic).contains("--auth-no-challenge"));
        let digest = HttpRequest {
            auth: Auth::Digest {
                username: "ana".into(),
                password: "p".into(),
            },
            ..basic.clone()
        };
        assert!(
            code(Lang::Python, &digest).contains("requests.auth.HTTPDigestAuth(\"ana\", \"p\")")
        );
        assert!(code(Lang::Go, &digest).starts_with("// Digest auth is not generated"));
        assert!(code(Lang::Httpie, &digest).contains("--auth-type=digest"));
        let aws = HttpRequest {
            auth: Auth::AwsSigV4 {
                access_key: "k".into(),
                secret_key: "s".into(),
                session_token: "t".into(),
                region: "us-east-1".into(),
                service: "s3".into(),
            },
            ..basic
        };
        let c = code(Lang::Python, &aws);
        assert!(c.starts_with("# AWS Signature V4 is not generated"), "{c}");
        assert!(c.contains("\"X-Amz-Security-Token\": \"t\""), "{c}");
    }

    #[test]
    fn a_file_body() {
        let req = HttpRequest {
            method: "PUT".into(),
            url: "http://h/f".into(),
            body: Body::File {
                path: "data.bin".into(),
                content_type: String::new(),
            },
            ..Default::default()
        };
        assert!(code(Lang::Python, &req).contains("data=open(\"/c/data.bin\", \"rb\"),"));
        assert!(code(Lang::Go, &req).contains("body, err := os.Open(\"/c/data.bin\")"));
        assert!(code(Lang::Httpie, &req).ends_with(" \\\n  < /c/data.bin\n"));
        assert!(code(Lang::Wget, &req).contains("--body-file=/c/data.bin"));
    }
}
