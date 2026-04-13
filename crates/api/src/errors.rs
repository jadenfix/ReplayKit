use std::fmt;

use replaykit_collector::CollectorError;
use replaykit_diff_engine::DiffError;
use replaykit_replay_engine::ReplayError;
use replaykit_storage::StorageError;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    InvalidInput,
    Conflict,
    ReplayBlocked,
    InvalidPatch,
    IntegrityError,
    IncompatibleExecutor,
    StorageUnavailable,
    Internal,
}

// ---------------------------------------------------------------------------
// API error body (serialized as JSON in HTTP responses)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct ApiErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ApiErrorBody {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::NotFound,
            message: message.into(),
            details: None,
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidInput,
            message: message.into(),
            details: None,
        }
    }

    pub fn replay_blocked(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::ReplayBlocked,
            message: message.into(),
            details: None,
        }
    }

    pub fn invalid_patch(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidPatch,
            message: message.into(),
            details: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Internal,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn http_status(&self) -> u16 {
        match self.code {
            ErrorCode::NotFound => 404,
            ErrorCode::InvalidInput | ErrorCode::InvalidPatch => 400,
            ErrorCode::Conflict => 409,
            ErrorCode::ReplayBlocked | ErrorCode::IncompatibleExecutor => 422,
            ErrorCode::IntegrityError | ErrorCode::StorageUnavailable | ErrorCode::Internal => 500,
        }
    }
}

// ---------------------------------------------------------------------------
// ApiError (internal, converts to ApiErrorBody for HTTP)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ApiError {
    Storage(StorageError),
    Collector(CollectorError),
    Replay(ReplayError),
    Diff(DiffError),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Storage(e) => write!(f, "{e}"),
            ApiError::Collector(e) => write!(f, "{e}"),
            ApiError::Replay(e) => write!(f, "{e}"),
            ApiError::Diff(e) => write!(f, "{e:?}"),
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(value: StorageError) -> Self {
        Self::Storage(value)
    }
}

impl From<CollectorError> for ApiError {
    fn from(value: CollectorError) -> Self {
        Self::Collector(value)
    }
}

impl From<ReplayError> for ApiError {
    fn from(value: ReplayError) -> Self {
        Self::Replay(value)
    }
}

impl From<DiffError> for ApiError {
    fn from(value: DiffError) -> Self {
        Self::Diff(value)
    }
}

impl From<ApiError> for ApiErrorBody {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Storage(StorageError::NotFound(msg)) => ApiErrorBody::not_found(msg),
            ApiError::Storage(StorageError::Conflict(msg)) => ApiErrorBody {
                code: ErrorCode::Conflict,
                message: msg,
                details: None,
            },
            ApiError::Storage(StorageError::InvalidInput(msg)) => ApiErrorBody::invalid_input(msg),
            ApiError::Storage(StorageError::Internal(msg)) => ApiErrorBody {
                code: ErrorCode::StorageUnavailable,
                message: msg,
                details: None,
            },
            ApiError::Collector(CollectorError::Storage(se)) => {
                ApiErrorBody::from(ApiError::Storage(se))
            }
            ApiError::Collector(CollectorError::InvalidInput(msg)) => {
                ApiErrorBody::invalid_input(msg)
            }
            ApiError::Replay(ReplayError::Storage(se)) => ApiErrorBody::from(ApiError::Storage(se)),
            ApiError::Replay(ReplayError::InvalidPatch(msg)) => ApiErrorBody::invalid_patch(msg),
            ApiError::Replay(ReplayError::Blocked(msg)) => ApiErrorBody::replay_blocked(msg),
            ApiError::Diff(DiffError::Storage(se)) => ApiErrorBody::from(ApiError::Storage(se)),
        }
    }
}
