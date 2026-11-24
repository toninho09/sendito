use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const ROOT_PREFIXES: [&str; 7] = [
    "crates/", "src/", "tests/", "scripts/", "docs/", "bench/", ".github/",
];
const SKIP_SCHEMES: [&str; 3] = ["http://", "https://", "mailto:"];

struct Mention<'a> {
    path: &'a str,
    symbols: Vec<&'a str>,
    from_root: bool,
    beside_doc: bool,
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn shown(root: &Path, path: &Path) -> impl std::fmt::Display {
    path.strip_prefix(root).unwrap_or(path).display()
}

fn report(problems: &[String], hint: &str) {
    assert!(
        problems.is_empty(),
        "{} stale reference(s):\n  {}\n{hint}",
        problems.len(),
        problems.join("\n  ")
    );
}

fn is_path_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'-')
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || !c.is_ascii()
}

fn current_docs(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, docs: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "reports" {
                    walk(&path, docs);
                }
            } else if path.extension().is_some_and(|e| e == "md") && name != "PLAN.md" {
                docs.push(path);
            }
        }
    }
    let mut docs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|entries| entries.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            p.extension().is_some_and(|e| e == "md")
                && p.file_name().is_none_or(|n| n != "CHANGELOG.md")
        })
        .collect();
    walk(&root.join("docs"), &mut docs);
    docs.sort();
    docs
}

fn lines(text: &str) -> impl Iterator<Item = (usize, &str, bool)> {
    let mut fenced = false;
    text.lines().enumerate().filter_map(move |(n, line)| {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            return None;
        }
        Some((n + 1, line, fenced))
    })
}

fn mentions(line: &str) -> Vec<Mention<'_>> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_path_char(bytes[i]) || (i > 0 && is_path_char(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_path_char(bytes[i]) {
            i += 1;
        }
        let raw = &line[start..i];
        let named_doc =
            !raw.contains('/') && raw.ends_with(".md") && raw.as_bytes()[0].is_ascii_alphanumeric();
        let beside_doc = raw.starts_with("../") || raw.starts_with("./");
        let from_root = ROOT_PREFIXES.iter().any(|p| raw.starts_with(p));
        if !(named_doc || beside_doc || from_root) {
            continue;
        }
        if raw.ends_with('/') && matches!(bytes.get(i), Some(b'*') | Some(b'<')) {
            continue;
        }
        let symbols = line[i..]
            .strip_prefix("::")
            .map_or_else(Vec::new, cited_symbols);
        found.push(Mention {
            path: raw.trim_end_matches('/').trim_end_matches('.'),
            symbols,
            from_root: from_root || named_doc,
            beside_doc: beside_doc || named_doc,
        });
    }
    found
}

fn cited_symbols(rest: &str) -> Vec<&str> {
    if let Some(list) = rest.strip_prefix('{') {
        list.split('}')
            .next()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        let end = rest
            .bytes()
            .position(|c| !(c.is_ascii_alphanumeric() || c == b'_'))
            .unwrap_or(rest.len());
        (end > 0).then_some(&rest[..end]).into_iter().collect()
    }
}

fn contains_word(source: &str, word: &str) -> bool {
    let bytes = source.as_bytes();
    let is_ident = |i: usize| bytes.get(i).is_some_and(|c| is_ident_char(*c));
    source
        .match_indices(word)
        .any(|(i, _)| !is_ident(i.wrapping_sub(1)) && !is_ident(i + word.len()))
}

fn make_targets(root: &Path) -> BTreeSet<String> {
    read(&root.join("Makefile"))
        .lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        })
        .map(str::to_string)
        .collect()
}

fn cited_targets(code: &str) -> Vec<&str> {
    let bytes = code.as_bytes();
    code.match_indices("make ")
        .filter(|(i, _)| {
            *i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric() || matches!(bytes[i - 1], b'-' | b'_'))
        })
        .filter_map(|(i, _)| {
            let rest = &code[i + 5..];
            let end = rest
                .bytes()
                .position(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'))
                .unwrap_or(rest.len());
            (end > 0).then_some(&rest[..end])
        })
        .collect()
}

fn code_segments(line: &str, fenced: bool) -> Vec<&str> {
    if fenced {
        vec![line]
    } else {
        line.split('`').skip(1).step_by(2).collect()
    }
}

fn link_targets(line: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find("](") {
        let after = &rest[i + 2..];
        let end = after.find(')').unwrap_or(after.len());
        found.push(after[..end].trim());
        rest = &after[end..];
    }
    found
}

fn heading_slugs(text: &str) -> BTreeSet<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut slugs = BTreeSet::new();
    for (_, line, _) in lines(text) {
        let Some(heading) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        let hashes = 1 + heading.chars().take_while(|c| *c == '#').count();
        let title = &line.trim_start()[hashes..];
        if !title.starts_with(' ') {
            continue;
        }
        let slug = title
            .trim()
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            .collect::<String>()
            .replace(' ', "-");
        let count = seen.entry(slug.clone()).or_default();
        slugs.insert(if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        });
        *count += 1;
    }
    slugs
}

#[test]
fn cited_paths_and_symbols_exist() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut problems = Vec::new();
    for doc in current_docs(&root) {
        let text = read(&doc);
        let dir = doc.parent().unwrap_or(&root);
        for (n, line, _) in lines(&text) {
            for mention in mentions(line) {
                let mut paths = Vec::new();
                if mention.from_root {
                    paths.push(root.join(mention.path));
                }
                if mention.beside_doc {
                    paths.push(dir.join(mention.path));
                }
                let Some(found) = paths.iter().find(|p| p.exists()) else {
                    problems.push(format!(
                        "{}:{n}: {} does not exist",
                        shown(&root, &doc),
                        mention.path
                    ));
                    continue;
                };
                if mention.symbols.is_empty() || !found.is_file() {
                    continue;
                }
                let source = sources
                    .entry(found.clone())
                    .or_insert_with(|| read(found))
                    .clone();
                for symbol in &mention.symbols {
                    if !contains_word(&source, symbol) {
                        problems.push(format!(
                            "{}:{n}: {} has no `{symbol}`",
                            shown(&root, &doc),
                            shown(&root, found)
                        ));
                    }
                }
            }
        }
    }
    report(
        &problems,
        "Point the line at the file or symbol that replaced it, or write a generated \
         output as a pattern (directory/<name>.ext).",
    );
}

#[test]
fn cited_make_targets_exist() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let targets = make_targets(&root);
    let mut problems = Vec::new();
    for doc in current_docs(&root) {
        let text = read(&doc);
        for (n, line, fenced) in lines(&text) {
            for code in code_segments(line, fenced) {
                for cited in cited_targets(code) {
                    if !targets.contains(cited) {
                        problems.push(format!(
                            "{}:{n}: `make {cited}` is not a Makefile target",
                            shown(&root, &doc)
                        ));
                    }
                }
            }
        }
    }
    report(
        &problems,
        "Name an existing target (`make help` lists them), or add it to the Makefile.",
    );
}

#[test]
fn links_resolve() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut slugs: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut problems = Vec::new();
    for doc in current_docs(&root) {
        let text = read(&doc);
        let dir = doc.parent().unwrap_or(&root);
        for (n, line, fenced) in lines(&text) {
            if fenced {
                continue;
            }
            for target in link_targets(line) {
                let target = match target.find(" \"") {
                    Some(i) => &target[..i],
                    None => target,
                };
                let target = target.trim_matches(['<', '>']);
                if SKIP_SCHEMES.iter().any(|s| target.starts_with(s)) {
                    continue;
                }
                let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
                let linked = if file.is_empty() {
                    doc.clone()
                } else {
                    dir.join(file)
                };
                if !linked.exists() {
                    problems.push(format!(
                        "{}:{n}: link to {target} does not exist",
                        shown(&root, &doc)
                    ));
                    continue;
                }
                if anchor.is_empty() {
                    continue;
                }
                let headings = slugs
                    .entry(linked.clone())
                    .or_insert_with(|| heading_slugs(&read(&linked)));
                if !headings.contains(anchor) {
                    problems.push(format!(
                        "{}:{n}: {target} has no heading `{anchor}`",
                        shown(&root, &doc)
                    ));
                }
            }
        }
    }
    report(
        &problems,
        "Repoint the link, or restore the heading other documents link to.",
    );
}
