//! Walk every location under a home root into [`SkillRecord`]s. Read-only:
//! nothing here opens a file for writing.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::frontmatter;
use crate::locations::{Layout, Location, Provenance, Roots, LOCATIONS};
use crate::model::{ContentHash, Install, Mode, SkillName, SkillRecord, Source};

/// Deep enough for `cache/<marketplace>/<plugin>/<version>/skills`, with room.
const MAX_SEARCH_DEPTH: usize = 6;

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
}

/// Scan every location in [`LOCATIONS`] under `roots`. Missing locations and
/// unreadable entries are skipped.
#[must_use]
pub fn inventory(roots: &Roots) -> Inventory {
    let mut hashes: HashMap<PathBuf, ContentHash> = HashMap::new();
    let mut found = Vec::new();
    let mut elsewhere = BTreeMap::new();

    for location in LOCATIONS {
        let root = roots.dir(location);
        for skills_dir in skills_dirs(&root, location.layout) {
            for (name, path) in skill_folders(&skills_dir) {
                // A plugin cache keeps every plugin ever fetched. Until a
                // manifest says which are installed, none of them is loaded.
                let loaded = !location.agents.is_empty() && location.layout == Layout::Flat;
                if !loaded {
                    elsewhere.entry(name).or_insert(path);
                    continue;
                }
                let real_path = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                let hash = hashes
                    .entry(real_path.clone())
                    .or_insert_with(|| hash_dir(&real_path))
                    .clone();
                let mode = match fs::read_link(&path) {
                    Ok(target) => Mode::Symlink { target },
                    Err(_) => Mode::Copy,
                };
                let source = source(location, &real_path, &mode);
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
                });
            }
        }
    }

    Inventory {
        roots: roots.clone(),
        records: group(found),
        elsewhere,
    }
}

fn source(location: &Location, real_path: &Path, mode: &Mode) -> Source {
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
                let record = new_record(name, install, source);
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

fn new_record(name: SkillName, install: Install, source: Source) -> SkillRecord {
    let bytes = fs::read(install.real_path.join("SKILL.md")).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let (parsed, _body) = frontmatter::parse(&text);
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
    for (name, path) in children(dir) {
        // Not followed: a link here could loop or leave the cache entirely.
        let is_dir = fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
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
fn skill_folders(dir: &Path) -> Vec<(SkillName, PathBuf)> {
    children(dir)
        .into_iter()
        .filter(|(_, path)| path.join("SKILL.md").is_file())
        .collect()
}

/// Sorted, non-hidden, UTF-8-named entries of a directory.
fn children(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (!name.starts_with('.')).then(|| (name, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// blake3 over the folder's SKILL.md.
fn hash_dir(root: &Path) -> ContentHash {
    let bytes = fs::read(root.join("SKILL.md")).unwrap_or_default();
    ContentHash(blake3::hash(&bytes).to_hex().to_string())
}
