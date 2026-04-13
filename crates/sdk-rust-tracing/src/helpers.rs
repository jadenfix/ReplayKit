use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, ReplayPolicy, SpanId, SpanKind, SpanStatus,
};

use crate::CompletedSpanSpec;

// ---------------------------------------------------------------------------
// SpanBuilder
// ---------------------------------------------------------------------------

/// Ergonomic builder for [`CompletedSpanSpec`].
///
/// Factory functions like [`planner_step`], [`model_call`], [`tool_call`],
/// etc. set the correct [`SpanKind`] and default [`ReplayPolicy`] so callers
/// don't have to remember the right combinations.
pub struct SpanBuilder {
    spec: CompletedSpanSpec,
}

impl SpanBuilder {
    fn new(kind: SpanKind, name: impl Into<String>, replay_policy: ReplayPolicy) -> Self {
        let name = name.into();
        Self {
            spec: CompletedSpanSpec {
                kind,
                replay_policy,
                ..CompletedSpanSpec::simple(kind, name, 0, 0)
            },
        }
    }

    /// Set a fixed span id (useful for deterministic fixtures).
    pub fn span_id(mut self, id: impl Into<String>) -> Self {
        self.spec.span_id = Some(SpanId(id.into()));
        self
    }

    /// Set control parent.
    pub fn parent(mut self, id: &SpanId) -> Self {
        self.spec.parent_span_id = Some(id.clone());
        self
    }

    /// Set start and end timestamps.
    pub fn times(mut self, started_at: u64, ended_at: u64) -> Self {
        self.spec.started_at = started_at;
        self.spec.ended_at = ended_at;
        self
    }

    /// Set span status (defaults to `Completed`).
    pub fn status(mut self, status: SpanStatus) -> Self {
        self.spec.status = status;
        self
    }

    /// Mark the span as failed with an error summary.
    pub fn failed(mut self, error: impl Into<String>) -> Self {
        self.spec.status = SpanStatus::Failed;
        self.spec.error_summary = Some(error.into());
        self
    }

    /// Set executor kind and version (e.g. `("claude-3.5-sonnet", "2024-10-22")`).
    pub fn executor(mut self, kind: impl Into<String>, version: impl Into<String>) -> Self {
        self.spec.executor_kind = Some(kind.into());
        self.spec.executor_version = Some(version.into());
        self
    }

    /// Override the replay policy set by the factory function.
    pub fn replay_policy(mut self, policy: ReplayPolicy) -> Self {
        self.spec.replay_policy = policy;
        self
    }

    pub fn input_fingerprint(mut self, fp: impl Into<String>) -> Self {
        self.spec.input_fingerprint = Some(fp.into());
        self
    }

    pub fn output_fingerprint(mut self, fp: impl Into<String>) -> Self {
        self.spec.output_fingerprint = Some(fp.into());
        self
    }

    pub fn environment_fingerprint(mut self, fp: impl Into<String>) -> Self {
        self.spec.environment_fingerprint = Some(fp.into());
        self
    }

    /// Attach an input artifact summary.
    pub fn input(mut self, artifact_type: ArtifactType, summary: Document) -> Self {
        self.spec.input_artifact_type = Some(artifact_type);
        self.spec.input_summary = Some(summary);
        self
    }

    /// Attach an output artifact summary.
    pub fn output(mut self, artifact_type: ArtifactType, summary: Document) -> Self {
        self.spec.output_artifact_type = Some(artifact_type);
        self.spec.output_summary = Some(summary);
        self
    }

    /// Attach a state snapshot summary.
    pub fn snapshot(mut self, summary: Document) -> Self {
        self.spec.snapshot_summary = Some(summary);
        self
    }

    /// Set cost metrics.
    pub fn cost(mut self, input_tokens: u64, output_tokens: u64, cost_micros: u64) -> Self {
        self.spec.cost = CostMetrics {
            input_tokens,
            output_tokens,
            estimated_cost_micros: cost_micros,
        };
        self
    }

    /// Set freeform attributes.
    pub fn attributes(mut self, attrs: Document) -> Self {
        self.spec.attributes = attrs;
        self
    }

    /// Consume the builder and return the completed spec.
    pub fn build(self) -> CompletedSpanSpec {
        self.spec
    }
}

// ---------------------------------------------------------------------------
// Semantic factory functions
// ---------------------------------------------------------------------------

/// Planner step (default: `RecordOnly`).
pub fn planner_step(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::PlannerStep, name, ReplayPolicy::RecordOnly)
}

/// LLM / model call (default: `RerunnableSupported`).
pub fn model_call(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::LlmCall, name, ReplayPolicy::RerunnableSupported)
}

/// Tool call (default: `RerunnableSupported`).
pub fn tool_call(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::ToolCall, name, ReplayPolicy::RerunnableSupported)
}

/// Shell command (default: `RerunnableSupported`).
pub fn shell_command(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(
        SpanKind::ShellCommand,
        name,
        ReplayPolicy::RerunnableSupported,
    )
}

/// File read (default: `PureReusable` — deterministic if file unchanged).
pub fn file_read(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::FileRead, name, ReplayPolicy::PureReusable)
}

/// File write (default: `RerunnableSupported`).
pub fn file_write(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::FileWrite, name, ReplayPolicy::RerunnableSupported)
}

/// Human input (default: `RecordOnly` — cannot be replayed).
pub fn human_input(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::HumanInput, name, ReplayPolicy::RecordOnly)
}

/// Retrieval / search (default: `CacheableIfFingerprintMatches`).
pub fn retrieval(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(
        SpanKind::Retrieval,
        name,
        ReplayPolicy::CacheableIfFingerprintMatches,
    )
}

/// Guardrail check (default: `PureReusable`).
pub fn guardrail_check(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::GuardrailCheck, name, ReplayPolicy::PureReusable)
}
