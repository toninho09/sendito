//! The collection: a directory tree of folders and request files.
//!
//! Paths handed out by this module are relative to the collection root and use
//! `/` separators, so they are stable across platforms and safe to store.

use std::fs;
use std::path::{Path, PathBuf};

use sendito_core::Request;
use sendito_core::inherit::ChainItem;
use sendito_core::model::FolderSettings;

pub(crate) use crate::fsio::make_private;
pub use crate::fsio::{write_atomic, write_private};
use crate::{Result, StoreError};
use crate::{examples, folder, request_file};

pub const META_DIR: &str = ".sendito";
pub const EXT: &str = "toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    /// Relative path ("" for the root).
    pub rel: String,
    pub folders: Vec<Folder>,
    pub requests: Vec<RequestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestEntry {
    pub name: String,
    /// Relative path including the `.toml` extension.
    pub rel: String,
    /// `GET`, `POST`, ..., or `GRPC`.
    pub badge: String,
    /// The request's `description`, read with the badge (empty: none).
    pub description: String,
}

impl Folder {
    pub fn count_requests(&self) -> usize {
        self.requests.len()
            + self
                .folders
                .iter()
                .map(Folder::count_requests)
                .sum::<usize>()
    }

    pub fn count_folders(&self) -> usize {
        self.folders.len()
            + self
                .folders
                .iter()
                .map(Folder::count_folders)
                .sum::<usize>()
    }

    pub fn find_folder(&self, rel: &str) -> Option<&Folder> {
        if self.rel == rel {
            return Some(self);
        }
        self.folders.iter().find_map(|f| f.find_folder(rel))
    }

    pub fn find_request(&self, rel: &str) -> Option<&RequestEntry> {
        self.requests
            .iter()
            .find(|r| r.rel == rel)
            .or_else(|| self.folders.iter().find_map(|f| f.find_request(rel)))
    }

    /// All request entries, depth first.
    pub fn all_requests(&self) -> Vec<&RequestEntry> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a RequestEntry>) {
        for f in &self.folders {
            f.collect(out);
        }
        out.extend(self.requests.iter());
    }
}

fn collect_rels(f: &Folder, out: &mut Vec<String>) {
    for r in &f.requests {
        out.push(r.rel.clone());
    }
    for sub in &f.folders {
        collect_rels(sub, out);
    }
}

/// Joins a relative collection path.
pub fn join_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Parent of a relative path ("" for top level).
pub fn parent_rel(rel: &str) -> &str {
    rel.rfind('/').map(|i| &rel[..i]).unwrap_or("")
}

/// Display name of a relative path (file stem for requests).
pub fn name_of(rel: &str) -> &str {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.strip_suffix(".toml").unwrap_or(base)
}

/// The folders whose settings apply to `rel` (a request, or a folder),
/// from the root (`""`) to its own folder.
pub fn folders_above(rel: &str) -> Vec<String> {
    let dir = if rel.ends_with(".toml") {
        parent_rel(rel)
    } else {
        rel
    };
    let mut out = vec![String::new()];
    let mut acc = String::new();
    for part in dir.split('/').filter(|p| !p.is_empty()) {
        acc = join_rel(&acc, part);
        out.push(acc.clone());
    }
    out
}

/// Checks a user-supplied request name: a folder name, and not the name of
/// the folder settings file.
pub fn validate_request_name(name: &str) -> Result<()> {
    validate_name(name)?;
    if format!("{name}.{EXT}").eq_ignore_ascii_case(folder::FILE) {
        return Err(StoreError::Invalid(format!(
            "invalid name {name:?}: {} holds the folder settings",
            folder::FILE
        )));
    }
    Ok(())
}

/// Checks a user-supplied folder name: a name, and not one that looks like
/// a request's examples directory.
pub fn validate_folder_name(name: &str) -> Result<()> {
    validate_name(name)?;
    if name.ends_with(examples::DIR_SUFFIX) {
        return Err(StoreError::Invalid(format!(
            "invalid folder name {name:?}: names ending in {} hold saved examples",
            examples::DIR_SUFFIX
        )));
    }
    Ok(())
}

/// Checks a user-supplied folder or request name.
pub fn validate_name(name: &str) -> Result<()> {
    let n = name.trim();
    let bad = n.is_empty()
        || n != name
        || n.starts_with('.')
        || n.chars()
            .any(|c| matches!(c, '/' | '\\' | '\0') || c.is_control())
        || n == ".."
        || n.len() > 200;
    if bad {
        return Err(StoreError::Invalid(format!(
            "invalid name {name:?}: names cannot be empty, start with '.', or contain '/', '\\' or control characters"
        )));
    }
    Ok(())
}

/// Checks a relative folder path (`""` is the root): every part must stay
/// inside the collection, so `..`, `.` and `\` are refused.
pub fn validate_rel(rel: &str) -> Result<()> {
    let bad = rel.split('/').filter(|p| !p.is_empty()).any(|p| {
        p == "."
            || p == ".."
            || p.chars()
                .any(|c| matches!(c, '\\' | '\0') || c.is_control())
    });
    if bad {
        return Err(StoreError::Invalid(format!(
            "invalid folder {rel:?}: a folder path cannot contain '.', '..' or '\\' parts"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Collection {
    pub root: PathBuf,
    pub ignore: Vec<String>,
}

impl Collection {
    pub fn new(root: impl Into<PathBuf>, ignore: Vec<String>) -> Self {
        Self {
            root: root.into(),
            ignore,
        }
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        let mut p = self.root.clone();
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p
    }

    pub fn meta_dir(&self) -> PathBuf {
        self.root.join(META_DIR)
    }

    /// Relative paths of every request under `rel` (`""` for the whole
    /// collection), in tree order: a folder's own requests first, then its
    /// subfolders, each in natural sort order.
    pub fn requests_under(&self, rel: &str) -> Result<Vec<String>> {
        let tree = self.scan()?;
        let start = if rel.is_empty() {
            Some(&tree)
        } else {
            tree.find_folder(rel)
        };
        let Some(start) = start else {
            return Err(StoreError::Invalid(format!(
                "folder \"{rel}\" does not exist in the collection"
            )));
        };
        let mut out = Vec::new();
        collect_rels(start, &mut out);
        Ok(out)
    }

    /// Scans the tree. Request badges are sniffed from the file head; hidden
    /// entries and ignored directory names are skipped.
    pub fn scan(&self) -> Result<Folder> {
        let name = self
            .root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "collection".into());
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        let mut root = self.scan_dir(&self.root, String::new(), name, &mut files)?;
        let heads = read_heads(&files);
        fill_heads(&mut root, &heads);
        prune(&mut root, true);
        Ok(root)
    }

    fn scan_dir(
        &self,
        dir: &Path,
        rel: String,
        name: String,
        files: &mut Vec<(String, PathBuf)>,
    ) -> Result<Folder> {
        let mut folder = Folder {
            name,
            rel: rel.clone(),
            folders: Vec::new(),
            requests: Vec::new(),
        };
        let rd = fs::read_dir(dir).map_err(|e| StoreError::io(dir, e))?;
        let mut entries: Vec<(String, PathBuf, bool)> = Vec::new();
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            if fname.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            let is_dir = if ft.is_symlink() {
                if links_to_ancestor(&path, dir) {
                    continue;
                }
                path.is_dir()
            } else {
                ft.is_dir()
            };
            entries.push((fname, path, is_dir));
        }
        entries.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        let mut has_other_files = false;
        for (fname, path, is_dir) in entries {
            if is_dir {
                // A request's saved examples, not a folder.
                if self.ignore.contains(&fname) || fname.ends_with(examples::DIR_SUFFIX) {
                    continue;
                }
                let child_rel = join_rel(&rel, &fname);
                folder
                    .folders
                    .push(self.scan_dir(&path, child_rel, fname, files)?);
            } else if fname == folder::FILE {
                // The folder settings, not a request.
            } else if path.extension().is_some_and(|e| e == EXT) {
                let child_rel = join_rel(&rel, &fname);
                files.push((child_rel.clone(), path));
                folder.requests.push(RequestEntry {
                    name: name_of(&fname).to_string(),
                    rel: child_rel,
                    badge: String::new(),
                    description: String::new(),
                });
            } else {
                has_other_files = true;
            }
        }
        if has_other_files && folder.requests.is_empty() && folder.folders.is_empty() {
            // Marks folders that only hold unrelated files so `prune` drops them.
            folder.name.insert(0, '\0');
        }
        Ok(folder)
    }

    pub fn load(&self, rel: &str) -> Result<Request> {
        let path = self.abs(rel);
        let text = fs::read_to_string(&path).map_err(|e| StoreError::io(&path, e))?;
        request_file::parse(&text).map_err(|msg| StoreError::Parse { path, msg })
    }

    /// Writes a request. When the file already holds an equal request it is left
    /// untouched, so saving an unchanged request never alters the bytes on disk.
    /// Returns whether the file was written.
    pub fn save(&self, rel: &str, req: &Request) -> Result<bool> {
        let path = self.abs(rel);
        if let Ok(existing) = fs::read_to_string(&path)
            && request_file::parse(&existing).ok().as_ref() == Some(req)
        {
            return Ok(false);
        }
        write_atomic(&path, request_file::to_toml(req).as_bytes())?;
        Ok(true)
    }

    fn folder_file(&self, rel: &str) -> PathBuf {
        self.abs(rel).join(folder::FILE)
    }

    /// The settings of a folder (`""` is the root); empty when it has no
    /// `folder.toml`.
    pub fn load_folder(&self, rel: &str) -> Result<FolderSettings> {
        let path = self.folder_file(rel);
        match fs::read_to_string(&path) {
            Ok(text) => folder::parse(&text).map_err(|msg| StoreError::Parse { path, msg }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FolderSettings::default()),
            Err(e) => Err(StoreError::io(&path, e)),
        }
    }

    /// Writes a folder's settings; empty settings remove its `folder.toml`.
    /// Equal settings leave the file untouched. Returns whether it changed.
    pub fn save_folder(&self, rel: &str, settings: &FolderSettings) -> Result<bool> {
        validate_rel(rel)?;
        if self.load_folder(rel).ok().as_ref() == Some(settings) {
            return Ok(false);
        }
        let path = self.folder_file(rel);
        if settings.is_empty() {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(StoreError::io(&path, e)),
            };
        }
        write_atomic(&path, folder::to_toml(settings).as_bytes())?;
        Ok(true)
    }

    /// The settings of every folder from the root to the folder that holds
    /// `request_rel`, root first, without the folders that define nothing. A
    /// `folder.toml` that cannot be read is an error: sending without the
    /// auth it defines would be wrong.
    pub fn chain(&self, request_rel: &str) -> Result<Vec<ChainItem>> {
        let mut out = Vec::new();
        for rel in folders_above(request_rel) {
            let f = self.load_folder(&rel)?;
            if !f.is_empty() {
                out.push((rel, f));
            }
        }
        Ok(out)
    }

    pub fn create_folder(&self, parent: &str, name: &str) -> Result<String> {
        validate_folder_name(name)?;
        validate_rel(parent)?;
        let rel = join_rel(parent, name);
        let p = self.abs(&rel);
        if p.exists() {
            return Err(StoreError::Exists(p));
        }
        fs::create_dir_all(&p).map_err(|e| StoreError::io(&p, e))?;
        Ok(rel)
    }

    pub fn create_request(&self, parent: &str, name: &str, req: &Request) -> Result<String> {
        validate_request_name(name)?;
        validate_rel(parent)?;
        let rel = join_rel(parent, &format!("{name}.{EXT}"));
        let p = self.abs(&rel);
        if p.exists() {
            return Err(StoreError::Exists(p));
        }
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
        }
        write_atomic(&p, request_file::to_toml(req).as_bytes())?;
        Ok(rel)
    }

    fn is_request_rel(rel: &str) -> bool {
        rel.ends_with(".toml")
    }

    /// Renames a folder or request in place. Returns the new relative path.
    pub fn rename(&self, rel: &str, new_name: &str) -> Result<String> {
        self.relocate(rel, parent_rel(rel), new_name)
    }

    /// Moves a node to the folder `new_parent` ("" is the root) with the
    /// name `new_name` (without `.toml`) in one step: when the target is
    /// taken or a name is not valid, nothing changes. Folders above the
    /// target that are missing are created. Returns the new relative path.
    pub fn relocate(&self, rel: &str, new_parent: &str, new_name: &str) -> Result<String> {
        let is_request = Self::is_request_rel(rel);
        if new_name != name_of(rel) {
            if is_request {
                validate_request_name(new_name)?;
            } else {
                validate_folder_name(new_name)?;
            }
        }
        validate_rel(new_parent)?;
        if !is_request && (new_parent == rel || new_parent.starts_with(&format!("{rel}/"))) {
            return Err(StoreError::Invalid(
                "cannot move a folder into itself".into(),
            ));
        }
        let fname = if is_request {
            format!("{new_name}.{EXT}")
        } else {
            new_name.to_string()
        };
        let new_rel = join_rel(new_parent, &fname);
        if new_rel == rel {
            return Ok(new_rel);
        }
        let (from, to) = (self.abs(rel), self.abs(&new_rel));
        // Case-only renames on case-insensitive file systems refer to the same file.
        if to.exists() && !rel.eq_ignore_ascii_case(&new_rel) {
            return Err(StoreError::Exists(to));
        }
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
        }
        fs::rename(&from, &to).map_err(|e| StoreError::io(&from, e))?;
        self.carry_examples(rel, &new_rel, false)?;
        Ok(new_rel)
    }

    /// Moves (or with `copy`, copies) a request's examples directory along
    /// with the request. Folders carry theirs inside them.
    fn carry_examples(&self, from_rel: &str, to_rel: &str, copy: bool) -> Result<()> {
        if !Self::is_request_rel(from_rel) {
            return Ok(());
        }
        let from = self.abs(&examples::dir_rel(from_rel));
        if !from.is_dir() {
            return Ok(());
        }
        let to = self.abs(&examples::dir_rel(to_rel));
        if copy {
            copy_dir(&from, &to)
        } else {
            fs::rename(&from, &to).map_err(|e| StoreError::io(&from, e))
        }
    }

    /// Moves a node into another folder ("" is the root).
    pub fn move_to(&self, rel: &str, new_parent: &str) -> Result<String> {
        self.relocate(rel, new_parent, name_of(rel))
    }

    /// Copies a node next to itself as "<name> copy", "<name> copy 2", ...
    pub fn duplicate(&self, rel: &str) -> Result<String> {
        let parent = parent_rel(rel);
        let name = name_of(rel);
        let is_req = Self::is_request_rel(rel);
        for i in 1..1000 {
            let candidate = if i == 1 {
                format!("{name} copy")
            } else {
                format!("{name} copy {i}")
            };
            let fname = if is_req {
                format!("{candidate}.{EXT}")
            } else {
                candidate
            };
            let new_rel = join_rel(parent, &fname);
            let to = self.abs(&new_rel);
            if to.exists() {
                continue;
            }
            let from = self.abs(rel);
            if is_req {
                fs::copy(&from, &to).map_err(|e| StoreError::io(&from, e))?;
                self.carry_examples(rel, &new_rel, true)?;
            } else {
                copy_dir(&from, &to)?;
            }
            return Ok(new_rel);
        }
        Err(StoreError::Invalid("too many copies".into()))
    }

    pub fn delete(&self, rel: &str) -> Result<()> {
        if rel.is_empty() {
            return Err(StoreError::Invalid(
                "cannot delete the collection root".into(),
            ));
        }
        let p = self.abs(rel);
        if p.is_dir() {
            fs::remove_dir_all(&p).map_err(|e| StoreError::io(&p, e))
        } else {
            fs::remove_file(&p).map_err(|e| StoreError::io(&p, e))?;
            let ex = self.abs(&examples::dir_rel(rel));
            if Self::is_request_rel(rel) && ex.is_dir() {
                fs::remove_dir_all(&ex).map_err(|e| StoreError::io(&ex, e))?;
            }
            Ok(())
        }
    }
}

/// True when the symlink `link` inside `dir` leads back to a folder already
/// on the path to `dir` (or above one). Each folder on the path is checked,
/// so two sibling folders that link to each other stop too. Following such
/// a link repeats the tree without end.
fn links_to_ancestor(link: &Path, dir: &Path) -> bool {
    let Ok(target) = fs::canonicalize(link) else {
        return false;
    };
    dir.ancestors()
        .any(|a| fs::canonicalize(a).is_ok_and(|c| c.starts_with(&target)))
}

#[cfg(unix)]
fn copy_link(src: &Path, dst: &Path) -> Result<()> {
    let target = fs::read_link(src).map_err(|e| StoreError::io(src, e))?;
    std::os::unix::fs::symlink(target, dst).map_err(|e| StoreError::io(dst, e))
}

/// Links need extra rights on Windows: they are left out of the copy.
#[cfg(not(unix))]
fn copy_link(_src: &Path, _dst: &Path) -> Result<()> {
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).map_err(|e| StoreError::io(to, e))?;
    for e in fs::read_dir(from)
        .map_err(|e| StoreError::io(from, e))?
        .flatten()
    {
        let src = e.path();
        let dst = to.join(e.file_name());
        // A link is copied as a link: following it could copy files from
        // outside the collection (or loop).
        if e.file_type().is_ok_and(|t| t.is_symlink()) {
            copy_link(&src, &dst)?;
            continue;
        }
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| StoreError::io(&src, e))?;
        }
    }
    Ok(())
}

/// What the scan reads from the top-level keys of a request file.
struct Head {
    /// `None`: the file is not a request.
    badge: Option<String>,
    description: String,
}

/// Reads the heads of many files, spreading the work over a few threads.
fn read_heads(files: &[(String, PathBuf)]) -> std::collections::HashMap<String, Head> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = files.len().div_ceil(threads).max(64);
    let mut out = std::collections::HashMap::with_capacity(files.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|(rel, p)| {
                            let text = read_head(p).unwrap_or_default();
                            let head = Head {
                                badge: request_file::sniff_badge(&text),
                                description: request_file::sniff_description(&text),
                            };
                            (rel.clone(), head)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            if let Ok(v) = h.join() {
                out.extend(v);
            }
        }
    });
    out
}

/// The top-level keys of a TOML file: the lines before the first table header.
/// A badge sits there, so the rest of a large request file is not read.
fn read_head(p: &Path) -> std::io::Result<String> {
    use std::io::BufRead;
    let mut head = String::new();
    let mut text_lines = request_file::TextLines::default();
    for line in std::io::BufReader::new(fs::File::open(p)?).lines() {
        let line = line?;
        // A line of a multi-line string (`[a](b)` in a description) is text.
        if !text_lines.in_text(&line) && line.trim_start().starts_with('[') {
            break;
        }
        head.push_str(&line);
        head.push('\n');
    }
    Ok(head)
}

/// Gives each request its badge and description; a file that is not a
/// request leaves the tree.
fn fill_heads(f: &mut Folder, heads: &std::collections::HashMap<String, Head>) {
    f.requests.retain_mut(|r| match heads.get(&r.rel) {
        Some(Head {
            badge: Some(b),
            description,
        }) => {
            r.badge = b.clone();
            r.description = description.clone();
            true
        }
        _ => false,
    });
    for c in &mut f.folders {
        fill_heads(c, heads);
    }
}

/// Drops folders that contain only non-request files (e.g. `src/` in a code repo).
/// Empty folders are kept: they are tree nodes the user created.
fn prune(f: &mut Folder, is_root: bool) -> bool {
    f.folders.retain_mut(|c| prune(c, false));
    let marked = f.name.starts_with('\0');
    if marked {
        f.name.remove(0);
    }
    let has_content = !f.requests.is_empty() || !f.folders.is_empty();
    is_root || has_content || !marked
}

/// Orders names so that "req 2" sorts before "req 10", case-insensitively.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let o = na
                    .trim_start_matches('0')
                    .len()
                    .cmp(&nb.trim_start_matches('0').len())
                    .then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::model::*;

    #[test]
    fn the_scan_reads_each_description_with_the_badge() {
        let (_d, c) = coll();
        fs::write(
            c.root.join("a.toml"),
            "kind = \"http\"\ndescription = '''\n[a](b)\n'''\nmethod = \"post\"\n[[headers]]\nname = \"X\"\nvalue = \"1\"\n",
        )
        .unwrap();
        fs::write(c.root.join("b.toml"), "kind = \"grpc\"\n").unwrap();
        let tree = c.scan().unwrap();
        let a = tree.find_request("a.toml").unwrap();
        assert_eq!(
            (a.badge.as_str(), a.description.as_str()),
            ("POST", "[a](b)\n")
        );
        assert_eq!(tree.find_request("b.toml").unwrap().description, "");
    }

    fn coll() -> (tempfile::TempDir, Collection) {
        let d = tempfile::tempdir().unwrap();
        let c = Collection::new(d.path(), vec!["target".into()]);
        (d, c)
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_an_ancestor_is_not_followed() {
        let (d, c) = coll();
        let users = c.create_folder("", "Users").unwrap();
        c.create_request(&users, "a", &Request::new_http()).unwrap();
        std::os::unix::fs::symlink("..", d.path().join("Users/up")).unwrap();
        std::os::unix::fs::symlink(".", d.path().join("self")).unwrap();
        let root = c.scan().unwrap();
        assert_eq!(root.folders.len(), 1);
        assert!(root.folders[0].folders.is_empty());
        // Two folders that link to each other.
        let admin = c.create_folder("", "Admin").unwrap();
        c.create_request(&admin, "b", &Request::new_http()).unwrap();
        std::os::unix::fs::symlink("../Admin", d.path().join("Users/to-admin")).unwrap();
        std::os::unix::fs::symlink("../Users", d.path().join("Admin/to-users")).unwrap();
        let root = c.scan().unwrap();
        assert_eq!(root.folders.len(), 2);
        // A duplicate keeps links as links: nothing behind them is copied.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("id_key"), "private").unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("Users/keys")).unwrap();
        let copy = c.duplicate(&users).unwrap();
        let keys = d.path().join(&copy).join("keys");
        assert!(
            fs::symlink_metadata(&keys)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn requests_under_lists_in_tree_order() {
        let (_d, c) = coll();
        c.create_request("", "ping", &Request::new_grpc()).unwrap();
        let users = c.create_folder("", "Users").unwrap();
        c.create_request(&users, "zeta", &Request::new_http())
            .unwrap();
        c.create_request(&users, "alpha", &Request::new_http())
            .unwrap();
        let admin = c.create_folder(&users, "Admin").unwrap();
        c.create_request(&admin, "ban", &Request::new_http())
            .unwrap();

        // A folder's own requests (natural sort) before its subfolders.
        assert_eq!(
            c.requests_under(&users).unwrap(),
            vec![
                "Users/alpha.toml",
                "Users/zeta.toml",
                "Users/Admin/ban.toml"
            ]
        );
        assert_eq!(
            c.requests_under("Users/Admin").unwrap(),
            vec!["Users/Admin/ban.toml"]
        );
        // "" runs the whole collection, root requests first.
        assert_eq!(
            c.requests_under("").unwrap(),
            vec![
                "ping.toml",
                "Users/alpha.toml",
                "Users/zeta.toml",
                "Users/Admin/ban.toml"
            ]
        );
        let e = c.requests_under("nope").unwrap_err().to_string();
        assert!(e.contains("nope"), "{e}");
    }

    #[test]
    fn create_scan_ops() {
        let (_d, c) = coll();
        let f = c.create_folder("", "Users").unwrap();
        assert_eq!(f, "Users");
        let r = c
            .create_request(
                &f,
                "list users",
                &Request::Http(HttpRequest {
                    method: "PUT".into(),
                    ..Default::default()
                }),
            )
            .unwrap();
        assert_eq!(r, "Users/list users.toml");
        c.create_request("", "ping", &Request::new_grpc()).unwrap();
        c.create_folder("", "empty").unwrap();
        fs::create_dir_all(c.abs("src")).unwrap();
        fs::write(c.abs("src/main.rs"), "fn main(){}").unwrap();
        fs::create_dir_all(c.abs("target/x")).unwrap();
        fs::write(c.abs("Cargo.toml"), "[package]\nname='x'").unwrap();
        fs::create_dir_all(c.abs(".sendito")).unwrap();
        fs::write(c.abs(".sendito/config.toml"), "kind = \"http\"").unwrap();

        let tree = c.scan().unwrap();
        let names: Vec<_> = tree.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["empty", "Users"]);
        assert_eq!(tree.requests.len(), 1);
        assert_eq!(tree.requests[0].badge, "GRPC");
        assert_eq!(tree.count_requests(), 2);
        assert_eq!(tree.count_folders(), 2);
        assert_eq!(tree.find_request(&r).map(|e| e.badge.as_str()), Some("PUT"));
        assert!(tree.find_folder("Users").is_some());
        assert_eq!(tree.all_requests().len(), 2);

        assert!(c.create_folder("", "Users").is_err());
        assert!(
            c.create_request(&f, "list users", &Request::new_http())
                .is_err()
        );

        let r2 = c.rename(&r, "all users").unwrap();
        assert_eq!(r2, "Users/all users.toml");
        assert_eq!(c.rename(&r2, "all users").unwrap(), r2);
        let dup = c.duplicate(&r2).unwrap();
        assert_eq!(dup, "Users/all users copy.toml");
        let dup2 = c.duplicate(&r2).unwrap();
        assert_eq!(dup2, "Users/all users copy 2.toml");
        let moved = c.move_to(&dup2, "empty").unwrap();
        assert_eq!(moved, "empty/all users copy 2.toml");
        assert_eq!(c.move_to(&moved, "empty").unwrap(), moved);
        assert!(c.move_to("Users", "Users/sub").is_err());
        let fdup = c.duplicate("Users").unwrap();
        assert_eq!(fdup, "Users copy");
        assert_eq!(
            c.scan()
                .unwrap()
                .find_folder("Users copy")
                .map(|f| f.requests.len()),
            Some(2)
        );
        let f2 = c.rename("Users copy", "Clients").unwrap();
        let moved_folder = c.move_to(&f2, "empty").unwrap();
        assert_eq!(moved_folder, "empty/Clients");
        c.delete(&moved_folder).unwrap();
        c.delete(&moved).unwrap();
        assert!(c.delete("").is_err());
        assert!(c.rename(&dup, "all users").is_err());
        assert_eq!(c.scan().unwrap().count_requests(), 3);
    }

    #[test]
    fn folder_settings_chain_and_scan() {
        let (_d, c) = coll();
        c.create_folder("", "Users").unwrap();
        c.create_request("Users", "list", &Request::new_http())
            .unwrap();
        let root = FolderSettings {
            auth: Auth::Bearer {
                token: "{{t}}".into(),
            },
            ..Default::default()
        };
        let users = FolderSettings {
            headers: vec![KeyValue::new("X-A", "1")],
            ..Default::default()
        };
        assert!(c.save_folder("", &root).unwrap());
        assert!(!c.save_folder("", &root).unwrap());
        c.save_folder("Users", &users).unwrap();
        let chain = c.chain("Users/list.toml").unwrap();
        assert_eq!(
            chain,
            vec![(String::new(), root.clone()), ("Users".into(), users)]
        );
        assert_eq!(c.chain("Users").unwrap().len(), 2);
        // folder.toml is not a request in the tree.
        let tree = c.scan().unwrap();
        assert_eq!(tree.count_requests(), 1);
        // Empty settings remove the file.
        assert!(c.save_folder("Users", &FolderSettings::default()).unwrap());
        assert!(!c.abs("Users/folder.toml").exists());
        assert_eq!(
            c.chain("Users/list.toml").unwrap(),
            vec![(String::new(), root)]
        );
        // A broken folder.toml stops the chain with its path.
        fs::write(c.abs("Users/folder.toml"), "kind = \"http\"\n").unwrap();
        let e = c.chain("Users/list.toml").unwrap_err().to_string();
        assert!(
            e.contains("folder.toml") && e.contains("request file"),
            "{e}"
        );
    }

    #[test]
    fn examples_follow_their_request() {
        let (_d, c) = coll();
        c.create_folder("", "A").unwrap();
        let rel = c.create_request("", "login", &Request::new_http()).unwrap();
        let ex = examples::Example {
            name: "200".into(),
            status: 200,
            ..Default::default()
        };
        examples::save(&c, &rel, &ex).unwrap();
        // Not a folder in the tree.
        assert_eq!(c.scan().unwrap().count_folders(), 1);
        assert!(c.create_folder("", "x.examples").is_err());
        let rel = c.rename(&rel, "sign in").unwrap();
        assert_eq!(examples::list(&c, &rel).unwrap(), ["200"]);
        let rel = c.move_to(&rel, "A").unwrap();
        assert_eq!(examples::list(&c, &rel).unwrap(), ["200"]);
        let copy = c.duplicate(&rel).unwrap();
        assert_eq!(examples::list(&c, &copy).unwrap(), ["200"]);
        c.delete(&rel).unwrap();
        assert!(!c.abs("A/sign in.examples").exists());
        assert!(c.abs("A/sign in copy.examples").is_dir());
    }

    #[test]
    fn a_request_cannot_be_named_folder() {
        let (_d, c) = coll();
        assert!(
            c.create_request("", "folder", &Request::new_http())
                .is_err()
        );
        let rel = c.create_request("", "x", &Request::new_http()).unwrap();
        assert!(c.rename(&rel, "Folder").is_err());
        assert!(c.create_folder("", "folder").is_ok());
    }

    #[test]
    fn save_is_stable() {
        let (_d, c) = coll();
        let req = Request::Http(HttpRequest {
            url: "http://x".into(),
            ..Default::default()
        });
        let rel = c.create_request("", "a", &req).unwrap();
        // Hand-formatted but equivalent content is left untouched.
        let custom = "# my comment\nkind = 'http'\nmethod = 'GET'\nurl = 'http://x'\n";
        fs::write(c.abs(&rel), custom).unwrap();
        assert!(!c.save(&rel, &req).unwrap());
        assert_eq!(fs::read_to_string(c.abs(&rel)).unwrap(), custom);
        let mut changed = req.clone();
        if let Request::Http(h) = &mut changed {
            h.url = "http://y".into();
        }
        assert!(c.save(&rel, &changed).unwrap());
        assert_eq!(c.load(&rel).unwrap(), changed);
        fs::write(c.abs(&rel), "kind = 1").unwrap();
        assert!(matches!(c.load(&rel), Err(StoreError::Parse { .. })));
        assert!(c.load("nope.toml").is_err());
    }

    #[test]
    fn names() {
        assert!(validate_name("ok name").is_ok());
        for bad in ["", " x", ".hidden", "a/b", "a\\b", "x\u{1}", ".."] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        for ok in ["", "a", "a/b c", "a/.config-like name/b"] {
            assert!(validate_rel(ok).is_ok(), "{ok}");
        }
        for bad in ["..", "a/../..", "./a", "a\\b"] {
            assert!(validate_rel(bad).is_err(), "{bad}");
        }
        assert_eq!(join_rel("", "a"), "a");
        assert_eq!(join_rel("a", "b"), "a/b");
        assert_eq!(parent_rel("a/b/c.toml"), "a/b");
        assert_eq!(parent_rel("c.toml"), "");
        assert_eq!(name_of("a/b/c.toml"), "c");
        assert_eq!(name_of("a/b"), "b");
        let mut v = vec!["req 10", "Req 2", "req 1", "alpha", "req 02"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["alpha", "req 1", "Req 2", "req 02", "req 10"]);
    }
}
