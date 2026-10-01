use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    pub code: Option<String>,
    pub details: Option<Value>,
}

impl ErrorResponse {
    pub fn new(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            code: None,
            details: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RestError {
    #[error("Not found")]
    NotFound {
        message: String,
        code: Option<String>,
        details: Option<Value>,
    },
    #[error("Invalid input")]
    InvalidInput {
        message: String,
        code: Option<String>,
        details: Option<Value>,
    },
    #[error("Conflict")]
    Conflict {
        message: String,
        code: Option<String>,
        details: Option<Value>,
    },
    #[error("Unauthorized")]
    Unauthorized {
        message: String,
        code: Option<String>,
        details: Option<Value>,
    },
    #[error("Account disabled")]
    AccountDisabled { message: String },
    #[error("Forbidden")]
    Forbidden {
        message: String,
        code: Option<String>,
        details: Option<Value>,
    },
    #[error("Internal server error")]
    Internal {
        message: String,
        code: Option<String>,
        details: Option<Value>,
    },
}

impl RestError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub fn account_disabled(message: impl Into<String>) -> Self {
        Self::AccountDisabled {
            message: message.into(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    fn error_key(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::InvalidInput { .. } => "invalid_input",
            Self::Conflict { .. } => "conflict",
            Self::Unauthorized { .. } => "unauthorized",
            Self::AccountDisabled { .. } => "account_disabled",
            Self::Forbidden { .. } => "forbidden",
            Self::Internal { .. } => "internal",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::NotFound { message, .. }
            | Self::InvalidInput { message, .. }
            | Self::Conflict { message, .. }
            | Self::Unauthorized { message, .. }
            | Self::Forbidden { message, .. }
            | Self::Internal { message, .. }
            | Self::AccountDisabled { message } => message,
        }
    }

    fn code(&self) -> Option<&str> {
        match self {
            Self::NotFound { code, .. }
            | Self::InvalidInput { code, .. }
            | Self::Conflict { code, .. }
            | Self::Unauthorized { code, .. }
            | Self::Forbidden { code, .. }
            | Self::Internal { code, .. } => code.as_deref(),
            Self::AccountDisabled { .. } => None,
        }
    }

    fn details(&self) -> Option<&Value> {
        match self {
            Self::NotFound { details, .. }
            | Self::InvalidInput { details, .. }
            | Self::Conflict { details, .. }
            | Self::Unauthorized { details, .. }
            | Self::Forbidden { details, .. }
            | Self::Internal { details, .. } => details.as_ref(),
            Self::AccountDisabled { .. } => None,
        }
    }

    fn to_error_response(&self) -> ErrorResponse {
        ErrorResponse {
            error: self.error_key().to_string(),
            message: self.message().to_string(),
            code: self.code().map(|c| c.to_string()),
            details: self.details().cloned(),
        }
    }
}

impl ResponseError for RestError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::InvalidInput { .. } => StatusCode::BAD_REQUEST,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Unauthorized { .. } | Self::AccountDisabled { .. } => StatusCode::UNAUTHORIZED,
            Self::Forbidden { .. } => StatusCode::FORBIDDEN,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code()).json(self.to_error_response())
    }
}

impl From<crate::Error> for RestError {
    fn from(err: crate::Error) -> Self {
        match err {
            crate::Error::NotFound(id) => {
                RestError::not_found(format!("Record not found for id {id}"))
            }
            crate::Error::DatabaseError(_) => RestError::internal("Internal server error"),
            crate::Error::RecordAlreadyExists(msg) => RestError::conflict(msg),
            crate::Error::InvalidInput(msg) => RestError::invalid_input(msg),
            crate::Error::Unauthorized(msg) => RestError::unauthorized(msg),
            crate::Error::AccountDisabled => RestError::account_disabled("Account is disabled"),
        }
    }
}

impl From<sqlx::Error> for RestError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => RestError::not_found("Record not found"),
            _ => RestError::internal("Internal server error"),
        }
    }
}

impl From<crate::logic::metrics::MetricLogicError> for RestError {
    fn from(err: crate::logic::metrics::MetricLogicError) -> Self {
        match err {
            crate::logic::metrics::MetricLogicError::InvalidInput(msg) => {
                RestError::invalid_input(msg)
            }
            crate::logic::metrics::MetricLogicError::NotFound(msg) => RestError::not_found(msg),
            crate::logic::metrics::MetricLogicError::RecordAlreadyExists(msg) => {
                RestError::conflict(msg)
            }
            crate::logic::metrics::MetricLogicError::Unauthenticated(msg) => {
                RestError::unauthorized(msg)
            }
            crate::logic::metrics::MetricLogicError::PermissionDenied(msg) => {
                RestError::forbidden(msg)
            }
            crate::logic::metrics::MetricLogicError::Database(_) => {
                RestError::internal("Internal server error")
            }
        }
    }
}

impl From<crate::logic::feature_evaluation::FeatureEvaluationLogicError> for RestError {
    fn from(err: crate::logic::feature_evaluation::FeatureEvaluationLogicError) -> Self {
        match err {
            crate::logic::feature_evaluation::FeatureEvaluationLogicError::InvalidInput(msg) => {
                RestError::invalid_input(msg)
            }
            crate::logic::feature_evaluation::FeatureEvaluationLogicError::NotFound => {
                RestError::not_found("Record not found")
            }
            crate::logic::feature_evaluation::FeatureEvaluationLogicError::DatabaseError(_) => {
                RestError::internal("Internal server error")
            }
        }
    }
}

impl From<crate::logic::canary::CanaryLogicError> for RestError {
    fn from(err: crate::logic::canary::CanaryLogicError) -> Self {
        match err {
            crate::logic::canary::CanaryLogicError::InvalidInput(msg) => {
                RestError::invalid_input(msg)
            }
            crate::logic::canary::CanaryLogicError::NotFound(msg) => RestError::not_found(msg),
            crate::logic::canary::CanaryLogicError::Feature(inner) => RestError::from(inner),
            crate::logic::canary::CanaryLogicError::Metrics(inner) => RestError::from(inner),
            crate::logic::canary::CanaryLogicError::Database(_) => {
                RestError::internal("Internal server error")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::body::to_bytes;

    #[actix_web::test]
    async fn account_disabled_maps_to_401_with_account_disabled_error() {
        let err = RestError::from(crate::Error::AccountDisabled);
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);

        let resp = err.error_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(resp.into_body()).await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["error"], "account_disabled");
    }
}
