//! Uniform JSON error responses: `{"error": {"code": "...", "message": "..."}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: "invalid_request", message: message.into() }
    }

    pub fn not_found(id: &str) -> Self {
        Self { status: StatusCode::NOT_FOUND, code: "job_not_found", message: format!("no job with id `{id}`") }
    }

    pub fn not_finished() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "job_not_finished",
            message: "the job is still running; poll GET /api/v1/jobs/{id} until its status is `completed`".into(),
        }
    }

    pub fn failed(message: &str) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, code: "job_failed", message: message.to_string() }
    }

    pub fn too_many_jobs(message: impl Into<String>) -> Self {
        Self { status: StatusCode::TOO_MANY_REQUESTS, code: "too_many_jobs", message: message.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: ErrorDetail { code: self.code.into(), message: self.message } };
        (self.status, Json(body)).into_response()
    }
}
