//! Walk every location under a home root into [`SkillRecord`]s. Read-only:
//! nothing here opens a file for writing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::frontmatter;
use crate::locations::{Layout, Location, Provenance, Roots, LOCATIONS};
use crate::lockfiles::{self, Lockfiles};
use crate::model::{ContentHash, Install, Mode, SkillName, SkillRecord, Source};

/// Deep enough for `cache/<marketplace>/<plugin>/<version>/skills`, with room.
const MAX_SEARCH_DEPTH: usize = 6;
/// Bytes of each file read for hashing and for SKILL.md. A file's full length
/// is hashed too, so growth past the cap still changes the hash.
pub const MAX_READ_BYTES: u64 = 1 << 20;

/// Everything found under one set of roots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    pub roots: Roots,
    /// Sorted by name, then by first install location.
    pub records: Vec<SkillRecord>,
    /// Skills that exist on the machine but no agent loads: vendored copies
    /// and cached plugins nothing installed or enabled.
    pub elsewhere: BTreeMap<SkillName, PathBuf>,
}

struct Found {
    name: SkillName,
    install: Install,
    source: Source,
    skill_md: Option<Vec<u8>>,
}

/// Scan every location in [`LOCATIONS`] under `roots`. Missing locations and
/// unreadable entries are skipped.
#[must_use]
pub fn inventory(roots: &Roots) -> Inventory {
    let locks = lockfiles::read(roots);
    let agents_dir = fs::canonicalize(roots.home.join(".agents/skills")).ok();
    let mut hashes: HashMap<PathBuf, (ContentHash, Option<Vec<u8>>)> = HashMap::new();
    let mut found = Vec::new();
    let mut elsewhere = BTreeMap::new();

    for location in LOCATIONS {
        let root = roots.dir(location);
        for skills_dir in skills_dirs(&root, location.layout) {
            let canonical_skills_dir =
                fs::canonicalize(&skills_dir).unwrap_or_else(|_| skills_dir.clone());
            for (name, path, file_type) in skill_folders(&skills_dir) {
                let relative = path.strip_prefix(&root).unwrap_or(&path);
                let plugin = plugin_source(location.provenance, relative, &locks);
                let loaded = !location.agents.is_empty()
                    && (location.layout == Layout::Flat || plugin.is_some());
                if !loaded {
                    elsewhere.entry(name).or_insert(path);
                    continue;
                }
                let is_symlink = file_type.as_ref().is_some_and(|ft| ft.is_symlink());
                let real_path = if is_symlink {
                    fs::canonicalize(&path).unwrap_or_else(|_| path.clone())
                } else {
                    canonical_skills_dir.join(&name)
                };
                let (hash, skill_md) = hashes
                    .entry(real_path.clone())
                    .or_insert_with(|| hash_dir(&real_path))
                    .clone();
                let mode = if is_symlink {
                    match fs::read_link(&path) {
                        Ok(target) => Mode::Symlink { target },
                        Err(_) => Mode::Copy,
                    }
                } else {
                    Mode::Copy
                };
                let source = source(
                    location,
                    &name,
                    &real_path,
                    &mode,
                    plugin,
                    &locks,
                    agents_dir.as_deref(),
                );
                let install = Install {
                    location: location.id,
                    agents: location.agents.to_vec(),
                    path,
                    real_path,
                    mode,
                    hash,
                };
                found.push(Found {
                    name,
                    install,
                    source,
                    skill_md,
                });
            }
        }
    }

    let mut records = group(found);
    let known: HashSet<String> = records
        .iter()
        .map(|r| r.name.clone())
        .chain(elsewhere.keys().cloned())
        .collect();
    for record in &mut records {
        record
            .refs
            .retain(|r| known.contains(r) && *r != record.name);
    }

    Inventory {
        roots: roots.clone(),
        records,
        elsewhere,
    }
}

fn plugin_source(provenance: Provenance, relative: &Path, locks: &Lockfiles) -> Option<Source> {
    match provenance {
        Provenance::ClaudePlugins => lockfiles::claude_plugin(relative, &locks.claude_plugins),
        Provenance::CodexPlugins => lockfiles::codex_plugin(relative, &locks.codex_enabled),
        Provenance::Lockfiles | Provenance::Builtin(_) | Provenance::None => None,
    }
}

fn source(
    location: &Location,
    name: &str,
    real_path: &Path,
    mode: &Mode,
    plugin: Option<Source>,
    locks: &Lockfiles,
    agents_dir: Option<&Path>,
) -> Source {
    // The skills CLI installs into ~/.agents/skills and links other agents'
    // folders to it, so a lock entry covers any install that resolves there.
    let in_agents_dir = agents_dir.is_some_and(|dir| real_path.parent() == Some(dir));
    if let Some(tracked) = locks.tracked.get(name).filter(|_| in_agents_dir) {
        return tracked.clone();
    }
    if let Some(plugin) = plugin {
        return plugin;
    }
    if let Provenance::Builtin(agent) = location.provenance {
        return Source::Builtin { agent };
    }
    match mode {
        Mode::Symlink { .. } => Source::Local {
            path: real_path.to_path_buf(),
        },
        Mode::Copy => Source::Unknown,
    }
}

/// One record per (name, content hash), installs in location order.
fn group(found: Vec<Found>) -> Vec<SkillRecord> {
    let mut by_key: BTreeMap<(SkillName, ContentHash), SkillRecord> = BTreeMap::new();
    for Found {
        name,
        install,
        source,
        skill_md,
    } in found
    {
        let key = (name.clone(), install.hash.clone());
        match by_key.get_mut(&key) {
            Some(record) => {
                if source.specificity() > record.source.specificity() {
                    record.source = source;
                }
                record.installs.push(install);
            }
            None => {
                let record = new_record(name, install, source, skill_md);
                by_key.insert(key, record);
            }
        }
    }
    let mut records: Vec<SkillRecord> = by_key.into_values().collect();
    for record in &mut records {
        record
            .installs
            .sort_by_key(|i| (crate::locations::order(i.location), i.path.clone()));
    }
    records.sort_by(|a, b| {
        let first = |r: &SkillRecord| {
            r.installs
                .first()
                .map(|i| (crate::locations::order(i.location), i.path.clone()))
        };
        a.name.cmp(&b.name).then_with(|| first(a).cmp(&first(b)))
    });
    records
}

fn new_record(
    name: SkillName,
    install: Install,
    source: Source,
    skill_md: Option<Vec<u8>>,
) -> SkillRecord {
    let bytes = skill_md
        .or_else(|| {
            read_capped(&install.real_path.join("SKILL.md"))
                .map(|(_, bytes)| bytes)
                .ok()
        })
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let (parsed, body) = frontmatter::parse(&text);
    let refs = frontmatter::mentions(body);
    let lines = text.lines().count();
    let (declared_name, description, frontmatter_error) = match parsed {
        Ok(fm) => (Some(fm.name), Some(fm.description), None),
        Err(reason) => (None, None, Some(format!("frontmatter {reason}"))),
    };
    SkillRecord {
        name,
        hash: install.hash.clone(),
        source,
        installs: vec![install],
        refs,
        declared_name,
        description,
        frontmatter_error,
        lines,
    }
}

/// The directories whose children are skill folders.
fn skills_dirs(root: &Path, layout: Layout) -> Vec<PathBuf> {
    match layout {
        Layout::Flat if root.is_dir() => vec![root.to_path_buf()],
        Layout::Flat => Vec::new(),
        Layout::Nested => {
            let mut out = Vec::new();
            find_skills_dirs(root, 0, &mut out);
            out
        }
    }
}

fn find_skills_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_SEARCH_DEPTH {
        return;
    }
    for (name, path, file_type) in children(dir).unwrap_or_default() {
        // Not followed: a link here could loop or leave the cache entirely.
        let is_dir = file_type
            .map(|ft| ft.is_dir())
            .unwrap_or_else(|| fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()));
        if !is_dir {
            continue;
        }
        if name == "skills" {
            out.push(path);
        } else {
            find_skills_dirs(&path, depth + 1, out);
        }
    }
}

/// Child folders holding a SKILL.md, by name. Symlinks are followed.
fn skill_folders(dir: &Path) -> Vec<(SkillName, PathBuf, Option<fs::FileType>)> {
    children(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, path, _)| path.join("SKILL.md").is_file())
        .collect()
}

/// Sorted, non-hidden, UTF-8-named entries of a directory with file types.
fn children(dir: &Path) -> io::Result<Vec<(String, PathBuf, Option<fs::FileType>)>> {
    let mut out: Vec<(String, PathBuf, Option<fs::FileType>)> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.')).then(|| (name, e.path(), e.file_type().ok()))
        })
        .collect();
    out.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
    Ok(out)
}

/// A file's full length and at most [`MAX_READ_BYTES`] of its bytes.
fn read_capped(path: &Path) -> io::Result<(u64, Vec<u8>)> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let cap = len.min(MAX_READ_BYTES) as usize;
    let mut bytes = Vec::with_capacity(cap);
    file.take(MAX_READ_BYTES).read_to_end(&mut bytes)?;
    Ok((len, bytes))
}

/// One entry [`hash_dir`] covers.
enum Entry {
    /// A regular file inside the skill folder.
    File,
    /// A link that leaves the skill folder: its target text is hashed, never
    /// what it points at.
    Outside(PathBuf),
    /// A directory already walked under another path, by its path relative to
    /// the skill folder. Not walked again, so a link back up costs one entry.
    Walked(PathBuf),
    /// A directory whose entries could not be listed.
    Unlistable,
}

/// blake3 over every non-hidden file below `root`: relative path, full
/// length, and the first [`MAX_READ_BYTES`] bytes, in sorted order. Hidden
/// files (`.DS_Store`, editor state) are not content an agent reads. Links are
/// followed only while they stay inside `root`, which is already resolved, and
/// each real directory is walked once. Returns the content hash and any
/// `SKILL.md` bytes read at the root.
fn hash_dir(root: &Path) -> (ContentHash, Option<Vec<u8>>) {
    let mut files = Vec::new();
    let mut walked = HashSet::from([root.to_path_buf()]);
    collect_files(root, root, "", &mut walked, &mut files);
    let mut hasher = blake3::Hasher::new();
    let mut skill_md = None;
    for (relative, entry) in files {
        hasher.update(relative.as_bytes());
        hasher.update(&[0]);
        match entry {
            Entry::Outside(target) => {
                hasher.update(b"\xfelink");
                hasher.update(target.as_os_str().as_encoded_bytes());
            }
            Entry::Walked(real) => {
                hasher.update(b"\xfewalked");
                hasher.update(real.as_os_str().as_encoded_bytes());
            }
            Entry::Unlistable => {
                hasher.update(b"\xffunlistable");
            }
            Entry::File => match read_capped(&root.join(&relative)) {
                Ok((len, bytes)) => {
                    hasher.update(&len.to_le_bytes());
                    hasher.update(&bytes);
                    if relative == "SKILL.md" {
                        skill_md = Some(bytes);
                    }
                }
                Err(_) => {
                    hasher.update(b"\xffunreadable");
                }
            },
        }
    }
    (
        ContentHash(hasher.finalize().to_hex().to_string()),
        skill_md,
    )
}

fn collect_files(
    root: &Path,
    dir: &Path,
    prefix: &str,
    walked: &mut HashSet<PathBuf>,
    out: &mut Vec<(String, Entry)>,
) {
    let Ok(children) = children(dir) else {
        out.push((prefix.to_string(), Entry::Unlistable));
        return;
    };
    for (name, path, file_type) in children {
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        // A linked top-level SKILL.md is still the file the agent reads.
        let entry_point = relative == "SKILL.md";
        let is_symlink = file_type.as_ref().is_some_and(|ft| ft.is_symlink());
        if is_symlink && !entry_point {
            if let Ok(target) = fs::read_link(&path) {
                let inside = fs::canonicalize(&path).is_ok_and(|real| real.starts_with(root));
                if !inside {
                    out.push((relative, Entry::Outside(target)));
                    continue;
                }
            }
        }
        if file_type.as_ref().is_some_and(|ft| ft.is_file()) {
            out.push((relative, Entry::File));
        } else if file_type.as_ref().is_some_and(|ft| ft.is_dir()) {
            match fs::canonicalize(&path) {
                Ok(real) if walked.insert(real.clone()) => {
                    collect_files(root, &path, &relative, walked, out);
                }
                Ok(real) => {
                    let inner = real.strip_prefix(root).unwrap_or(&real).to_path_buf();
                    out.push((relative, Entry::Walked(inner)));
                }
                Err(_) => out.push((relative, Entry::Unlistable)),
            }
        } else {
            // Symlink or unqueried file type: fall back to fs::metadata
            match fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => match fs::canonicalize(&path) {
                    Ok(real) if walked.insert(real.clone()) => {
                        collect_files(root, &path, &relative, walked, out);
                    }
                    Ok(real) => {
                        let inner = real.strip_prefix(root).unwrap_or(&real).to_path_buf();
                        out.push((relative, Entry::Walked(inner)));
                    }
                    Err(_) => out.push((relative, Entry::Unlistable)),
                },
                Ok(meta) if meta.is_file() => out.push((relative, Entry::File)),
                _ => {}
            }
        }
    }
}
