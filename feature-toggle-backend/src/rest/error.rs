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
    #[error("Self approval not allowed")]
    SelfApprovalNotAllowed { message: String },
    #[error("Last admin required")]
    LastAdminRequired { message: String },
    #[error("SSO managed")]
    SsoManaged { message: String },
    #[error("SSO user has no local password")]
    SsoUserNoLocalPassword { message: String },
    #[error("Enforce SSO requires a local admin")]
    EnforceSsoRequiresLocalAdmin { message: String },
    #[error("Encryption key missing")]
    EncryptionKeyMissing { message: String },
    #[error("Invalid refresh token")]
    InvalidRefreshToken { message: String },
    #[error("Refresh token reused")]
    RefreshTokenReused { message: String },
    #[error("SSO required")]
    SsoRequired { message: String },
    #[error("Invalid SSO code")]
    InvalidSsoCode { message: String },
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

    /// 403 `self_approval_not_allowed`: a requester tried to approve their own request.
    pub fn self_approval_not_allowed() -> Self {
        Self::SelfApprovalNotAllowed {
            message: "Requesters cannot approve their own request".to_string(),
        }
    }

    /// 409 `last_admin_required`: the update would disable or demote the last
    /// enabled administrator.
    pub fn last_admin_required() -> Self {
        Self::LastAdminRequired {
            message: "At least one enabled administrator must remain".to_string(),
        }
    }

    /// 409 `sso_managed`: the role or team assignment comes from SSO group sync and
    /// cannot be removed by hand.
    pub fn sso_managed() -> Self {
        Self::SsoManaged {
            message: "This assignment is managed by SSO group sync".to_string(),
        }
    }

    /// 409 `enforce_sso_requires_local_admin`: SSO cannot be enforced while no
    /// enabled break-glass admin (not granted by SSO, with a password) exists.
    pub fn enforce_sso_requires_local_admin() -> Self {
        Self::EnforceSsoRequiresLocalAdmin {
            message: "Enforcing SSO requires at least one enabled administrator who was not granted admin by SSO and has a password".to_string(),
        }
    }

    /// 400 `sso_user_no_local_password`: SSO users have no local password to reset.
    pub fn sso_user_no_local_password() -> Self {
        Self::SsoUserNoLocalPassword {
            message: "SSO users sign in through their identity provider and have no local password"
                .to_string(),
        }
    }

    /// 400 `encryption_key_missing`: a client secret was supplied but
    /// `FLUXGATE_ENCRYPTION_KEY` is not configured.
    pub fn encryption_key_missing() -> Self {
        Self::EncryptionKeyMissing {
            message: "FLUXGATE_ENCRYPTION_KEY must be set to store a client secret".to_string(),
        }
    }

    /// 401 `invalid_refresh_token`: unknown or expired refresh token, or its user
    /// is missing or disabled.
    pub fn invalid_refresh_token() -> Self {
        Self::InvalidRefreshToken {
            message: "Refresh token is invalid or expired".to_string(),
        }
    }

    /// 401 `refresh_token_reused`: an already revoked refresh token was presented
    /// and its token family has been revoked.
    pub fn refresh_token_reused() -> Self {
        Self::RefreshTokenReused {
            message: "Refresh token was already used; the session has been revoked".to_string(),
        }
    }

    /// 403 `sso_required`: while SSO is enforced, password login is disabled for
    /// everyone except break-glass admins (admins not granted by SSO).
    pub fn sso_required() -> Self {
        Self::SsoRequired {
            message: "Sign in with single sign-on".to_string(),
        }
    }

    /// 401 `invalid_sso_code`: the one-time SSO code is unknown, expired or used.
    pub fn invalid_sso_code() -> Self {
        Self::InvalidSsoCode {
            message: "SSO login code is invalid or expired".to_string(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
            code: None,
            details: None,
        }
    }

    /// 403 with `code: "policy_denied"`, the shape `JwtGuard` uses for policy denials.
    pub fn policy_denied(message: impl Into<String>) -> Self {
        Self::Forbidden {
            message: message.into(),
            code: Some("policy_denied".to_string()),
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
            Self::SelfApprovalNotAllowed { .. } => "self_approval_not_allowed",
            Self::LastAdminRequired { .. } => "last_admin_required",
            Self::EncryptionKeyMissing { .. } => "encryption_key_missing",
            Self::SsoManaged { .. } => "sso_managed",
            Self::EnforceSsoRequiresLocalAdmin { .. } => "enforce_sso_requires_local_admin",
            Self::SsoUserNoLocalPassword { .. } => "sso_user_no_local_password",
            Self::InvalidRefreshToken { .. } => "invalid_refresh_token",
            Self::RefreshTokenReused { .. } => "refresh_token_reused",
            Self::SsoRequired { .. } => "sso_required",
            Self::InvalidSsoCode { .. } => "invalid_sso_code",
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
            | Self::AccountDisabled { message }
            | Self::SelfApprovalNotAllowed { message }
            | Self::LastAdminRequired { message }
            | Self::SsoManaged { message }
            | Self::EnforceSsoRequiresLocalAdmin { message }
            | Self::EncryptionKeyMissing { message }
            | Self::SsoUserNoLocalPassword { message }
            | Self::InvalidRefreshToken { message }
            | Self::RefreshTokenReused { message }
            | Self::SsoRequired { message }
            | Self::InvalidSsoCode { message } => message,
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
            Self::AccountDisabled { .. }
            | Self::SelfApprovalNotAllowed { .. }
            | Self::LastAdminRequired { .. }
            | Self::SsoManaged { .. }
            | Self::EnforceSsoRequiresLocalAdmin { .. }
            | Self::SsoUserNoLocalPassword { .. }
            | Self::EncryptionKeyMissing { .. }
            | Self::InvalidRefreshToken { .. }
            | Self::RefreshTokenReused { .. }
            | Self::SsoRequired { .. }
            | Self::InvalidSsoCode { .. } => None,
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
            Self::AccountDisabled { .. }
            | Self::SelfApprovalNotAllowed { .. }
            | Self::LastAdminRequired { .. }
            | Self::SsoManaged { .. }
            | Self::EnforceSsoRequiresLocalAdmin { .. }
            | Self::SsoUserNoLocalPassword { .. }
            | Self::EncryptionKeyMissing { .. }
            | Self::InvalidRefreshToken { .. }
            | Self::RefreshTokenReused { .. }
            | Self::SsoRequired { .. }
            | Self::InvalidSsoCode { .. } => None,
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
            Self::Conflict { .. }
            | Self::LastAdminRequired { .. }
            | Self::SsoManaged { .. }
            | Self::EnforceSsoRequiresLocalAdmin { .. } => StatusCode::CONFLICT,
            Self::InvalidInput { .. }
            | Self::SsoUserNoLocalPassword { .. }
            | Self::EncryptionKeyMissing { .. } => StatusCode::BAD_REQUEST,
            Self::Unauthorized { .. }
            | Self::AccountDisabled { .. }
            | Self::InvalidRefreshToken { .. }
            | Self::RefreshTokenReused { .. }
            | Self::InvalidSsoCode { .. } => StatusCode::UNAUTHORIZED,
            Self::Forbidden { .. }
            | Self::SelfApprovalNotAllowed { .. }
            | Self::SsoRequired { .. } => StatusCode::FORBIDDEN,
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
            crate::Error::SelfApprovalNotAllowed => RestError::self_approval_not_allowed(),
            crate::Error::LastAdminRequired => RestError::last_admin_required(),
            crate::Error::SsoManaged => RestError::sso_managed(),
            crate::Error::SsoUserNoLocalPassword => RestError::sso_user_no_local_password(),
            crate::Error::EnforceSsoRequiresLocalAdmin => {
                RestError::enforce_sso_requires_local_admin()
            }
        }
    }
}

impl From<crate::logic::sso_provider::SsoAdminError> for RestError {
    fn from(err: crate::logic::sso_provider::SsoAdminError) -> Self {
        use crate::logic::sso_provider::SsoAdminError;
        match err {
            SsoAdminError::Invalid(message) => RestError::invalid_input(message),
            SsoAdminError::EncryptionKeyMissing => RestError::encryption_key_missing(),
            SsoAdminError::Other(inner) => RestError::from(inner),
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
    async fn last_admin_required_maps_to_409_with_last_admin_required_error() {
        let err = RestError::from(crate::Error::LastAdminRequired);
        assert_eq!(err.status_code(), StatusCode::CONFLICT);

        let resp = err.error_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = to_bytes(resp.into_body()).await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["error"], "last_admin_required");
    }

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

    #[actix_web::test]
    async fn refresh_token_errors_map_to_401_with_their_error_codes() {
        for (err, code) in [
            (RestError::invalid_refresh_token(), "invalid_refresh_token"),
            (RestError::refresh_token_reused(), "refresh_token_reused"),
        ] {
            let resp = err.error_response();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            let body = to_bytes(resp.into_body()).await.expect("body");
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
            assert_eq!(json["error"], code);
        }
    }

    #[actix_web::test]
    async fn sso_errors_map_to_their_status_and_error_codes() {
        use crate::logic::sso_provider::SsoAdminError;
        let cases = [
            (
                RestError::from(crate::Error::SsoManaged),
                StatusCode::CONFLICT,
                "sso_managed",
            ),
            (
                RestError::from(crate::Error::SsoUserNoLocalPassword),
                StatusCode::BAD_REQUEST,
                "sso_user_no_local_password",
            ),
            (
                RestError::from(crate::Error::EnforceSsoRequiresLocalAdmin),
                StatusCode::CONFLICT,
                "enforce_sso_requires_local_admin",
            ),
            (
                RestError::from(SsoAdminError::EncryptionKeyMissing),
                StatusCode::BAD_REQUEST,
                "encryption_key_missing",
            ),
            (
                RestError::from(SsoAdminError::invalid("bad")),
                StatusCode::BAD_REQUEST,
                "invalid_input",
            ),
        ];
        for (err, status, code) in cases {
            let resp = err.error_response();
            assert_eq!(resp.status(), status);
            let body = to_bytes(resp.into_body()).await.expect("body");
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
            assert_eq!(json["error"], code);
        }
    }
}
