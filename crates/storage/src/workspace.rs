//! Workspace snapshots.
//!
//! A workspace snapshot is a deterministic, self-contained archive of a
//! directory tree (files, directories, symlinks, permission bits). The
//! collector captures one at every span boundary of a run that declares a
//! `workspace_root`, and the replay engine restores them into a private
//! sandbox so that forked replays never touch (or see side effects in) the
//! live working tree.
//!
//! Archives are byte-for-byte deterministic for a given tree (entries are
//! sorted, no timestamps are stored), so identical workspace states share a
//! single content-addressed blob in the blob store.
//!
//! Format (`RKWS1`):
//!
//! ```text
//! b"RKWS1\n" | u64 LE manifest length | manifest JSON | concatenated file bytes
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use replaykit_core_model::{
    ArtifactId, ArtifactRecord, ArtifactType, Document, IdKind, RunId, SnapshotId, SnapshotRecord,
    SpanId, Value,
};
use serde::{Deserialize, Serialize};

use crate::{Storage, StorageError, sha256_hex};

/// `SnapshotRecord::kind` used for workspace snapshots.
pub const WORKSPACE_SNAPSHOT_KIND: &str = "workspace";
/// MIME type of workspace archive artifacts.
pub const WORKSPACE_ARCHIVE_MIME: &str = "application/x-replaykit-workspace";
/// Default cap on the total file bytes captured in one snapshot (256 MiB).
pub const DEFAULT_MAX_WORKSPACE_BYTES: u64 = 256 * 1024 * 1024;

const MAGIC: &[u8] = b"RKWS1\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// Relative, `/`-separated path.
    pub path: String,
    pub kind: EntryKind,
    /// Unix permission bits (`0o777` mask).
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default)]
    pub len: u64,
    #[serde(default)]
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl WorkspaceEntry {
    fn same_content(&self, other: &WorkspaceEntry) -> bool {
        self.kind == other.kind
            && self.mode == other.mode
            && self.sha256 == other.sha256
            && self.target == other.target
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    entries: Vec<WorkspaceEntry>,
}

/// Options for [`WorkspaceArchive::capture_with`].
#[derive(Clone, Debug)]
pub struct CaptureOptions {
    /// Refuse to capture trees whose file bytes exceed this many bytes.
    pub max_bytes: u64,
    /// Absolute paths that are never captured (for example a ReplayKit data
    /// root that lives inside the workspace).
    pub exclude: Vec<PathBuf>,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_WORKSPACE_BYTES,
            exclude: Vec::new(),
        }
    }
}

/// How one path differs between two archives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceChange {
    pub path: String,
    pub kind: ChangeKind,
    pub entry_kind: EntryKind,
}

/// An in-memory workspace archive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceArchive {
    entries: Vec<WorkspaceEntry>,
    data: Vec<u8>,
}

fn io_err(context: impl std::fmt::Display, err: io::Error) -> StorageError {
    StorageError::Internal(format!("{context}: {err}"))
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(meta: &fs::Metadata) -> u32 {
    if meta.is_dir() {
        0o755
    } else if meta.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| io_err(format!("set permissions on {}", path.display()), e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &Path) -> Result<(), StorageError> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| io_err(format!("create symlink {}", link.display()), e))
}

#[cfg(not(unix))]
fn make_symlink(target: &str, link: &Path) -> Result<(), StorageError> {
    fs::write(link, target.as_bytes())
        .map_err(|e| io_err(format!("write symlink stand-in {}", link.display()), e))
}

/// Reject absolute paths and `..` so an archive can never write outside the
/// directory it is restored into.
fn safe_relative(path: &str) -> Result<PathBuf, StorageError> {
    let candidate = Path::new(path);
    let mut out = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => {
                return Err(StorageError::InvalidInput(format!(
                    "workspace archive entry {path:?} escapes the workspace"
                )));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(StorageError::InvalidInput(
            "workspace archive entry has an empty path".into(),
        ));
    }
    Ok(out)
}

impl WorkspaceArchive {
    /// Capture `root` with default options.
    pub fn capture(root: &Path) -> Result<Self, StorageError> {
        Self::capture_with(root, &CaptureOptions::default())
    }

    /// Capture every file, directory and symlink under `root`.
    pub fn capture_with(root: &Path, options: &CaptureOptions) -> Result<Self, StorageError> {
        let meta = fs::metadata(root)
            .map_err(|e| io_err(format!("workspace root {}", root.display()), e))?;
        if !meta.is_dir() {
            return Err(StorageError::InvalidInput(format!(
                "workspace root {} is not a directory",
                root.display()
            )));
        }
        let exclude = options
            .exclude
            .iter()
            .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            .collect::<Vec<_>>();
        let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let mut archive = WorkspaceArchive::default();
        archive.walk(&canonical_root, "", &exclude, options.max_bytes)?;
        Ok(archive)
    }

    fn walk(
        &mut self,
        dir: &Path,
        prefix: &str,
        exclude: &[PathBuf],
        max_bytes: u64,
    ) -> Result<(), StorageError> {
        let mut children = fs::read_dir(dir)
            .map_err(|e| io_err(format!("read dir {}", dir.display()), e))?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| io_err(format!("read dir {}", dir.display()), e))?;
        children.sort();
        for name in children {
            let Some(name_str) = name.to_str() else {
                return Err(StorageError::InvalidInput(format!(
                    "non-UTF-8 file name under {}",
                    dir.display()
                )));
            };
            let full = dir.join(&name);
            if exclude.iter().any(|ex| ex == &full) {
                continue;
            }
            let rel = if prefix.is_empty() {
                name_str.to_owned()
            } else {
                format!("{prefix}/{name_str}")
            };
            let meta = fs::symlink_metadata(&full)
                .map_err(|e| io_err(format!("stat {}", full.display()), e))?;
            let file_type = meta.file_type();
            if file_type.is_symlink() {
                let target = fs::read_link(&full)
                    .map_err(|e| io_err(format!("read link {}", full.display()), e))?;
                self.entries.push(WorkspaceEntry {
                    path: rel,
                    kind: EntryKind::Symlink,
                    mode: 0o777,
                    sha256: None,
                    len: 0,
                    offset: 0,
                    target: Some(target.to_string_lossy().into_owned()),
                });
            } else if file_type.is_dir() {
                self.entries.push(WorkspaceEntry {
                    path: rel.clone(),
                    kind: EntryKind::Dir,
                    mode: mode_of(&meta),
                    sha256: None,
                    len: 0,
                    offset: 0,
                    target: None,
                });
                self.walk(&full, &rel, exclude, max_bytes)?;
            } else if file_type.is_file() {
                let bytes =
                    fs::read(&full).map_err(|e| io_err(format!("read {}", full.display()), e))?;
                if self.data.len() as u64 + bytes.len() as u64 > max_bytes {
                    return Err(StorageError::InvalidInput(format!(
                        "workspace exceeds the snapshot limit of {max_bytes} bytes"
                    )));
                }
                let offset = self.data.len() as u64;
                self.entries.push(WorkspaceEntry {
                    path: rel,
                    kind: EntryKind::File,
                    mode: mode_of(&meta),
                    sha256: Some(sha256_hex(&bytes)),
                    len: bytes.len() as u64,
                    offset,
                    target: None,
                });
                self.data.extend_from_slice(&bytes);
            }
            // Sockets, FIFOs and devices are not part of a workspace.
        }
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let manifest = Manifest {
            version: 1,
            entries: self.entries.clone(),
        };
        let manifest_json = serde_json::to_vec(&manifest).expect("manifest serializes");
        let mut out = Vec::with_capacity(MAGIC.len() + 8 + manifest_json.len() + self.data.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(manifest_json.len() as u64).to_le_bytes());
        out.extend_from_slice(&manifest_json);
        out.extend_from_slice(&self.data);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StorageError> {
        let corrupt =
            |why: &str| StorageError::InvalidInput(format!("bad workspace archive: {why}"));
        if bytes.len() < MAGIC.len() + 8 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(corrupt("missing RKWS1 header"));
        }
        let mut len_bytes = [0u8; 8];
        len_bytes.copy_from_slice(&bytes[MAGIC.len()..MAGIC.len() + 8]);
        let manifest_len = u64::from_le_bytes(len_bytes) as usize;
        let manifest_start = MAGIC.len() + 8;
        let data_start = manifest_start
            .checked_add(manifest_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| corrupt("manifest length out of range"))?;
        let manifest: Manifest = serde_json::from_slice(&bytes[manifest_start..data_start])
            .map_err(|e| corrupt(&e.to_string()))?;
        let data = bytes[data_start..].to_vec();
        for entry in &manifest.entries {
            safe_relative(&entry.path)?;
            if entry.kind == EntryKind::File && entry.offset + entry.len > data.len() as u64 {
                return Err(corrupt("file entry out of range"));
            }
        }
        Ok(Self {
            entries: manifest.entries,
            data,
        })
    }

    pub fn entries(&self) -> &[WorkspaceEntry] {
        &self.entries
    }

    pub fn entry(&self, path: &str) -> Option<&WorkspaceEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    /// Bytes of a regular file in the archive.
    pub fn file_content(&self, path: &str) -> Option<&[u8]> {
        self.entry(path)
            .filter(|entry| entry.kind == EntryKind::File)
            .map(|entry| self.slice(entry))
    }

    fn slice(&self, entry: &WorkspaceEntry) -> &[u8] {
        &self.data[entry.offset as usize..(entry.offset + entry.len) as usize]
    }

    pub fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count()
    }

    pub fn total_bytes(&self) -> u64 {
        self.data.len() as u64
    }

    /// Materialize the archive into `dest`, which must not exist or be empty.
    pub fn restore(&self, dest: &Path) -> Result<(), StorageError> {
        if dest.exists() {
            let mut entries =
                fs::read_dir(dest).map_err(|e| io_err(format!("read {}", dest.display()), e))?;
            if entries.next().is_some() {
                return Err(StorageError::InvalidInput(format!(
                    "restore target {} is not empty",
                    dest.display()
                )));
            }
        }
        fs::create_dir_all(dest).map_err(|e| io_err(format!("create {}", dest.display()), e))?;
        let mut dirs = Vec::new();
        for entry in &self.entries {
            self.write_entry(dest, entry)?;
            if entry.kind == EntryKind::Dir {
                dirs.push(entry);
            }
        }
        // Apply directory modes last so read-only directories can be filled.
        for entry in dirs.into_iter().rev() {
            set_mode(&dest.join(safe_relative(&entry.path)?), entry.mode)?;
        }
        Ok(())
    }

    fn write_entry(&self, dest: &Path, entry: &WorkspaceEntry) -> Result<(), StorageError> {
        let path = dest.join(safe_relative(&entry.path)?);
        match entry.kind {
            EntryKind::Dir => {
                if path.is_file() || path.is_symlink() {
                    remove_path(&path)?;
                }
                fs::create_dir_all(&path)
                    .map_err(|e| io_err(format!("create {}", path.display()), e))?;
            }
            EntryKind::File => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| io_err(format!("create {}", parent.display()), e))?;
                }
                if path.is_dir() || path.is_symlink() {
                    remove_path(&path)?;
                }
                fs::write(&path, self.slice(entry))
                    .map_err(|e| io_err(format!("write {}", path.display()), e))?;
                set_mode(&path, entry.mode)?;
            }
            EntryKind::Symlink => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| io_err(format!("create {}", parent.display()), e))?;
                }
                if fs::symlink_metadata(&path).is_ok() {
                    remove_path(&path)?;
                }
                make_symlink(entry.target.as_deref().unwrap_or_default(), &path)?;
            }
        }
        Ok(())
    }

    /// Paths that differ between `self` (earlier) and `next` (later).
    pub fn changes_to(&self, next: &WorkspaceArchive) -> Vec<WorkspaceChange> {
        let before = self
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let after = next
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let mut changes = Vec::new();
        for (path, entry) in &after {
            match before.get(path) {
                None => changes.push(WorkspaceChange {
                    path: (*path).to_owned(),
                    kind: ChangeKind::Added,
                    entry_kind: entry.kind,
                }),
                Some(old) if !old.same_content(entry) => changes.push(WorkspaceChange {
                    path: (*path).to_owned(),
                    kind: ChangeKind::Modified,
                    entry_kind: entry.kind,
                }),
                Some(_) => {}
            }
        }
        for (path, entry) in &before {
            if !after.contains_key(path) {
                changes.push(WorkspaceChange {
                    path: (*path).to_owned(),
                    kind: ChangeKind::Removed,
                    entry_kind: entry.kind,
                });
            }
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        changes
    }

    /// Replay the recorded transition `self -> next` onto the directory
    /// `dest`: paths changed in `next` are written from `next`, paths removed
    /// in `next` are deleted. Paths the transition did not touch are left as
    /// they are in `dest`, so changes made there by re-executed spans survive.
    pub fn apply_transition(
        &self,
        next: &WorkspaceArchive,
        dest: &Path,
    ) -> Result<Vec<WorkspaceChange>, StorageError> {
        let changes = self.changes_to(next);
        // Removals deepest-first, then writes parents-first.
        for change in changes.iter().rev() {
            if change.kind == ChangeKind::Removed {
                let path = dest.join(safe_relative(&change.path)?);
                if fs::symlink_metadata(&path).is_ok() {
                    remove_path(&path)?;
                }
            }
        }
        for change in &changes {
            if change.kind != ChangeKind::Removed
                && let Some(entry) = next.entry(&change.path)
            {
                next.write_entry(dest, entry)?;
                if entry.kind == EntryKind::Dir {
                    set_mode(&dest.join(safe_relative(&entry.path)?), entry.mode)?;
                }
            }
        }
        Ok(changes)
    }
}

fn remove_path(path: &Path) -> Result<(), StorageError> {
    let meta =
        fs::symlink_metadata(path).map_err(|e| io_err(format!("stat {}", path.display()), e))?;
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|e| io_err(format!("remove {}", path.display()), e))
    } else {
        fs::remove_file(path).map_err(|e| io_err(format!("remove {}", path.display()), e))
    }
}

// ---------------------------------------------------------------------------
// Snapshot points recorded in storage
// ---------------------------------------------------------------------------

/// Where in a run a workspace snapshot was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspacePhase {
    /// Immediately before the span started.
    Before,
    /// Immediately after the span ended.
    After,
    /// When the run finished (no span).
    End,
}

impl WorkspacePhase {
    pub fn label(self) -> &'static str {
        match self {
            WorkspacePhase::Before => "before",
            WorkspacePhase::After => "after",
            WorkspacePhase::End => "end",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "before" => Some(WorkspacePhase::Before),
            "after" => Some(WorkspacePhase::After),
            "end" => Some(WorkspacePhase::End),
            _ => None,
        }
    }
}

/// A workspace snapshot record together with its decoded position.
#[derive(Clone, Debug)]
pub struct WorkspacePoint {
    pub snapshot: SnapshotRecord,
    pub phase: WorkspacePhase,
    pub ordinal: i64,
    pub span_id: Option<SpanId>,
}

/// All workspace snapshot points of a run, in capture order.
pub fn list_workspace_points<S: Storage + ?Sized>(
    storage: &S,
    run_id: &RunId,
) -> Result<Vec<WorkspacePoint>, StorageError> {
    let mut points = storage
        .list_snapshots(run_id)?
        .into_iter()
        .filter(|snapshot| snapshot.kind == WORKSPACE_SNAPSHOT_KIND)
        .filter_map(|snapshot| {
            let phase = match snapshot.summary.get("phase") {
                Some(Value::Text(text)) => WorkspacePhase::parse(text)?,
                _ => return None,
            };
            let ordinal = match snapshot.summary.get("ordinal") {
                Some(Value::Int(n)) => *n,
                _ => return None,
            };
            Some(WorkspacePoint {
                span_id: snapshot.span_id.clone(),
                snapshot,
                phase,
                ordinal,
            })
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.ordinal);
    Ok(points)
}

/// The recorded workspace root of a run's snapshots, if any.
pub fn workspace_root_of(point: &WorkspacePoint) -> Option<String> {
    match point.snapshot.summary.get("root") {
        Some(Value::Text(root)) => Some(root.clone()),
        _ => None,
    }
}

/// Load and decode the archive behind a workspace snapshot.
pub fn load_workspace_archive<S: Storage + ?Sized>(
    storage: &S,
    snapshot: &SnapshotRecord,
) -> Result<WorkspaceArchive, StorageError> {
    let bytes = storage.read_artifact_content(&snapshot.run_id, &snapshot.artifact_id)?;
    WorkspaceArchive::from_bytes(&bytes)
}

/// Persist `archive` as a workspace snapshot point of `run_id`.
#[allow(clippy::too_many_arguments)]
pub fn record_workspace_point<S: Storage + ?Sized>(
    storage: &S,
    run_id: &RunId,
    span_id: Option<&SpanId>,
    phase: WorkspacePhase,
    ordinal: i64,
    archive: &WorkspaceArchive,
    root_label: &str,
    created_at: u64,
) -> Result<SnapshotRecord, StorageError> {
    let bytes = archive.to_bytes();
    let mut summary = Document::new();
    summary.insert("phase".into(), Value::Text(phase.label().into()));
    summary.insert("ordinal".into(), Value::Int(ordinal));
    summary.insert("root".into(), Value::Text(root_label.into()));
    summary.insert("files".into(), Value::Int(archive.file_count() as i64));
    summary.insert("bytes".into(), Value::Int(archive.total_bytes() as i64));
    let artifact = ArtifactRecord {
        artifact_id: ArtifactId(storage.allocate_id(IdKind::Artifact)?),
        run_id: run_id.clone(),
        span_id: span_id.cloned(),
        artifact_type: ArtifactType::StateSnapshot,
        mime: WORKSPACE_ARCHIVE_MIME.into(),
        sha256: String::new(),
        byte_len: bytes.len(),
        blob_path: String::new(),
        summary: summary.clone(),
        redaction: Document::new(),
        created_at,
    };
    let artifact = storage.store_artifact_with_content(artifact, &bytes)?;
    let snapshot = SnapshotRecord {
        snapshot_id: SnapshotId(storage.allocate_id(IdKind::Snapshot)?),
        run_id: run_id.clone(),
        span_id: span_id.cloned(),
        kind: WORKSPACE_SNAPSHOT_KIND.into(),
        artifact_id: artifact.artifact_id,
        summary,
        created_at,
    };
    storage.insert_snapshot(snapshot.clone())?;
    Ok(snapshot)
}

/// Capture `root` and record it as the next workspace point of `run_id`.
pub fn capture_workspace_point<S: Storage + ?Sized>(
    storage: &S,
    run_id: &RunId,
    span_id: Option<&SpanId>,
    phase: WorkspacePhase,
    root: &Path,
    options: &CaptureOptions,
    created_at: u64,
) -> Result<SnapshotRecord, StorageError> {
    let archive = WorkspaceArchive::capture_with(root, options)?;
    let ordinal = list_workspace_points(storage, run_id)?
        .last()
        .map(|point| point.ordinal + 1)
        .unwrap_or(0);
    record_workspace_point(
        storage,
        run_id,
        span_id,
        phase,
        ordinal,
        &archive,
        &root.to_string_lossy(),
        created_at,
    )
}

/// File-level difference between the final workspaces of two runs, with
/// small UTF-8 file contents inlined for display.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceFileDiff {
    pub path: String,
    pub change: ChangeKind,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// Compare two archives, skipping paths under `skip_prefixes` (e.g. `.git`).
pub fn diff_workspaces(
    before: &WorkspaceArchive,
    after: &WorkspaceArchive,
    skip_prefixes: &[&str],
) -> Vec<WorkspaceFileDiff> {
    const INLINE_LIMIT: usize = 64 * 1024;
    let text = |archive: &WorkspaceArchive, path: &str| {
        archive
            .file_content(path)
            .filter(|bytes| bytes.len() <= INLINE_LIMIT)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(str::to_owned)
    };
    before
        .changes_to(after)
        .into_iter()
        .filter(|change| change.entry_kind == EntryKind::File)
        .filter(|change| {
            !skip_prefixes.iter().any(|prefix| {
                change.path == *prefix || change.path.starts_with(&format!("{prefix}/"))
            })
        })
        .map(|change| WorkspaceFileDiff {
            before: text(before, &change.path),
            after: text(after, &change.path),
            path: change.path,
            change: change.kind,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rk-ws-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn capture_restore_roundtrip_is_deterministic() {
        let root = scratch("roundtrip");
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/a.py"), "print('a')\n").unwrap();
        fs::write(root.join("src/nested/b.txt"), "bee").unwrap();
        fs::write(root.join("README"), "hi").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("README"), fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("src/a.py", root.join("link")).unwrap();
        }

        let first = WorkspaceArchive::capture(&root).unwrap();
        let second = WorkspaceArchive::capture(&root).unwrap();
        assert_eq!(first.to_bytes(), second.to_bytes());

        let decoded = WorkspaceArchive::from_bytes(&first.to_bytes()).unwrap();
        assert_eq!(decoded, first);
        assert_eq!(decoded.file_content("src/nested/b.txt"), Some(&b"bee"[..]));

        let dest = scratch("roundtrip-dest");
        fs::remove_dir_all(&dest).unwrap();
        decoded.restore(&dest).unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("src/a.py")).unwrap(),
            "print('a')\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dest.join("README"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
            assert_eq!(
                fs::read_link(dest.join("link")).unwrap(),
                PathBuf::from("src/a.py")
            );
        }
        assert_eq!(
            WorkspaceArchive::capture(&dest).unwrap().entries,
            first.entries
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn apply_transition_preserves_unrelated_local_changes() {
        let root = scratch("transition");
        fs::write(root.join("a.txt"), "a1").unwrap();
        fs::write(root.join("b.txt"), "b1").unwrap();
        let before = WorkspaceArchive::capture(&root).unwrap();
        fs::write(root.join("a.txt"), "a2").unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        fs::write(root.join("c.txt"), "c").unwrap();
        let after = WorkspaceArchive::capture(&root).unwrap();

        let changes = before.changes_to(&after);
        assert_eq!(
            changes
                .iter()
                .map(|c| (c.path.as_str(), c.kind))
                .collect::<Vec<_>>(),
            vec![
                ("a.txt", ChangeKind::Modified),
                ("b.txt", ChangeKind::Removed),
                ("c.txt", ChangeKind::Added),
            ]
        );

        let sandbox = scratch("transition-sandbox");
        fs::remove_dir_all(&sandbox).unwrap();
        before.restore(&sandbox).unwrap();
        fs::write(sandbox.join("local.txt"), "mine").unwrap();
        before.apply_transition(&after, &sandbox).unwrap();
        assert_eq!(fs::read_to_string(sandbox.join("a.txt")).unwrap(), "a2");
        assert!(!sandbox.join("b.txt").exists());
        assert_eq!(fs::read_to_string(sandbox.join("c.txt")).unwrap(), "c");
        assert_eq!(
            fs::read_to_string(sandbox.join("local.txt")).unwrap(),
            "mine"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn capture_respects_limits_and_excludes() {
        let root = scratch("limits");
        fs::write(root.join("big.bin"), vec![0u8; 64]).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/replaykit.db"), "db").unwrap();
        let err = WorkspaceArchive::capture_with(
            &root,
            &CaptureOptions {
                max_bytes: 10,
                exclude: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");

        let archive = WorkspaceArchive::capture_with(
            &root,
            &CaptureOptions {
                max_bytes: 1024,
                exclude: vec![root.join("data")],
            },
        )
        .unwrap();
        assert!(archive.entry("data").is_none());
        assert!(archive.entry("big.bin").is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_archives_that_escape_the_workspace() {
        let archive = WorkspaceArchive {
            entries: vec![WorkspaceEntry {
                path: "../evil".into(),
                kind: EntryKind::File,
                mode: 0o644,
                sha256: None,
                len: 0,
                offset: 0,
                target: None,
            }],
            data: Vec::new(),
        };
        assert!(WorkspaceArchive::from_bytes(&archive.to_bytes()).is_err());
    }
}
