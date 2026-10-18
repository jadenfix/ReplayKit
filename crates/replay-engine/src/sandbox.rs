//! The hermetic replay sandbox.
//!
//! Every forked replay runs in a private directory restored from the
//! workspace snapshot recorded immediately before the first span that must
//! re-execute. As the replay walks forward through the recorded timeline,
//! the recorded workspace changes of spans that are *not* re-executed are
//! re-applied to the sandbox, while re-executed spans produce their own
//! changes live. The live working tree is never read or written.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use replaykit_core_model::{RunId, SpanId};
use replaykit_storage::workspace::{
    WorkspaceArchive, WorkspacePhase, WorkspacePoint, load_workspace_archive,
    record_workspace_point,
};
use replaykit_storage::{Storage, StorageError};

/// The sandbox a replayed span executes in.
#[derive(Clone, Debug)]
pub struct ReplayWorkspace {
    /// Private directory holding the restored workspace.
    pub sandbox: PathBuf,
    /// The workspace root the run was recorded in (absolute), if known.
    pub recorded_root: Option<String>,
}

impl ReplayWorkspace {
    /// Map a recorded path (relative, or absolute under the recorded
    /// workspace root) into the sandbox. Paths that would escape the sandbox
    /// are rejected, because replaying them would touch the live filesystem.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            let Some(root) = &self.recorded_root else {
                return Err(format!(
                    "absolute path {path} is outside the replay sandbox and the run recorded no \
                     workspace; re-record with a workspace_root to replay it hermetically"
                ));
            };
            for alias in root_aliases(root) {
                if let Ok(rest) = candidate.strip_prefix(&alias) {
                    return join_inside(&self.sandbox, rest, path);
                }
            }
            if let Ok(canonical) = std::fs::canonicalize(candidate) {
                for alias in root_aliases(root) {
                    if let Ok(rest) = canonical.strip_prefix(&alias) {
                        return join_inside(&self.sandbox, rest, path);
                    }
                }
            }
            Err(format!(
                "path {path} is outside the recorded workspace {root}; replaying it would touch \
                 the live filesystem"
            ))
        } else {
            join_inside(&self.sandbox, candidate, path)
        }
    }
}

fn root_aliases(root: &str) -> Vec<PathBuf> {
    let mut aliases = vec![PathBuf::from(root)];
    if let Some(rest) = root.strip_prefix("/private/") {
        aliases.push(PathBuf::from(format!("/{rest}")));
    } else if root.starts_with("/tmp/") || root.starts_with("/var/") {
        aliases.push(PathBuf::from(format!("/private{root}")));
    }
    aliases
}

fn join_inside(sandbox: &Path, rest: &Path, original: &str) -> Result<PathBuf, String> {
    let mut out = sandbox.to_path_buf();
    let mut depth = 0usize;
    for component in rest.components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                depth += 1;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(format!("path {original} escapes the replay sandbox"));
                }
                out.pop();
                depth -= 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("path {original} escapes the replay sandbox"));
            }
        }
    }
    Ok(out)
}

/// Drives the sandbox along the recorded workspace timeline of the source
/// run and records the replayed timeline into the target run.
pub(crate) struct WorkspaceTimeline<'a, S: Storage> {
    storage: &'a S,
    target_run_id: RunId,
    points: Vec<WorkspacePoint>,
    archives: Vec<Option<WorkspaceArchive>>,
    /// `owners[i]`: innermost span open between point `i` and `i + 1`.
    owners: Vec<Option<SpanId>>,
    cursor: Option<usize>,
    sandbox: PathBuf,
    created_at: u64,
}

impl<'a, S: Storage> WorkspaceTimeline<'a, S> {
    pub(crate) fn new(
        storage: &'a S,
        target_run_id: RunId,
        points: Vec<WorkspacePoint>,
        sandbox: PathBuf,
        created_at: u64,
    ) -> Self {
        let mut owners = Vec::with_capacity(points.len());
        let mut stack: Vec<SpanId> = Vec::new();
        for point in &points {
            match (point.phase, &point.span_id) {
                (WorkspacePhase::Before, Some(span_id)) => stack.push(span_id.clone()),
                (WorkspacePhase::After, Some(span_id)) => {
                    if let Some(pos) = stack.iter().rposition(|open| open == span_id) {
                        stack.truncate(pos);
                    }
                }
                (WorkspacePhase::End, _) => stack.clear(),
                _ => {}
            }
            owners.push(stack.last().cloned());
        }
        let archives = vec![None; points.len()];
        Self {
            storage,
            target_run_id,
            points,
            archives,
            owners,
            cursor: None,
            sandbox,
            created_at,
        }
    }

    pub(crate) fn index_of(&self, phase: WorkspacePhase, span_id: &SpanId) -> Option<usize> {
        self.points
            .iter()
            .position(|p| p.phase == phase && p.span_id.as_ref() == Some(span_id))
    }

    fn archive(&mut self, index: usize) -> Result<&WorkspaceArchive, StorageError> {
        if self.archives[index].is_none() {
            let archive = load_workspace_archive(self.storage, &self.points[index].snapshot)?;
            self.archives[index] = Some(archive);
        }
        Ok(self.archives[index].as_ref().expect("archive loaded"))
    }

    /// Restore the sandbox from point `index` (or from `override_archive`)
    /// and record that point in the target run.
    pub(crate) fn start(
        &mut self,
        index: usize,
        override_archive: Option<&WorkspaceArchive>,
    ) -> Result<(), StorageError> {
        match override_archive {
            Some(archive) => archive.restore(&self.sandbox)?,
            None => {
                let sandbox = self.sandbox.clone();
                self.archive(index)?.restore(&sandbox)?;
            }
        }
        self.cursor = Some(index);
        self.record(index)
    }

    /// Restore an arbitrary archive with no recorded timeline.
    pub(crate) fn start_detached(
        &mut self,
        archive: Option<&WorkspaceArchive>,
    ) -> Result<(), StorageError> {
        match archive {
            Some(archive) => archive.restore(&self.sandbox),
            None => std::fs::create_dir_all(&self.sandbox).map_err(|e| {
                StorageError::Internal(format!("create sandbox {}: {e}", self.sandbox.display()))
            }),
        }
    }

    /// Walk forward to point `index`, re-applying the recorded workspace
    /// changes of every interval whose owning span is not in `skip`
    /// (re-executed or blocked spans), and record each reached point.
    pub(crate) fn advance_to(
        &mut self,
        index: usize,
        skip: &BTreeSet<SpanId>,
    ) -> Result<(), StorageError> {
        let Some(mut cursor) = self.cursor else {
            return Ok(());
        };
        while cursor < index && cursor + 1 < self.points.len() {
            let owner = self.owners[cursor].clone();
            let apply = owner.as_ref().is_none_or(|span| !skip.contains(span));
            if apply {
                self.archive(cursor)?;
                self.archive(cursor + 1)?;
                let from = self.archives[cursor].as_ref().expect("loaded");
                let to = self.archives[cursor + 1].as_ref().expect("loaded");
                from.apply_transition(to, &self.sandbox)?;
            }
            cursor += 1;
            self.cursor = Some(cursor);
            self.record(cursor)?;
        }
        Ok(())
    }

    /// Advance through the rest of the recorded timeline. Returns whether an
    /// end-of-run point was recorded.
    pub(crate) fn finish(&mut self, skip: &BTreeSet<SpanId>) -> Result<bool, StorageError> {
        if self.cursor.is_none() || self.points.is_empty() {
            return Ok(false);
        }
        self.advance_to(self.points.len() - 1, skip)?;
        let last = self.points.last().expect("non-empty");
        if last.phase != WorkspacePhase::End {
            let ordinal = last.ordinal + 1;
            let archive = WorkspaceArchive::capture(&self.sandbox)?;
            let root = replaykit_storage::workspace::workspace_root_of(last).unwrap_or_default();
            record_workspace_point(
                self.storage,
                &self.target_run_id,
                None,
                WorkspacePhase::End,
                ordinal,
                &archive,
                &root,
                self.created_at,
            )?;
        }
        Ok(true)
    }

    fn record(&self, index: usize) -> Result<(), StorageError> {
        let point = &self.points[index];
        let archive = WorkspaceArchive::capture(&self.sandbox)?;
        let root = replaykit_storage::workspace::workspace_root_of(point).unwrap_or_default();
        record_workspace_point(
            self.storage,
            &self.target_run_id,
            point.span_id.as_ref(),
            point.phase,
            point.ordinal,
            &archive,
            &root,
            self.created_at,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_recorded_absolute_paths_into_the_sandbox() {
        let ws = ReplayWorkspace {
            sandbox: PathBuf::from("/sb"),
            recorded_root: Some("/private/tmp/repo".into()),
        };
        assert_eq!(
            ws.resolve("src/a.py").unwrap(),
            PathBuf::from("/sb/src/a.py")
        );
        assert_eq!(ws.resolve(".").unwrap(), PathBuf::from("/sb"));
        assert_eq!(
            ws.resolve("/private/tmp/repo/src/a.py").unwrap(),
            PathBuf::from("/sb/src/a.py")
        );
        assert_eq!(
            ws.resolve("/tmp/repo/src").unwrap(),
            PathBuf::from("/sb/src")
        );
        assert!(ws.resolve("/etc/passwd").is_err());
        assert!(ws.resolve("../outside").is_err());
        assert_eq!(ws.resolve("a/../b").unwrap(), PathBuf::from("/sb/b"));

        let no_root = ReplayWorkspace {
            sandbox: PathBuf::from("/sb"),
            recorded_root: None,
        };
        assert!(no_root.resolve("/workspace/project").is_err());
        assert_eq!(no_root.resolve("x").unwrap(), PathBuf::from("/sb/x"));
    }
}
