use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use replaykit_core_model::{CostMetrics, Document, ReplayPolicy, SpanId, SpanKind, SpanStatus};

use crate::sink::{CloseSpanSpec, OpenSpanSpec, Sink};

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// Timestamp source.  Use [`WallClock`] in production and
/// [`SequentialClock`] for deterministic tests / fixtures.
pub trait Clock: Send + Sync {
    fn now(&self) -> u64;
}

/// Real wall-clock time (milliseconds since epoch).
pub struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// Monotonically-increasing counter, useful for deterministic tests.
pub struct SequentialClock {
    counter: AtomicU64,
}

impl SequentialClock {
    pub fn new(start: u64) -> Self {
        Self {
            counter: AtomicU64::new(start),
        }
    }
}

impl Clock for SequentialClock {
    fn now(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Span extension (stored in tracing span extensions)
// ---------------------------------------------------------------------------

struct SpanData {
    rk_span_id: SpanId,
    #[allow(dead_code)]
    started_at: u64,
    output_fingerprint: Option<String>,
    error_summary: Option<String>,
    status: SpanStatus,
    input_tokens: u64,
    output_tokens: u64,
    cost_micros: u64,
}

// ---------------------------------------------------------------------------
// Field visitor – extracts rk_* fields from span attributes
// ---------------------------------------------------------------------------

struct SpanFieldVisitor {
    kind: Option<SpanKind>,
    replay_policy: Option<ReplayPolicy>,
    span_id: Option<String>,
    executor_kind: Option<String>,
    executor_version: Option<String>,
    input_fingerprint: Option<String>,
    output_fingerprint: Option<String>,
    environment_fingerprint: Option<String>,
    error_summary: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cost_micros: u64,
    attributes: Document,
}

impl SpanFieldVisitor {
    fn new() -> Self {
        Self {
            kind: None,
            replay_policy: None,
            span_id: None,
            executor_kind: None,
            executor_version: None,
            input_fingerprint: None,
            output_fingerprint: None,
            environment_fingerprint: None,
            error_summary: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_micros: 0,
            attributes: Document::new(),
        }
    }
}

impl Visit for SpanFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "rk_kind" => self.kind = parse_span_kind(value),
            "rk_replay_policy" => self.replay_policy = parse_replay_policy(value),
            "rk_span_id" => self.span_id = Some(value.to_owned()),
            "rk_executor_kind" => self.executor_kind = Some(value.to_owned()),
            "rk_executor_version" => self.executor_version = Some(value.to_owned()),
            "rk_input_fingerprint" => self.input_fingerprint = Some(value.to_owned()),
            "rk_output_fingerprint" => self.output_fingerprint = Some(value.to_owned()),
            "rk_environment_fingerprint" => {
                self.environment_fingerprint = Some(value.to_owned());
            }
            "rk_error_summary" => self.error_summary = Some(value.to_owned()),
            name if !name.starts_with("rk_") => {
                self.attributes.insert(
                    name.to_owned(),
                    replaykit_core_model::Value::Text(value.to_owned()),
                );
            }
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "rk_input_tokens" => self.input_tokens = value,
            "rk_output_tokens" => self.output_tokens = value,
            "rk_cost_micros" => self.cost_micros = value,
            name if !name.starts_with("rk_") => {
                self.attributes.insert(
                    name.to_owned(),
                    replaykit_core_model::Value::Int(value as i64),
                );
            }
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !field.name().starts_with("rk_") {
            self.attributes.insert(
                field.name().to_owned(),
                replaykit_core_model::Value::Int(value),
            );
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !field.name().starts_with("rk_") {
            self.attributes.insert(
                field.name().to_owned(),
                replaykit_core_model::Value::Bool(value),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Update visitor – for on_record (values recorded after span creation)
// ---------------------------------------------------------------------------

struct UpdateVisitor {
    output_fingerprint: Option<String>,
    error_summary: Option<String>,
    status: Option<SpanStatus>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cost_micros: Option<u64>,
}

impl UpdateVisitor {
    fn new() -> Self {
        Self {
            output_fingerprint: None,
            error_summary: None,
            status: None,
            input_tokens: None,
            output_tokens: None,
            cost_micros: None,
        }
    }
}

impl Visit for UpdateVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "rk_output_fingerprint" => self.output_fingerprint = Some(value.to_owned()),
            "rk_error_summary" => self.error_summary = Some(value.to_owned()),
            "rk_status" => self.status = parse_span_status(value),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "rk_input_tokens" => self.input_tokens = Some(value),
            "rk_output_tokens" => self.output_tokens = Some(value),
            "rk_cost_micros" => self.cost_micros = Some(value),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// ReplayKitLayer
// ---------------------------------------------------------------------------

/// A [`tracing_subscriber::Layer`] that intercepts spans carrying `rk_kind`
/// fields and emits them as ReplayKit semantic spans through a [`Sink`].
///
/// Spans without an `rk_kind` field are silently ignored.
///
/// # Usage
///
/// ```ignore
/// use tracing_subscriber::prelude::*;
///
/// let layer = session.layer(Arc::new(SequentialClock::new(1)));
/// tracing_subscriber::registry().with(layer).init();
///
/// // This span will be captured:
/// let _span = tracing::info_span!("planner", rk_kind = "PlannerStep").entered();
///
/// // This one is ignored (no rk_kind):
/// let _span = tracing::info_span!("internal_log").entered();
/// ```
pub struct ReplayKitLayer {
    sink: Arc<dyn Sink>,
    clock: Arc<dyn Clock>,
}

impl ReplayKitLayer {
    pub fn new(sink: Arc<dyn Sink>, clock: Arc<dyn Clock>) -> Self {
        Self { sink, clock }
    }
}

impl<S> Layer<S> for ReplayKitLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = SpanFieldVisitor::new();
        attrs.record(&mut visitor);

        let Some(kind) = visitor.kind else {
            return; // Not a ReplayKit span.
        };

        // Resolve parent ReplayKit span id (if parent exists and is tracked).
        let parent_span_id = ctx
            .span(id)
            .and_then(|span_ref| span_ref.parent())
            .and_then(|parent| {
                parent
                    .extensions()
                    .get::<SpanData>()
                    .map(|d| d.rk_span_id.clone())
            });

        let started_at = self.clock.now();
        let span_name = ctx
            .span(id)
            .map(|s| s.name().to_owned())
            .unwrap_or_default();

        let spec = OpenSpanSpec {
            span_id: visitor.span_id.map(SpanId),
            parent_span_id,
            kind,
            name: span_name,
            started_at,
            replay_policy: visitor.replay_policy.unwrap_or(ReplayPolicy::RecordOnly),
            executor_kind: visitor.executor_kind,
            executor_version: visitor.executor_version,
            input_fingerprint: visitor.input_fingerprint,
            environment_fingerprint: visitor.environment_fingerprint,
            attributes: visitor.attributes,
        };

        if let Ok(record) = self.sink.open_span(spec)
            && let Some(span_ref) = ctx.span(id)
        {
            span_ref.extensions_mut().insert(SpanData {
                rk_span_id: record.span_id,
                started_at,
                output_fingerprint: visitor.output_fingerprint,
                error_summary: visitor.error_summary,
                status: SpanStatus::Completed,
                input_tokens: visitor.input_tokens,
                output_tokens: visitor.output_tokens,
                cost_micros: visitor.cost_micros,
            });
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span_ref) = ctx.span(id) else {
            return;
        };
        let mut extensions = span_ref.extensions_mut();
        let Some(data) = extensions.get_mut::<SpanData>() else {
            return;
        };

        let mut visitor = UpdateVisitor::new();
        values.record(&mut visitor);

        if let Some(fp) = visitor.output_fingerprint {
            data.output_fingerprint = Some(fp);
        }
        if let Some(err) = visitor.error_summary {
            data.error_summary = Some(err);
        }
        if let Some(status) = visitor.status {
            data.status = status;
        }
        if let Some(t) = visitor.input_tokens {
            data.input_tokens = t;
        }
        if let Some(t) = visitor.output_tokens {
            data.output_tokens = t;
        }
        if let Some(c) = visitor.cost_micros {
            data.cost_micros = c;
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span_ref) = ctx.span(&id) else {
            return;
        };
        let extensions = span_ref.extensions();
        let Some(data) = extensions.get::<SpanData>() else {
            return;
        };

        let ended_at = self.clock.now();

        let _ = self.sink.close_span(
            &data.rk_span_id,
            CloseSpanSpec {
                ended_at,
                status: data.status,
                output_fingerprint: data.output_fingerprint.clone(),
                error_summary: data.error_summary.clone(),
                cost: CostMetrics {
                    input_tokens: data.input_tokens,
                    output_tokens: data.output_tokens,
                    estimated_cost_micros: data.cost_micros,
                },
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

fn parse_span_kind(s: &str) -> Option<SpanKind> {
    match s {
        "Run" => Some(SpanKind::Run),
        "PlannerStep" => Some(SpanKind::PlannerStep),
        "LlmCall" => Some(SpanKind::LlmCall),
        "ToolCall" => Some(SpanKind::ToolCall),
        "ShellCommand" => Some(SpanKind::ShellCommand),
        "FileRead" => Some(SpanKind::FileRead),
        "FileWrite" => Some(SpanKind::FileWrite),
        "BrowserAction" => Some(SpanKind::BrowserAction),
        "Retrieval" => Some(SpanKind::Retrieval),
        "MemoryLookup" => Some(SpanKind::MemoryLookup),
        "HumanInput" => Some(SpanKind::HumanInput),
        "GuardrailCheck" => Some(SpanKind::GuardrailCheck),
        "Subgraph" => Some(SpanKind::Subgraph),
        "AdapterInternal" => Some(SpanKind::AdapterInternal),
        _ => None,
    }
}

fn parse_replay_policy(s: &str) -> Option<ReplayPolicy> {
    match s {
        "RecordOnly" => Some(ReplayPolicy::RecordOnly),
        "RerunnableSupported" => Some(ReplayPolicy::RerunnableSupported),
        "CacheableIfFingerprintMatches" => Some(ReplayPolicy::CacheableIfFingerprintMatches),
        "PureReusable" => Some(ReplayPolicy::PureReusable),
        _ => None,
    }
}

fn parse_span_status(s: &str) -> Option<SpanStatus> {
    match s {
        "Completed" => Some(SpanStatus::Completed),
        "Failed" => Some(SpanStatus::Failed),
        "Skipped" => Some(SpanStatus::Skipped),
        "Blocked" => Some(SpanStatus::Blocked),
        "Canceled" => Some(SpanStatus::Canceled),
        _ => None,
    }
}
