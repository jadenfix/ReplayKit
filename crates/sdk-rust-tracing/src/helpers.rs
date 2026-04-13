use replaykit_core_model::{
    ArtifactType, CostMetrics, Document, ReplayPolicy, SpanId, SpanKind, SpanStatus, Value, attrs,
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

    /// Merge freeform attributes into the span's attribute map.
    pub fn attributes(mut self, extra: Document) -> Self {
        self.spec.attributes.extend(extra);
        self
    }

    /// Set the shell command string (ShellCommand spans).
    ///
    /// **Required for replay.** Without this, the executor falls back to the
    /// span name, which weakens replay semantics with a contract warning.
    pub fn command(mut self, cmd: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::COMMAND.into(), Value::Text(cmd.into()));
        self
    }

    /// Set the working directory (ShellCommand spans).
    ///
    /// **Recommended for replay.** Without this, the executor uses the process
    /// working directory at replay time, which may differ from the original run.
    pub fn cwd(mut self, dir: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::CWD.into(), Value::Text(dir.into()));
        self
    }

    /// Set execution timeout in milliseconds (ShellCommand spans).
    ///
    /// Optional. Currently advisory — the executor does not enforce timeouts.
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.spec
            .attributes
            .insert(attrs::TIMEOUT_MS.into(), Value::Int(ms as i64));
        self
    }

    /// Set the filesystem path (FileRead/FileWrite spans).
    ///
    /// **Required for replay.** Without this, the executor falls back to the
    /// span name, which weakens replay semantics with a contract warning.
    pub fn path(mut self, p: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::PATH.into(), Value::Text(p.into()));
        self
    }

    /// Set the content to write (FileWrite spans).
    ///
    /// **Required for replay.** The executor hard-blocks if content is missing —
    /// a file write cannot be replayed without knowing what to write.
    pub fn write_content(mut self, content: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::CONTENT.into(), Value::Text(content.into()));
        self
    }

    /// Set the model provider name (LlmCall spans).
    ///
    /// **Recommended for replay.** Needed for live model dispatch via
    /// `PassthroughModelExecutor`. Omitting produces a contract warning.
    pub fn provider(mut self, name: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::PROVIDER.into(), Value::Text(name.into()));
        self
    }

    /// Set the model identifier (LlmCall spans).
    ///
    /// **Required for replay.** The executor hard-blocks if neither `model`
    /// nor `executor_kind` is present.
    pub fn model(mut self, name: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::MODEL.into(), Value::Text(name.into()));
        self
    }

    /// Set the serialized model request payload (LlmCall spans).
    ///
    /// **Recommended for replay.** Captures the full request for fidelity
    /// verification. Omitting produces a contract warning.
    pub fn model_request_json(mut self, json: impl Into<String>) -> Self {
        self.spec
            .attributes
            .insert(attrs::MODEL_REQUEST_JSON.into(), Value::Text(json.into()));
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
///
/// **Replay contract:**
/// - Required: `.model()` — the model identifier (or set `executor_kind`)
/// - Recommended: `.provider()` — needed for live dispatch
/// - Recommended: `.model_request_json()` — captures the full request
pub fn model_call(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::LlmCall, name, ReplayPolicy::RerunnableSupported)
}

/// Tool call (default: `RerunnableSupported`).
pub fn tool_call(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::ToolCall, name, ReplayPolicy::RerunnableSupported)
}

/// Shell command (default: `RerunnableSupported`).
///
/// **Replay contract:**
/// - Required: `.command()` — the shell command string
/// - Recommended: `.cwd()` — working directory (falls back to process cwd)
/// - Optional: `.timeout_ms()` — execution timeout (advisory)
pub fn shell_command(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(
        SpanKind::ShellCommand,
        name,
        ReplayPolicy::RerunnableSupported,
    )
}

/// File read (default: `PureReusable` — deterministic if file unchanged).
///
/// **Replay contract:**
/// - Required: `.path()` — the filesystem path to read
pub fn file_read(name: impl Into<String>) -> SpanBuilder {
    SpanBuilder::new(SpanKind::FileRead, name, ReplayPolicy::PureReusable)
}

/// File write (default: `RerunnableSupported`).
///
/// **Replay contract:**
/// - Required: `.path()` — the target filesystem path
/// - Required: `.write_content()` — the content to write (hard-blocks without it)
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
