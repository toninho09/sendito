//! The help (F1): the guide, the tutorials, the key tables and the function
//! tables. The prose is in `crates/ui/src/help/`. In it, `{key:action}` and
//! `{key:context:action}` show the key of a keymap action, so the text shows
//! the keys of the current mode and the keys the user changed, and a
//! `{options}` line becomes the table of the Options tab.

use super::*;

const GUIDE: &str = "Guide";
const TUTORIALS: &str = "Tutorials";
const KEYS: &str = "Keys";
const FUNCTIONS: &str = "Functions";

/// The line of a guide or tutorial text that becomes the Options table.
const OPTIONS_MARKER: &str = "{options}";

/// The guide: the sidebar name and the text of each section.
const GUIDE_TEXTS: &[(&str, &str)] = &[
    ("Start here", include_str!("../help/start_here.txt")),
    ("How it works", include_str!("../help/how_it_works.txt")),
];

/// The tutorials, in sidebar order.
const TUTORIAL_TEXTS: &[(&str, &str)] = &[
    (
        "First request",
        include_str!("../help/tutorials/first_request.txt"),
    ),
    (
        "Body types",
        include_str!("../help/tutorials/body_types.txt"),
    ),
    (
        "GraphQL request",
        include_str!("../help/tutorials/graphql_request.txt"),
    ),
    (
        "Save and organize",
        include_str!("../help/tutorials/save_and_organize.txt"),
    ),
    (
        "Docs and examples",
        include_str!("../help/tutorials/docs_and_examples.txt"),
    ),
    (
        "Environments",
        include_str!("../help/tutorials/environments.txt"),
    ),
    (
        "Secrets and auth",
        include_str!("../help/tutorials/secrets_and_auth.txt"),
    ),
    (
        "Chain requests",
        include_str!("../help/tutorials/chain_requests.txt"),
    ),
    (
        "Read a response",
        include_str!("../help/tutorials/read_a_response.txt"),
    ),
    (
        "Watch a request",
        include_str!("../help/tutorials/watch_request.txt"),
    ),
    (
        "History and cookies",
        include_str!("../help/tutorials/history_and_cookies.txt"),
    ),
    (
        "Import and export",
        include_str!("../help/tutorials/import_and_export.txt"),
    ),
    ("gRPC call", include_str!("../help/tutorials/grpc_call.txt")),
    (
        "Streams (WS, SSE, MQTT)",
        include_str!("../help/tutorials/streams.txt"),
    ),
    ("MCP tools", include_str!("../help/tutorials/mcp_tools.txt")),
    (
        "Request options",
        include_str!("../help/tutorials/request_options.txt"),
    ),
    (
        "Run a folder",
        include_str!("../help/tutorials/run_a_folder.txt"),
    ),
    ("Customize", include_str!("../help/tutorials/customize.txt")),
];

/// The sections of the help (F1), in sidebar order: the guide, the
/// tutorials, the keys ("All keys" holds every key section, then each key
/// section) and the functions a request or a script calls.
pub(super) fn help_sections(km: &Keymap, vim: bool) -> Vec<HelpSection> {
    let mut out = Vec::new();
    let mut add = |group, name, text| out.push(HelpSection { group, name, text });
    for (group, texts) in [(GUIDE, GUIDE_TEXTS), (TUTORIALS, TUTORIAL_TEXTS)] {
        for (name, text) in texts {
            let text = if text.contains(OPTIONS_MARKER) {
                text.replace(OPTIONS_MARKER, &options_table())
            } else {
                text.to_string()
            };
            add(group, name, with_keys(&text, km, vim));
        }
    }
    let rows = km.rows();
    let mut ctxs: Vec<Ctx> = Vec::new();
    for (ctx, ..) in &rows {
        if !ctxs.contains(ctx) {
            ctxs.push(*ctx);
        }
    }
    let mut all = String::from(
        "Every key of Sendito, with the keys that you changed in the config.\n\
         The next sections show the same keys, one place at a time.\n",
    );
    all.push_str(&mode_line(km, vim));
    for ctx in &ctxs {
        ctx_keys(&mut all, &rows, *ctx, vim);
    }
    fixed_keys(&mut all);
    symbols(&mut all);
    commands(&mut all, km, vim);
    add(KEYS, "All keys", all);
    for ctx in ctxs {
        let mut s = mode_line(km, vim);
        if ctx_keys(&mut s, &rows, ctx, vim) {
            add(KEYS, ctx.name(), s);
        }
    }
    let mut s = String::new();
    fixed_keys(&mut s);
    add(KEYS, "Fixed keys", s);
    let mut s = String::new();
    symbols(&mut s);
    add(KEYS, "Symbols", s);
    let mut s = String::new();
    commands(&mut s, km, vim);
    add(KEYS, "Commands", s);
    for (name, text) in help_functions() {
        add(FUNCTIONS, name, text);
    }
    out
}

/// The section for `:help <query>`, ignoring case: the first name that
/// starts with the query, else the first name that contains it, else the
/// text that has the query as a word the most times. An empty query gives
/// the first section.
pub(super) fn find_section(sections: &[HelpSection], query: &str) -> Option<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return (!sections.is_empty()).then_some(0);
    }
    let names: Vec<String> = sections.iter().map(|s| s.name.to_lowercase()).collect();
    names
        .iter()
        .position(|n| n.starts_with(&q))
        .or_else(|| names.iter().position(|n| n.contains(&q)))
        .or_else(|| most_mentions(sections, &q))
}

/// The section whose text has `q` as a word the most times; the first one
/// when two sections have the same count. None when no text has it.
fn most_mentions(sections: &[HelpSection], q: &str) -> Option<usize> {
    // From the end, because `max_by_key` keeps the last of the equals.
    (0..sections.len())
        .rev()
        .map(|i| (i, mentions(&sections[i].text, q)))
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .map(|(i, _)| i)
}

/// The number of times `q` is a word of `text`: the characters before and
/// after it are not letters or digits. Ignores case.
fn mentions(text: &str, q: &str) -> usize {
    let t = text.to_lowercase();
    let boundary = |c: char| !c.is_alphanumeric();
    t.match_indices(q)
        .filter(|&(i, m)| {
            let before = t[..i].chars().next_back().is_none_or(boundary);
            let after = t[i + m.len()..].chars().next().is_none_or(boundary);
            before && after
        })
        .count()
}

/// `text` with each `{key:action}` or `{key:context:action}` replaced by the
/// first key of the action in the current mode. An unknown name stays as
/// it is written (a test finds it).
fn with_keys(text: &str, km: &Keymap, vim: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{key:") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + "{key:".len()..];
        let Some(end) = tail.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        match key_of(&tail[..end], km, vim) {
            Some(k) => out.push_str(&k),
            None => out.push_str(&rest[i..i + "{key:".len() + end + 1]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The first key of `spec` (`action` or `context:action`), or None when the
/// name is unknown.
fn key_of(spec: &str, km: &Keymap, vim: bool) -> Option<String> {
    let (ctx, id) = match spec.split_once(':') {
        Some((c, id)) => (Some(ctx_of(c)?), id),
        None => (None, spec),
    };
    let action = Action::from_id(id)?;
    Some(match ctx {
        Some(c) => km.hint_in(c, action, vim),
        None => km.hint(action, vim),
    })
}

/// The context of a `{key:context:action}` placeholder.
fn ctx_of(name: &str) -> Option<Ctx> {
    Some(match name {
        "tree" => Ctx::Tree,
        "table" => Ctx::Table,
        "response" => Ctx::Response,
        "query" => Ctx::Query,
        "dialog" => Ctx::Dialog,
        _ => return None,
    })
}

/// The line that names the current key mode and where to change it.
fn mode_line(km: &Keymap, vim: bool) -> String {
    format!(
        "Current mode: {} (change it in Settings, {}).\n",
        if vim { "vim" } else { "conventional" },
        km.hint(Action::Settings, vim)
    )
}

/// Where the keys of `ctx` work: the line under its title.
fn ctx_intro(ctx: Ctx) -> &'static str {
    match ctx {
        Ctx::Global => "These keys work in the main screen, in all panes.",
        Ctx::Tree => "These keys work when the request tree in the sidebar has the focus.",
        Ctx::Editor => "These keys work when a text field or a text editor has the focus.",
        Ctx::Table => "These keys work in the tables: Params, Headers, Form and the dialog tables.",
        Ctx::Response => "These keys work when the response body has the focus.",
        Ctx::Query => "These keys work in the search or jq bar above the response body.",
        Ctx::Dialog => "These keys work in the dialogs that edit or ask for values.",
    }
}

/// The keys of `ctx` in the current mode, under its title. Returns whether
/// the context has a key in this mode.
fn ctx_keys(s: &mut String, rows: &[(Ctx, Action, String, String)], ctx: Ctx, vim: bool) -> bool {
    let mut any = false;
    s.push_str(&format!("\n── {} ──\n{}\n", ctx.title(), ctx_intro(ctx)));
    for (c, a, conv, vimk) in rows {
        let keys = if vim { vimk } else { conv };
        if *c != ctx || keys.is_empty() {
            continue;
        }
        any = true;
        s.push_str(&format!("  {:<28} {}\n", keys, a.description()));
    }
    any
}

/// Fixed keys, one section per place they work in.
fn fixed_keys(s: &mut String) {
    s.push_str("\nThe widgets own the keys below. The [keymap] config does not change them.\n");
    let mut place = "";
    let kw = crate::keymap::FIXED_KEYS
        .iter()
        .map(|(_, k, _)| k.chars().count())
        .max()
        .unwrap_or(28)
        .max(28);
    for (w, k, d) in crate::keymap::FIXED_KEYS {
        if *w != place {
            place = w;
            s.push_str(&format!("\n── Fixed keys · {w} ──\n"));
        }
        s.push_str(&format!("  {k:<kw$}  {d}\n"));
    }
}

fn symbols(s: &mut String) {
    s.push_str("\n── Symbols ──\nThe marks on the screen and what they tell you.\n");
    for (sym, what) in crate::keymap::SYMBOLS {
        s.push_str(&format!("  {sym:<28} {what}\n"));
    }
}

fn commands(s: &mut String, km: &Keymap, vim: bool) {
    s.push_str(&format!(
        "\n── Commands (command palette) ──\nOpen the command palette ({}), type \":\" and the command, then press Enter.\n",
        km.hint(Action::Palette, vim)
    ));
    for (c, d) in crate::overlay::COMMANDS {
        s.push_str(&format!("  {c:<28} {d}\n"));
    }
}

/// The function sections: the same tables the send runs, so the help
/// cannot drift. `FORMAT.md` has the full reference.
fn help_functions() -> Vec<(&'static str, String)> {
    let one = |title: &str, intro: &str, rows: &[Vec<String>]| {
        let mut s = String::new();
        section(&mut s, title, intro, rows);
        s
    };
    let funcs: Vec<Vec<String>> = sendito_core::funcs::FUNCS
        .iter()
        .map(|f| vec![format!("{}({})", f.name, f.params), f.doc.to_string()])
        .collect();
    let mut template = String::from(
        "The functions that a request field or a script can call.\n\
         FORMAT.md has the full reference.\n",
    );
    template.push_str(&one("Template functions", FUNC_INTRO, &funcs));
    let pre: Vec<Vec<String>> = sendito_core::funcs::PRE_SEND
        .iter()
        .map(|(c, d)| vec![(*c).to_string(), (*d).to_string()])
        .collect();
    let api: Vec<Vec<String>> = sendito_core::lua_script::API
        .iter()
        .map(|(c, p, d)| vec![(*c).to_string(), (*p).to_string(), (*d).to_string()])
        .collect();
    vec![
        ("Template functions", template),
        ("Dynamic values", dynamic_values()),
        (
            "Send-time values",
            one(
                "Values asked at send time, or from another response",
                PRE_SEND_INTRO,
                &pre,
            ),
        ),
        ("Scripts tab", script_tables()),
        (
            "Lua scripts",
            one("Lua scripts ([scripts.lua])", LUA_INTRO, &api),
        ),
        ("Lua catalog", one("Lua function catalog", LUA_CATALOG, &[])),
    ]
}

/// The dynamic values: the same table the send reads, so the help cannot
/// drift.
fn dynamic_values() -> String {
    let rows: Vec<Vec<String>> = sendito_core::vars::DYNAMIC_VARS
        .iter()
        .map(|(name, doc)| vec![format!("{{{{{}}}}}", name), (*doc).to_string()])
        .collect();
    let mut s = String::from(
        "A dynamic value is a variable that Sendito makes again for each send. \
         Write one in any field, in place of a value.\n",
    );
    section(&mut s, "Dynamic values", "", &rows);
    s.push_str(
        "\n{{$processEnv.NAME}} reads NAME from the environment of the Sendito process, \
         for example the variables of your shell. The value is a secret. A name that is \
         not set stays unresolved.\n",
    );
    s
}

/// The Scripts tab: its sections, then the sources and the operators of its
/// rows (the tables the send runs).
fn script_tables() -> String {
    let mut s = String::from(
        "The Scripts tab of a request has five sections. A folder has the same sections; \
         its scripts run first.\n",
    );
    let parts: Vec<Vec<String>> = [
        (
            "Pre-request: set variables",
            "Rows with a variable and its value. They run in order, and each row sees the rows before it.",
        ),
        (
            "After response: extract",
            "Rows that keep a part of the response in a variable.",
        ),
        (
            "After response: assertions",
            "Rows that test the response. The Results tab shows each test.",
        ),
        (
            "Pre-request: Lua script",
            "Lua code before the send. See the section Lua scripts.",
        ),
        (
            "After response: Lua script",
            "Lua code after the send. See the section Lua scripts.",
        ),
    ]
    .into_iter()
    .map(|(a, b)| vec![a.to_string(), b.to_string()])
    .collect();
    section(&mut s, "Sections", "", &parts);
    let sources: Vec<Vec<String>> = sendito_core::model::Source::ALL
        .iter()
        .map(|s| vec![s.as_str().to_string(), s.doc().to_string()])
        .collect();
    section(
        &mut s,
        "Sources (the From cell)",
        "What a row reads, and what its Expr cell selects.",
        &sources,
    );
    let ops: Vec<Vec<String>> = sendito_core::model::AssertOp::ALL
        .iter()
        .map(|o| {
            vec![
                o.symbol().to_string(),
                o.as_str().to_string(),
                o.doc().to_string(),
            ]
        })
        .collect();
    section(
        &mut s,
        "Operators (the Op cell)",
        "The Scripts tab shows the symbol. The request file writes the word.",
        &ops,
    );
    s.push_str("The operators <, <=, > and >= compare numbers as numbers and the rest as text.\n");
    s
}

/// The table of the Options tab: each field with the request kinds that have
/// it and its help line, from the editors, so the help cannot drift. A field
/// whose help line depends on the kind has one row for each line. The rows
/// keep the order of the first kind that has the field. The gRPC request has
/// proto files and the WebSocket request a GraphQL subscription, so the
/// table lists the fields that these two modes add.
fn options_table() -> String {
    let grpc = Request::Grpc(GrpcRequest {
        schema: SchemaSource::Files {
            files: Vec::new(),
            import_paths: Vec::new(),
        },
        ..Default::default()
    });
    let ws = Request::Ws(WebSocketRequest {
        graphql: Some(WsGraphql::default()),
        ..Default::default()
    });
    let requests = [
        Request::new_http(),
        grpc,
        ws,
        Request::new_sse(),
        Request::new_mqtt(),
        Request::new_mcp(),
    ];
    struct Row {
        label: &'static str,
        kinds: Vec<&'static str>,
        help: &'static str,
        /// The first row of this label, to keep the rows of a field together.
        first: usize,
    }
    let mut rows: Vec<Row> = Vec::new();
    for req in &requests {
        let ed = crate::tab::Editors::from_request(req);
        let kind = ed.kind;
        // Docs is the last tab of every kind; Options comes before it.
        let options = ed.tab_names().len() - 2;
        for field in ed.fields(options) {
            let (label, help) = (field.label(), field.help(kind));
            let first = rows
                .iter()
                .position(|r| r.label == label)
                .unwrap_or(rows.len());
            match rows.iter_mut().find(|r| r.label == label && r.help == help) {
                Some(r) => r.kinds.push(kind_name(kind)),
                None => rows.push(Row {
                    label,
                    kinds: vec![kind_name(kind)],
                    help,
                    first,
                }),
            }
        }
    }
    rows.sort_by_key(|r| r.first);
    let every = requests.len();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let kinds = if r.kinds.len() == every {
                "Every kind".to_string()
            } else {
                r.kinds.join(", ")
            };
            vec![r.label.to_string(), format!("{kinds}: {}", r.help)]
        })
        .collect();
    let mut s = String::new();
    table(&mut s, &rows);
    s
}

/// The name of a request kind in the help.
fn kind_name(kind: crate::tab::Kind) -> &'static str {
    match kind {
        crate::tab::Kind::Http => "HTTP",
        crate::tab::Kind::Grpc => "gRPC",
        crate::tab::Kind::Ws => "WebSocket",
        crate::tab::Kind::Sse => "SSE",
        crate::tab::Kind::Mqtt => "MQTT",
        crate::tab::Kind::Mcp => "MCP",
        crate::tab::Kind::Folder => "folder",
    }
}

/// A section of the help: a title, the lines of `intro`, then the rows.
fn section(out: &mut String, title: &str, intro: &str, rows: &[Vec<String>]) {
    out.push_str(&format!("\n── {title} ──\n"));
    for line in intro.lines() {
        out.push_str(&format!("{line}\n"));
    }
    table(out, rows);
}

/// The rows of a table, two spaces between the columns. Every column but the
/// last is padded to the width of its column.
fn table(out: &mut String, rows: &[Vec<String>]) {
    let cols = rows.first().map_or(0, Vec::len);
    let width: Vec<usize> = (0..cols.saturating_sub(1))
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    for row in rows {
        out.push_str("  ");
        for (c, cell) in row.iter().enumerate() {
            if c + 1 < cols {
                out.push_str(&format!("{cell:<w$}  ", w = width[c]));
            } else {
                out.push_str(cell);
            }
        }
        out.push('\n');
    }
}

const FUNC_INTRO: &str =
    "Write {{ $name(argument, ...) }} in any field. An argument is a 'literal',
a number, a variable (user_id, $uuid, $processEnv.NAME) or another call.
Calls nest up to 8 levels. A random call keeps one value for the whole send.
An unknown function, a wrong number of arguments or a bad value leaves the
placeholder unresolved, and the send tells you why.";

const PRE_SEND_INTRO: &str =
    "Every argument is a 'literal'. A send asks one question for each label.
Esc cancels the send. The third argument of $response is the policy:
  if_missing           send the named request when no response is kept (default)
  if_older:<seconds>   also send it when the kept response is older
  always               send it before every send
  never                do not send it; with no kept response, the send stops";

const LUA_INTRO: &str =
    "pre runs before the send, post runs after it (folders first, then the request).
An error in a pre script stops the send. An error in a post script is a failed test.
The scripts can use string, table, math and utf8 only: no files, no network,
no processes. A value that a script sets is a template: its {{variables}}
resolve at the send.";

const LUA_CATALOG: &str =
    "{{ $lua.name(args) }} calls a function of the *.lua files in .sendito/lua/
of the collection and in lua/ next to the global config.toml (~/.config/sendito/).
In scripts the same functions are globals. The value is a secret when an
argument is a secret or the function read a secret. An unknown function
leaves the placeholder unresolved.";

#[cfg(test)]
mod tests {
    use super::*;

    fn names(sections: &[HelpSection]) -> Vec<&str> {
        sections.iter().map(|s| s.name).collect()
    }

    #[test]
    fn help_groups_in_order_and_starts_with_the_guide() {
        let sections = help_sections(&Keymap::defaults(), false);
        assert_eq!(sections[0].name, "Start here");
        let mut groups: Vec<&str> = Vec::new();
        for s in &sections {
            if groups.last() != Some(&s.group) {
                groups.push(s.group);
            }
        }
        assert_eq!(groups, [GUIDE, TUTORIALS, KEYS, FUNCTIONS]);
        assert_eq!(names(&sections).last(), Some(&"Lua catalog"));
    }

    #[test]
    fn help_all_keys_holds_every_key_section() {
        let km = Keymap::defaults();
        let sections = help_sections(&km, false);
        let all = &sections.iter().find(|s| s.name == "All keys").unwrap().text;
        assert!(all.contains("\n── Global ──\n"), "{all}");
        assert!(
            all.contains("\n── Commands (command palette) ──\n"),
            "{all}"
        );
        assert!(!all.contains("Template functions"), "{all}");
        let header = mode_line(&km, false);
        for s in sections.iter().filter(|s| s.group == KEYS) {
            let body = s.text.strip_prefix(&header).unwrap_or(&s.text);
            assert!(all.contains(body), "{}", s.name);
        }
    }

    /// Every `{key:...}` of the prose names a known action with a key in
    /// both modes, and every `:command` of the prose exists.
    #[test]
    fn help_prose_names_real_keys_and_commands() {
        let km = Keymap::defaults();
        for (name, text) in GUIDE_TEXTS.iter().chain(TUTORIAL_TEXTS) {
            for vim in [false, true] {
                let out = with_keys(text, &km, vim);
                assert!(!out.contains("{key:"), "{name}: unknown key name");
                assert!(
                    !out.contains("(unbound)"),
                    "{name} (vim {vim}): unbound key"
                );
            }
            for (i, _) in text.match_indices(":") {
                let before = text[..i].chars().next_back();
                let word: String = text[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '!')
                    .collect();
                // A command is ":" plus a word at the start of a token.
                if word.is_empty() || before.is_some_and(|c| !c.is_whitespace() && c != '(') {
                    continue;
                }
                let known = crate::overlay::COMMANDS
                    .iter()
                    .any(|(c, _)| c.split_whitespace().next() == Some(format!(":{word}").as_str()));
                assert!(known, "{name}: no command :{word}");
            }
        }
    }

    #[test]
    fn help_find_section_prefers_a_prefix_then_a_part_then_the_text() {
        let sections = help_sections(&Keymap::defaults(), false);
        let name = |q: &str| find_section(&sections, q).map(|i| sections[i].name);
        assert_eq!(name(""), Some("Start here"));
        assert_eq!(name("grpc"), Some("gRPC call"));
        assert_eq!(name("JQ"), Some("Query (jq)"));
        assert_eq!(name("lua"), Some("Lua scripts"));
        // A word of no name: the text that has it as a word the most times.
        // "jsonpath" does not count for "xpath".
        assert_eq!(name("xpath"), Some("Scripts tab"));
        assert_eq!(name("proxy"), Some("Request options"));
        assert_eq!(name("init payload"), Some("GraphQL request"));
        assert_eq!(name("no such section"), None);
    }

    #[test]
    fn help_mentions_counts_whole_words_only() {
        assert_eq!(mentions("jsonpath, XPath 3.1 and xpath.", "xpath"), 2);
        assert_eq!(mentions("variables and variable", "variable"), 1);
        assert_eq!(mentions("Init payload", "init payload"), 1);
        assert_eq!(mentions("cookie jar", "cookies"), 0);
    }

    #[test]
    fn help_with_keys_shows_the_keys_of_the_mode() {
        let km = Keymap::defaults();
        let text = "Press {key:send} or {key:tree:tree_run_folder}.";
        let out = with_keys(text, &km, false);
        assert_eq!(
            out,
            format!(
                "Press {} or {}.",
                km.hint(Action::Send, false),
                km.hint_in(Ctx::Tree, Action::TreeRunFolder, false)
            )
        );
        assert_eq!(
            with_keys("{key:nothing} {key:", &km, false),
            "{key:nothing} {key:"
        );
    }

    #[test]
    fn help_functions_page_lists_every_call() {
        let sections = help_sections(&Keymap::defaults(), false);
        let s: String = sections
            .iter()
            .filter(|s| s.group == FUNCTIONS)
            .map(|s| s.text.as_str())
            .collect();
        for title in [
            "── Template functions ──",
            "── Values asked at send time, or from another response ──",
            "── Lua scripts ([scripts.lua]) ──",
            "── Lua function catalog ──",
        ] {
            assert!(s.contains(&format!("\n{title}\n")), "{title}");
        }
        // Every call starts a row, and the widest call of a table has the
        // two spaces that separate the columns.
        for f in sendito_core::funcs::FUNCS {
            let sig = format!("{}({})", f.name, f.params);
            assert!(s.contains(&format!("\n  {sig}")), "{sig}");
        }
        for (c, _) in sendito_core::funcs::PRE_SEND {
            assert!(s.contains(&format!("\n  {c}")), "{c}");
        }
        for (c, p, _) in sendito_core::lua_script::API {
            assert!(s.contains(&format!("\n  {c}")), "{c}");
            assert!(matches!(*p, "pre" | "post" | "pre, post"), "{c}: {p}");
        }
        assert!(
            s.contains("$hmacSha256Base64(key, text)  the HMAC-SHA256"),
            "{}",
            s
        );
        assert!(s.contains("'$.json.path')  a value"), "{}", s);
        assert!(
            s.contains("vars.set_secret(name, value)  pre, post  the same"),
            "{}",
            s
        );
    }

    /// The dynamic values and the script tables come from the tables the
    /// send runs: every row of each table has its line in the help.
    #[test]
    fn help_lists_the_dynamic_values_and_the_script_tables() {
        let sections = help_sections(&Keymap::defaults(), false);
        let text = |name: &str| {
            sections
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.text.as_str())
                .unwrap_or_default()
        };
        let dynamic = text("Dynamic values");
        assert!(dynamic.contains("\n── Dynamic values ──\n"), "{dynamic}");
        for (name, doc) in sendito_core::vars::DYNAMIC_VARS {
            assert!(dynamic.contains(&format!("  {{{{{}}}}}", name)), "{name}");
            assert!(dynamic.contains(doc), "{name}");
        }
        assert!(dynamic.contains("{{$processEnv.NAME}}"), "{dynamic}");

        let scripts = text("Scripts tab");
        for title in [
            "\n── Sections ──\n",
            "\n── Sources (the From cell) ──\n",
            "\n── Operators (the Op cell) ──\n",
        ] {
            assert!(scripts.contains(title), "{title}");
        }
        for source in sendito_core::model::Source::ALL {
            let row = format!("\n  {}  ", source.as_str());
            assert!(scripts.contains(&row), "{row}");
            assert!(scripts.contains(source.doc()), "{}", source.as_str());
        }
        for op in sendito_core::model::AssertOp::ALL {
            assert!(
                scripts.contains(&format!("\n  {}  ", op.symbol())),
                "{}",
                op.symbol()
            );
            assert!(scripts.contains(op.as_str()), "{}", op.as_str());
            assert!(scripts.contains(op.doc()), "{}", op.as_str());
        }
    }

    /// The tutorial "Request options" prints the fields of the Options tab
    /// from the editors: the `{options}` line is gone, and the fields that one
    /// kind or one mode adds are there, with their kinds.
    #[test]
    fn help_options_table_lists_the_fields_of_every_kind() {
        let sections = help_sections(&Keymap::defaults(), false);
        let text = sections
            .iter()
            .find(|s| s.name == "Request options")
            .map(|s| s.text.as_str())
            .unwrap_or_default();
        assert!(!text.contains(OPTIONS_MARKER), "{text}");
        // The line of a row: the label, then " <kinds>: " before the help.
        let has_row = |label: &str, kinds: &str| {
            text.lines()
                .any(|l| l.starts_with(&format!("  {label}")) && l.contains(&format!(" {kinds}: ")))
        };
        for (label, kinds) in [
            ("Redirects", "HTTP, SSE"),
            ("Schema", "gRPC"),
            ("Proto files", "gRPC"),
            ("Init payload", "WebSocket"),
            ("Clean session", "MQTT"),
            ("Retries", "Every kind"),
            // A help line that depends on the kind: one row for each line.
            ("Timeout", "HTTP"),
            ("Timeout", "MCP"),
            ("Deadline", "gRPC"),
            ("Deadline", "WebSocket, MQTT"),
        ] {
            assert!(has_row(label, kinds), "{label} {kinds}\n{text}");
        }
        // The rows of a field stay together.
        let row_index = |label: &str, kinds: &str| {
            text.lines()
                .position(|l| {
                    l.starts_with(&format!("  {label}")) && l.contains(&format!(" {kinds}: "))
                })
                .unwrap()
        };
        assert!(row_index("Timeout", "HTTP") < row_index("Timeout", "MCP"));
        assert!(row_index("Timeout", "MCP") < row_index("Redirects", "HTTP, SSE"));
    }
}
