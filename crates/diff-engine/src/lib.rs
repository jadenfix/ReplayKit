use std::collections::BTreeMap;
use std::sync::Arc;

use replaykit_core_model::{DiffId, Document, IdKind, RunDiffRecord, RunId, Value};
use replaykit_storage::{Storage, StorageError};

#[derive(Debug)]
pub enum DiffError {
    Storage(StorageError),
}

impl From<StorageError> for DiffError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

pub struct DiffEngine<S: Storage> {
    storage: Arc<S>,
}

impl<S: Storage> DiffEngine<S> {
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }

    pub fn diff_runs(
        &self,
        source_run_id: &RunId,
        target_run_id: &RunId,
        created_at: u64,
    ) -> Result<RunDiffRecord, DiffError> {
        let source_run = self.storage.get_run(source_run_id)?;
        let target_run = self.storage.get_run(target_run_id)?;
        let source_spans = self.storage.list_spans(source_run_id)?;
        let target_spans = self.storage.list_spans(target_run_id)?;
        let source_artifacts = self.storage.list_artifacts(source_run_id)?;
        let target_artifacts = self.storage.list_artifacts(target_run_id)?;

        let source_order = source_spans
            .iter()
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();
        let source_ids = source_order
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let target_only_ids = target_spans
            .iter()
            .filter(|span| !source_ids.contains(&span.span_id))
            .map(|span| span.span_id.clone())
            .collect::<Vec<_>>();
        let source_map = source_spans
            .into_iter()
            .map(|span| (span.span_id.clone(), span))
            .collect::<BTreeMap<_, _>>();
        let target_map = target_spans
            .into_iter()
            .map(|span| (span.span_id.clone(), span))
            .collect::<BTreeMap<_, _>>();

        let ordered_span_ids = source_order
            .into_iter()
            .chain(target_only_ids)
            .collect::<Vec<_>>();

        let mut first_divergent_span_id = None;
        let mut changed_span_count = 0usize;

        for span_id in ordered_span_ids {
            let source_span = source_map.get(&span_id);
            let target_span = target_map.get(&span_id);
            if spans_differ(source_span, target_span) {
                changed_span_count += 1;
                if first_divergent_span_id.is_none() {
                    first_divergent_span_id = Some(span_id);
                }
            }
        }

        let changed_artifact_count = count_changed_artifacts(&source_artifacts, &target_artifacts);
        let mut summary = Document::new();
        summary.insert(
            "source_status".into(),
            Value::Text(format!("{:?}", source_run.status)),
        );
        summary.insert(
            "target_status".into(),
            Value::Text(format!("{:?}", target_run.status)),
        );
        summary.insert(
            "changed_span_count".into(),
            Value::Int(changed_span_count as i64),
        );
        summary.insert(
            "changed_artifact_count".into(),
            Value::Int(changed_artifact_count as i64),
        );
        if let Some(span_id) = &first_divergent_span_id {
            summary.insert(
                "first_divergent_span".into(),
                Value::Text(span_id.0.clone()),
            );
        }

        let diff = RunDiffRecord {
            diff_id: DiffId(self.storage.allocate_id(IdKind::Diff)?),
            source_run_id: source_run_id.clone(),
            target_run_id: target_run_id.clone(),
            first_divergent_span_id,
            changed_span_count,
            changed_artifact_count,
            source_status: source_run.status,
            target_status: target_run.status,
            summary,
            created_at,
        };
        self.storage.insert_diff(diff.clone())?;
        Ok(diff)
    }

    pub fn get_cached_diff(
        &self,
        source_run_id: &RunId,
        target_run_id: &RunId,
    ) -> Result<RunDiffRecord, DiffError> {
        self.storage
            .get_diff(source_run_id, target_run_id)
            .map_err(Into::into)
    }
}

fn spans_differ(
    source_span: Option<&replaykit_core_model::SpanRecord>,
    target_span: Option<&replaykit_core_model::SpanRecord>,
) -> bool {
    match (source_span, target_span) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(source_span), Some(target_span)) => {
            source_span.status != target_span.status
                || source_span.output_fingerprint != target_span.output_fingerprint
                || source_span.input_fingerprint != target_span.input_fingerprint
                || source_span.snapshot_id != target_span.snapshot_id
                || source_span.error_summary != target_span.error_summary
        }
    }
}

fn count_changed_artifacts(
    source_artifacts: &[replaykit_core_model::ArtifactRecord],
    target_artifacts: &[replaykit_core_model::ArtifactRecord],
) -> usize {
    let source = source_artifacts
        .iter()
        .map(|artifact| (artifact.artifact_id.clone(), artifact.sha256.clone()))
        .collect::<BTreeMap<_, _>>();
    let target = target_artifacts
        .iter()
        .map(|artifact| (artifact.artifact_id.clone(), artifact.sha256.clone()))
        .collect::<BTreeMap<_, _>>();

    let ids = source
        .keys()
        .cloned()
        .chain(target.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();

    ids.into_iter()
        .filter(|artifact_id| source.get(artifact_id) != target.get(artifact_id))
        .count()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use replaykit_core_model::{
        ArtifactId, ArtifactRecord, ArtifactType, ReplayPolicy, RunRecord, RunStatus, SpanId,
        SpanKind, SpanRecord, SpanStatus, TraceId,
    };
    use replaykit_storage::InMemoryStorage;

    use super::*;

    #[test]
    fn finds_first_divergent_span() {
        let storage = Arc::new(InMemoryStorage::new());
        let mut source = RunRecord::new(
            RunId("run-a".into()),
            TraceId("trace-a".into()),
            "a",
            "agent.main",
            "adapter",
            "0.1.0",
            1,
        );
        source.status = RunStatus::Failed;
        let mut target = source.clone();
        target.run_id = RunId("run-b".into());
        target.status = RunStatus::Completed;
        storage.insert_run(source.clone()).unwrap();
        storage.insert_run(target.clone()).unwrap();

        let source_span = SpanRecord {
            run_id: source.run_id.clone(),
            span_id: SpanId("span-1".into()),
            trace_id: source.trace_id.clone(),
            parent_span_id: None,
            sequence_no: 1,
            kind: SpanKind::ToolCall,
            name: "tool".into(),
            status: SpanStatus::Failed,
            started_at: 1,
            ended_at: Some(2),
            replay_policy: ReplayPolicy::RerunnableSupported,
            executor_kind: None,
            executor_version: None,
            input_artifact_ids: Vec::new(),
            output_artifact_ids: vec![ArtifactId("artifact-1".into())],
            snapshot_id: None,
            input_fingerprint: None,
            output_fingerprint: Some("old".into()),
            environment_fingerprint: None,
            attributes: BTreeMap::new(),
            error_code: None,
            error_summary: Some("failed".into()),
            cost: Default::default(),
        };
        let mut target_span = source_span.clone();
        target_span.run_id = target.run_id.clone();
        target_span.status = SpanStatus::Completed;
        target_span.output_fingerprint = Some("new".into());
        target_span.error_summary = None;
        storage.upsert_span(source_span).unwrap();
        storage.upsert_span(target_span).unwrap();
        storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("artifact-1".into()),
                run_id: source.run_id.clone(),
                span_id: Some(SpanId("span-1".into())),
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "old".into(),
                byte_len: 1,
                blob_path: "memory://old".into(),
                summary: BTreeMap::new(),
                redaction: BTreeMap::new(),
                created_at: 1,
            })
            .unwrap();
        storage
            .insert_artifact(ArtifactRecord {
                artifact_id: ArtifactId("artifact-1".into()),
                run_id: target.run_id.clone(),
                span_id: Some(SpanId("span-1".into())),
                artifact_type: ArtifactType::ToolOutput,
                mime: "application/json".into(),
                sha256: "new".into(),
                byte_len: 1,
                blob_path: "memory://new".into(),
                summary: BTreeMap::new(),
                redaction: BTreeMap::new(),
                created_at: 1,
            })
            .unwrap();

        let engine = DiffEngine::new(storage);
        let diff = engine
            .diff_runs(&source.run_id, &target.run_id, 10)
            .unwrap();
        assert_eq!(diff.first_divergent_span_id, Some(SpanId("span-1".into())));
        assert_eq!(diff.changed_span_count, 1);
    }

    #[test]
    fn counts_target_only_spans_as_changed() {
        let storage = Arc::new(InMemoryStorage::new());
        let source = RunRecord::new(
            RunId("run-a".into()),
            TraceId("trace-a".into()),
            "a",
            "agent.main",
            "adapter",
            "0.1.0",
            1,
        );
        let mut target = source.clone();
        target.run_id = RunId("run-b".into());
        storage.insert_run(source.clone()).unwrap();
        storage.insert_run(target.clone()).unwrap();

        storage
            .upsert_span(SpanRecord {
                run_id: target.run_id.clone(),
                span_id: SpanId("span-extra".into()),
                trace_id: target.trace_id.clone(),
                parent_span_id: None,
                sequence_no: 1,
                kind: SpanKind::ToolCall,
                name: "extra".into(),
                status: SpanStatus::Completed,
                started_at: 1,
                ended_at: Some(2),
                replay_policy: ReplayPolicy::RerunnableSupported,
                executor_kind: None,
                executor_version: None,
                input_artifact_ids: Vec::new(),
                output_artifact_ids: Vec::new(),
                snapshot_id: None,
                input_fingerprint: None,
                output_fingerprint: Some("extra".into()),
                environment_fingerprint: None,
                attributes: BTreeMap::new(),
                error_code: None,
                error_summary: None,
                cost: Default::default(),
            })
            .unwrap();

        let engine = DiffEngine::new(storage);
        let diff = engine
            .diff_runs(&source.run_id, &target.run_id, 10)
            .unwrap();
        assert_eq!(
            diff.first_divergent_span_id,
            Some(SpanId("span-extra".into()))
        );
        assert_eq!(diff.changed_span_count, 1);
    }
}
