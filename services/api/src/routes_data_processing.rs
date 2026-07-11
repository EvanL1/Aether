//! Authenticated HTTP interface for the shared Data Processing application API.
//!
//! This module is deliberately transport-only: callers select a commissioned
//! task/binding identity and typed options. Source coordinates, complete
//! frames, processor endpoints, and artifact activation never cross this API.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_application::{
    Actor, ApplicationError, DATA_PROCESSING_AUDIT_FINALIZATION_TIMEOUT_MS,
    DataProcessingTaskSummary, RequestContext,
};
use aether_data_processing::{MEDIA_TYPE, encode_derived_data};
use aether_domain::{
    BindingIdentity, FeatureRole, FeatureValueType, ForecastOptions, HistoryAggregation,
    HistoryDuplicatePolicy, ProcessTaskRequest, ProcessingOptions, TaskIdentity, TimestampMs,
};
use aether_ports::{DataBoundary, PortErrorKind, ProcessorHealth};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Extension, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

const REQUEST_ID_HEADER: &str = "x-request-id";
const CONFIRMED_HEADER: &str = "x-aether-confirmed";
const MAX_PROCESS_BODY_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_HORIZON_STEPS: usize = 4_096;
const MAX_QUANTILES: usize = 19;

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/processors/health", get(processor_health))
        .route(
            "/process",
            post(process).layer(DefaultBodyLimit::max(MAX_PROCESS_BODY_BYTES)),
        )
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_ms: Option<u64>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retryable: Option<bool>,
    retry_after_ms: Option<u64>,
}

impl ApiError {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retryable: None,
            retry_after_ms: None,
        }
    }

    const fn invalid_request() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "INVALID_PROCESS_REQUEST",
            "the data-processing request is invalid",
        )
    }

    fn from_json_rejection(rejection: JsonRejection) -> Self {
        match rejection.into_response().status() {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "JSON_CONTENT_TYPE_REQUIRED",
                "a JSON content type is required",
            ),
            StatusCode::PAYLOAD_TOO_LARGE => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "PROCESS_REQUEST_TOO_LARGE",
                "the data-processing request body exceeds its limit",
            ),
            _ => Self::invalid_request(),
        }
    }
}

impl From<ApplicationError> for ApiError {
    fn from(error: ApplicationError) -> Self {
        match error {
            ApplicationError::PermissionDenied { .. } => Self::new(
                StatusCode::FORBIDDEN,
                "DATA_PROCESSING_PERMISSION_DENIED",
                "the authenticated actor is not allowed to perform this operation",
            ),
            ApplicationError::ConfirmationRequired { .. } => Self::new(
                StatusCode::PRECONDITION_REQUIRED,
                "DATA_PROCESSING_CONFIRMATION_REQUIRED",
                "explicit confirmation is required for this processing route",
            ),
            ApplicationError::InvalidProcessingRequest(_) => Self::invalid_request(),
            ApplicationError::InputQualityRejected(_) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "DATA_PROCESSING_INPUT_QUALITY_REJECTED",
                message: "current source data does not satisfy the commissioned task policy",
                retryable: Some(true),
                retry_after_ms: None,
            },
            ApplicationError::InvalidProcessingConfiguration(_) => Self::new(
                StatusCode::NOT_FOUND,
                "DATA_PROCESSING_ROUTE_NOT_FOUND",
                "the requested task and binding revision are not commissioned",
            ),
            ApplicationError::InvalidProcessorResult(_) => Self::new(
                StatusCode::BAD_GATEWAY,
                "INVALID_PROCESSOR_RESULT",
                "the processor returned data that failed Aether validation",
            ),
            ApplicationError::ProcessingUnavailable {
                retryable,
                retry_after_ms,
                ..
            } => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "PROCESSING_UNAVAILABLE",
                message: "no acceptable derived data is currently available",
                retryable: Some(retryable),
                retry_after_ms,
            },
            ApplicationError::HistoryQueryFailed(error)
            | ApplicationError::CovariateSourceFailed(error)
            | ApplicationError::ProcessorFailed(error)
            | ApplicationError::Port(error) => {
                let (status, code) = match error.kind() {
                    PortErrorKind::Timeout => {
                        (StatusCode::GATEWAY_TIMEOUT, "DATA_PROCESSING_TIMEOUT")
                    },
                    PortErrorKind::InvalidData
                    | PortErrorKind::Permanent
                    | PortErrorKind::Rejected => (
                        StatusCode::BAD_GATEWAY,
                        "DATA_PROCESSING_DEPENDENCY_REJECTED",
                    ),
                    PortErrorKind::Unavailable | PortErrorKind::Conflict => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "DATA_PROCESSING_DEPENDENCY_UNAVAILABLE",
                    ),
                };
                Self::new(
                    status,
                    code,
                    "a required data-processing dependency is unavailable",
                )
            },
            ApplicationError::ProcessingRequestTooLarge { .. } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "PROCESSOR_REQUEST_TOO_LARGE",
                "the assembled processor request exceeds the commissioned limit",
            ),
            ApplicationError::AuditUnavailable(_) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "DATA_PROCESSING_AUDIT_UNAVAILABLE",
                "mandatory processing audit is unavailable",
            ),
            ApplicationError::ProcessingCodec(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATA_PROCESSING_ENCODING_FAILED",
                "the data-processing response could not be encoded",
            ),
            ApplicationError::InvalidCommand(_) => Self::invalid_request(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorDetail {
                    code: self.code,
                    message: self.message,
                    retryable: self.retryable,
                    retry_after_ms: self.retry_after_ms,
                },
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
struct IdentityResponse {
    id: String,
    revision: u32,
}

#[derive(Debug, Serialize)]
struct FeatureResponse {
    name: String,
    role: &'static str,
    value_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum: Option<f64>,
    integer: bool,
}

#[derive(Debug, Serialize)]
struct TargetResponse {
    name: String,
    unit: String,
    sign_convention: String,
}

#[derive(Debug, Serialize)]
struct FallbackResponse {
    strategy: String,
    version: String,
    source_feature: String,
    max_output_age_ms: u64,
}

#[derive(Debug, Serialize)]
struct ForecastPolicyResponse {
    target: TargetResponse,
    cadence_ms: u64,
    history_aggregation: &'static str,
    history_duplicate_policy: &'static str,
    history_feature_policies: Vec<HistoryFeaturePolicyResponse>,
    history_steps: usize,
    max_horizon_steps: usize,
    max_quantiles: usize,
    max_output_age_ms: u64,
    max_missing_ratio: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_input_age_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_gap_ms: Option<u64>,
    require_future_issue_time: bool,
    allowed_fallbacks: Vec<String>,
    fallback_policies: Vec<FallbackResponse>,
}

#[derive(Debug, Serialize)]
struct HistoryFeaturePolicyResponse {
    feature: String,
    aggregation: &'static str,
    duplicate_policy: &'static str,
}

#[derive(Debug, Serialize)]
struct ArtifactResponse {
    kind: String,
    family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
}

#[derive(Debug, Serialize)]
struct TaskResponse {
    task: IdentityResponse,
    binding: IdentityResponse,
    kind: &'static str,
    processor_contract: String,
    features: Vec<FeatureResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forecast: Option<ForecastPolicyResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact: Option<ArtifactResponse>,
    processor_id: String,
    processor_version: String,
    data_boundary: &'static str,
    deadline_ms: u64,
    audit_finalization_timeout_ms: u64,
    max_concurrency: usize,
    max_frame_samples: usize,
    max_request_bytes: usize,
}

impl From<&DataProcessingTaskSummary> for TaskResponse {
    fn from(summary: &DataProcessingTaskSummary) -> Self {
        let definition = summary.definition();
        Self {
            task: IdentityResponse {
                id: summary.task().id().to_owned(),
                revision: summary.task().revision(),
            },
            binding: IdentityResponse {
                id: summary.binding().id().to_owned(),
                revision: summary.binding().revision(),
            },
            kind: "forecast",
            processor_contract: summary.processor_contract().to_owned(),
            features: definition
                .features()
                .iter()
                .map(|feature| FeatureResponse {
                    name: feature.name().to_owned(),
                    role: match feature.role() {
                        FeatureRole::History => "history",
                        FeatureRole::FutureCovariate => "future_covariate",
                        FeatureRole::Static => "static",
                    },
                    value_type: match feature.value_type() {
                        FeatureValueType::Number => "number",
                        FeatureValueType::Text => "text",
                        FeatureValueType::Boolean => "boolean",
                    },
                    unit: feature.unit().map(str::to_owned),
                    minimum: feature
                        .numeric_constraints()
                        .and_then(|constraints| constraints.minimum()),
                    maximum: feature
                        .numeric_constraints()
                        .and_then(|constraints| constraints.maximum()),
                    integer: feature
                        .numeric_constraints()
                        .is_some_and(|constraints| constraints.integer()),
                })
                .collect(),
            forecast: definition
                .forecast_spec()
                .map(|specification| ForecastPolicyResponse {
                    target: TargetResponse {
                        name: specification.target().name().to_owned(),
                        unit: specification.target().unit().to_owned(),
                        sign_convention: specification.target().sign_convention().to_owned(),
                    },
                    cadence_ms: specification.cadence_ms(),
                    history_aggregation: match specification.history_aggregation() {
                        HistoryAggregation::Mean => "mean",
                        HistoryAggregation::Last => "last",
                        HistoryAggregation::Sum => "sum",
                        HistoryAggregation::Min => "min",
                        HistoryAggregation::Max => "max",
                    },
                    history_duplicate_policy: duplicate_policy_name(
                        specification.history_duplicate_policy(),
                    ),
                    history_feature_policies: specification
                        .history_feature_policies()
                        .iter()
                        .map(|policy| HistoryFeaturePolicyResponse {
                            feature: policy.feature().to_owned(),
                            aggregation: aggregation_name(policy.aggregation()),
                            duplicate_policy: duplicate_policy_name(policy.duplicate_policy()),
                        })
                        .collect(),
                    history_steps: specification.history_steps(),
                    max_horizon_steps: specification.max_horizon_steps(),
                    max_quantiles: specification.max_quantiles(),
                    max_output_age_ms: specification.max_output_age_ms(),
                    max_missing_ratio: specification.max_missing_ratio(),
                    max_input_age_ms: specification.max_input_age_ms(),
                    max_gap_ms: specification.max_gap_ms(),
                    require_future_issue_time: specification.requires_future_issue_time(),
                    allowed_fallbacks: specification.allowed_fallbacks().to_vec(),
                    fallback_policies: specification
                        .fallback_policies()
                        .iter()
                        .map(|fallback| FallbackResponse {
                            strategy: fallback.strategy().to_owned(),
                            version: fallback.version().to_owned(),
                            source_feature: fallback.source_feature().to_owned(),
                            max_output_age_ms: fallback.max_output_age_ms(),
                        })
                        .collect(),
                }),
            artifact: summary.artifact().map(|artifact| ArtifactResponse {
                kind: artifact.kind().to_owned(),
                family: artifact.family().to_owned(),
                version: artifact.version().map(str::to_owned),
                digest: artifact.digest().map(str::to_owned),
            }),
            processor_id: summary.processor_id().to_owned(),
            processor_version: summary.processor_version().to_owned(),
            data_boundary: match summary.data_boundary() {
                DataBoundary::Local => "local",
                DataBoundary::Remote => "remote",
            },
            deadline_ms: summary.deadline_ms(),
            audit_finalization_timeout_ms: DATA_PROCESSING_AUDIT_FINALIZATION_TIMEOUT_MS,
            max_concurrency: summary.max_concurrency(),
            max_frame_samples: summary.max_frame_samples(),
            max_request_bytes: summary.max_request_bytes(),
        }
    }
}

const fn aggregation_name(value: HistoryAggregation) -> &'static str {
    match value {
        HistoryAggregation::Mean => "mean",
        HistoryAggregation::Last => "last",
        HistoryAggregation::Sum => "sum",
        HistoryAggregation::Min => "min",
        HistoryAggregation::Max => "max",
    }
}

const fn duplicate_policy_name(value: HistoryDuplicatePolicy) -> &'static str {
    match value {
        HistoryDuplicatePolicy::Latest => "latest",
        HistoryDuplicatePolicy::Reject => "reject",
    }
}

#[derive(Debug, Serialize)]
struct TasksResponse {
    schema: &'static str,
    tasks: Vec<TaskResponse>,
}

#[derive(Debug, Serialize)]
struct ProcessorHealthResponse {
    processor_id: String,
    health: &'static str,
}

#[derive(Debug, Serialize)]
struct ProcessorsHealthResponse {
    schema: &'static str,
    processors: Vec<ProcessorHealthResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessRequestBody {
    task_id: String,
    expected_task_revision: u32,
    binding_id: String,
    expected_binding_revision: u32,
    as_of: String,
    options: ProcessOptionsBody,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ProcessOptionsBody {
    Forecast {
        horizon_steps: usize,
        #[serde(default, deserialize_with = "deserialize_quantiles")]
        quantiles: Option<Vec<f64>>,
    },
}

fn deserialize_quantiles<'de, D>(deserializer: D) -> Result<Option<Vec<f64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<f64>::deserialize(deserializer).map(Some)
}

impl ProcessRequestBody {
    fn into_domain(self) -> Result<ProcessTaskRequest, ApiError> {
        if !valid_identifier(&self.task_id)
            || !valid_identifier(&self.binding_id)
            || self.expected_task_revision == 0
            || self.expected_binding_revision == 0
        {
            return Err(ApiError::invalid_request());
        }
        let task = TaskIdentity::new(self.task_id, self.expected_task_revision)
            .map_err(|_| ApiError::invalid_request())?;
        let binding = BindingIdentity::new(self.binding_id, self.expected_binding_revision)
            .map_err(|_| ApiError::invalid_request())?;
        let as_of = parse_utc_timestamp(&self.as_of)?;
        let options = match self.options {
            ProcessOptionsBody::Forecast {
                horizon_steps,
                quantiles,
            } => {
                if horizon_steps == 0 || horizon_steps > MAX_HORIZON_STEPS {
                    return Err(ApiError::invalid_request());
                }
                let quantiles = match quantiles {
                    Some(quantiles) if quantiles.is_empty() || quantiles.len() > MAX_QUANTILES => {
                        return Err(ApiError::invalid_request());
                    },
                    Some(quantiles) => quantiles,
                    None => Vec::new(),
                };
                ProcessingOptions::Forecast(
                    ForecastOptions::new(horizon_steps, quantiles)
                        .map_err(|_| ApiError::invalid_request())?,
                )
            },
        };
        Ok(ProcessTaskRequest::new(task, binding, as_of, options))
    }
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
) -> Result<Json<TasksResponse>, ApiError> {
    let application = processing_application(&state)?;
    let context = request_context(&claims, &headers, false)?;
    let tasks = application.list_tasks(&context).await?;
    Ok(Json(TasksResponse {
        schema: "aether.data-processing.tasks.v1",
        tasks: tasks.iter().map(TaskResponse::from).collect(),
    }))
}

async fn processor_health(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
) -> Result<Json<ProcessorsHealthResponse>, ApiError> {
    let application = processing_application(&state)?;
    let context = request_context(&claims, &headers, false)?;
    let processors = application.processor_health(&context).await?;
    Ok(Json(ProcessorsHealthResponse {
        schema: "aether.data-processing.processors-health.v1",
        processors: processors
            .iter()
            .map(|processor| ProcessorHealthResponse {
                processor_id: processor.processor_id().to_owned(),
                health: match processor.health() {
                    ProcessorHealth::Healthy => "healthy",
                    ProcessorHealth::Degraded => "degraded",
                    ProcessorHealth::Unavailable => "unavailable",
                },
            })
            .collect(),
    }))
}

async fn process(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
    payload: Result<Json<ProcessRequestBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(payload) = payload.map_err(ApiError::from_json_rejection)?;
    let confirmed = parse_confirmation(&headers)?;
    let context = request_context(&claims, &headers, confirmed)?;
    let request = payload.into_domain()?;
    let application = processing_application(&state)?;
    let derived = application.process(&context, request).await?;
    let encoded = encode_derived_data(&derived).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DATA_PROCESSING_ENCODING_FAILED",
            "the data-processing response could not be encoded",
        )
    })?;
    let mut response = Body::from(encoded).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
    Ok(response)
}

fn processing_application(
    state: &AppState,
) -> Result<Arc<aether_application::DataProcessingApplication>, ApiError> {
    state.data_processing.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "DATA_PROCESSING_DISABLED",
            "data processing is not enabled on this deployment",
        )
    })
}

fn request_context(
    claims: &Claims,
    headers: &HeaderMap,
    confirmed: bool,
) -> Result<RequestContext, ApiError> {
    let request_id = request_id(headers)?;
    let mut actor = Actor::new(format!("user:{}", claims.user_id));
    if matches!(
        claims.role.as_deref(),
        Some("Viewer" | "Engineer" | "Admin")
    ) {
        actor = actor.with_permission("data_processing.read");
    }
    if matches!(claims.role.as_deref(), Some("Engineer" | "Admin")) {
        actor = actor.with_permission("data_processing.run");
    }
    Ok(RequestContext::new(
        request_id,
        actor,
        confirmed,
        current_timestamp()?,
    ))
}

fn request_id(headers: &HeaderMap) -> Result<String, ApiError> {
    let mut values = headers.get_all(REQUEST_ID_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(Uuid::new_v4().to_string());
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_request());
    }
    let value = value.to_str().map_err(|_| ApiError::invalid_request())?;
    if value.is_empty()
        || value.len() > MAX_REQUEST_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(ApiError::invalid_request());
    }
    Ok(value.to_owned())
}

fn parse_confirmation(headers: &HeaderMap) -> Result<bool, ApiError> {
    let mut values = headers.get_all(CONFIRMED_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(false);
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_request());
    }
    match value.to_str() {
        Ok("true") => Ok(true),
        Ok("false") => Ok(false),
        _ => Err(ApiError::invalid_request()),
    }
}

fn current_timestamp() -> Result<TimestampMs, ApiError> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SYSTEM_CLOCK_UNAVAILABLE",
            "the system clock is unavailable",
        )
    })?;
    let milliseconds = u64::try_from(duration.as_millis()).map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SYSTEM_CLOCK_UNAVAILABLE",
            "the system clock is unavailable",
        )
    })?;
    Ok(TimestampMs::new(milliseconds))
}

fn valid_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    value.len() <= MAX_IDENTIFIER_BYTES
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn parse_utc_timestamp(value: &str) -> Result<TimestampMs, ApiError> {
    let bytes = value.as_bytes();
    let fixed_digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let fixed_shape_is_valid = bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && fixed_digits
            .iter()
            .all(|index| bytes.get(*index).is_some_and(u8::is_ascii_digit));
    let lexical_shape_is_valid = fixed_shape_is_valid
        && match bytes.len() {
            20 => bytes.get(19) == Some(&b'Z'),
            22..=24 => {
                bytes.get(19) == Some(&b'.')
                    && bytes.last() == Some(&b'Z')
                    && bytes[20..bytes.len() - 1].iter().all(u8::is_ascii_digit)
            },
            _ => false,
        };
    if !lexical_shape_is_valid {
        return Err(ApiError::invalid_request());
    }
    let parsed =
        chrono::DateTime::parse_from_rfc3339(value).map_err(|_| ApiError::invalid_request())?;
    let milliseconds =
        u64::try_from(parsed.timestamp_millis()).map_err(|_| ApiError::invalid_request())?;
    Ok(TimestampMs::new(milliseconds))
}

#[cfg(test)]
mod tests {
    use aether_application::{DataProcessingApplication, SafetyPolicy};
    use aether_store_local::{ManualClock, MemoryAuditSink, MemoryHistoryQuery, MemoryLiveState};
    use axum::extract::{Extension, State};
    use axum::response::IntoResponse;

    use super::*;
    use crate::test_support::{app_state, authorization_headers};

    fn claims(role: &str) -> Claims {
        Claims {
            user_id: 42,
            username: "processing-test".to_owned(),
            role: Some(role.to_owned()),
            token_id: None,
            exp: usize::MAX,
            iat: 0,
            token_type: "access".to_owned(),
        }
    }

    fn empty_application() -> Arc<DataProcessingApplication> {
        Arc::new(
            DataProcessingApplication::new(
                Vec::new(),
                Arc::new(MemoryHistoryQuery::new()),
                None,
                Arc::new(MemoryLiveState::new()),
                Arc::new(MemoryAuditSink::new()),
                Arc::new(ManualClock::new(TimestampMs::new(1))),
                SafetyPolicy,
            )
            .expect("empty application is valid for interface tests"),
        )
    }

    async fn enabled_state() -> Arc<AppState> {
        let mut state = app_state().await;
        Arc::get_mut(&mut state)
            .expect("test state is uniquely owned")
            .data_processing = Some(empty_application());
        state
    }

    fn valid_body() -> ProcessRequestBody {
        serde_json::from_value(serde_json::json!({
            "task_id": "energy.site-load-forecast",
            "expected_task_revision": 1,
            "binding_id": "energy.example-site",
            "expected_binding_revision": 1,
            "as_of": "1970-01-01T00:00:01Z",
            "options": {
                "kind": "forecast",
                "horizon_steps": 2,
                "quantiles": [0.1, 0.5, 0.9]
            }
        }))
        .expect("valid process body")
    }

    #[tokio::test]
    async fn viewer_can_read_discovery_but_cannot_run_processing() {
        let state = enabled_state().await;
        let headers = authorization_headers("Viewer");

        let discovery = list_tasks(
            State(Arc::clone(&state)),
            Extension(claims("Viewer")),
            headers.clone(),
        )
        .await
        .expect("Viewer has read permission");
        assert!(discovery.tasks.is_empty());

        let denied = process(
            State(state),
            Extension(claims("Viewer")),
            headers,
            Ok(Json(valid_body())),
        )
        .await
        .expect_err("Viewer must not have run permission")
        .into_response();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn engineer_and_admin_receive_run_permission() {
        for role in ["Engineer", "Admin"] {
            let state = enabled_state().await;
            let response = process(
                State(state),
                Extension(claims(role)),
                authorization_headers(role),
                Ok(Json(valid_body())),
            )
            .await
            .expect_err("empty commissioned routes must fail after authorization")
            .into_response();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn disabled_state_has_no_processing_application() {
        let state = app_state().await;
        assert!(state.data_processing.is_none());
        assert!(
            crate::commissioned_data_processing_router(&state).is_none(),
            "disabled Data Processing must not mount an HTTP router"
        );
        let response = list_tasks(
            State(state),
            Extension(claims("Admin")),
            authorization_headers("Admin"),
        )
        .await
        .expect_err("disabled processing must fail closed")
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn request_body_is_strict_and_versioned() {
        let unknown = serde_json::from_value::<ProcessRequestBody>(serde_json::json!({
            "task_id": "energy.site-load-forecast",
            "expected_task_revision": 1,
            "binding_id": "energy.example-site",
            "expected_binding_revision": 1,
            "as_of": "2026-07-11T12:00:00Z",
            "processor_endpoint": "https://attacker.invalid",
            "options": {"kind": "forecast", "horizon_steps": 2}
        }));
        assert!(unknown.is_err());

        let unknown_option = serde_json::from_value::<ProcessRequestBody>(serde_json::json!({
            "task_id": "energy.site-load-forecast",
            "expected_task_revision": 1,
            "binding_id": "energy.example-site",
            "expected_binding_revision": 1,
            "as_of": "2026-07-11T12:00:00Z",
            "options": {"kind": "forecast", "horizon_steps": 2, "frame": {}}
        }));
        assert!(unknown_option.is_err());

        let explicit_null = serde_json::from_value::<ProcessRequestBody>(serde_json::json!({
            "task_id": "energy.site-load-forecast",
            "expected_task_revision": 1,
            "binding_id": "energy.example-site",
            "expected_binding_revision": 1,
            "as_of": "2026-07-11T12:00:00Z",
            "options": {"kind": "forecast", "horizon_steps": 2, "quantiles": null}
        }));
        assert!(explicit_null.is_err());

        let empty_quantiles = serde_json::from_value::<ProcessRequestBody>(serde_json::json!({
            "task_id": "energy.site-load-forecast",
            "expected_task_revision": 1,
            "binding_id": "energy.example-site",
            "expected_binding_revision": 1,
            "as_of": "2026-07-11T12:00:00Z",
            "options": {"kind": "forecast", "horizon_steps": 2, "quantiles": []}
        }))
        .expect("array shape is decoded before semantic validation");
        assert!(empty_quantiles.into_domain().is_err());

        let stale_identity = serde_json::from_value::<ProcessRequestBody>(serde_json::json!({
            "task_id": "energy.site-load-forecast",
            "expected_task_revision": 0,
            "binding_id": "energy.example-site",
            "expected_binding_revision": 1,
            "as_of": "2026-07-11T12:00:00Z",
            "options": {"kind": "forecast", "horizon_steps": 2}
        }))
        .expect("revision semantics are validated when converting to the domain");
        assert!(stale_identity.into_domain().is_err());
    }

    #[test]
    fn as_of_requires_bounded_utc_rfc3339() {
        assert_eq!(
            parse_utc_timestamp("2026-07-11T12:00:00.123Z")
                .expect("millisecond UTC timestamp")
                .get(),
            1_783_771_200_123
        );
        for invalid in [
            "2026-07-11T12:00:00+08:00",
            "2026-07-11T12:00:00.1234Z",
            "2026-07-11 12:00:00Z",
        ] {
            assert!(parse_utc_timestamp(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn confirmation_header_is_explicit_and_request_ids_are_safe() {
        let mut headers = HeaderMap::new();
        assert!(!parse_confirmation(&headers).expect("missing means false"));
        headers.insert(CONFIRMED_HEADER, HeaderValue::from_static("true"));
        assert!(parse_confirmation(&headers).expect("explicit true"));
        headers.insert(CONFIRMED_HEADER, HeaderValue::from_static("yes"));
        assert!(parse_confirmation(&headers).is_err());

        headers.insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_static("line-break-free:1"),
        );
        assert_eq!(
            request_id(&headers).expect("safe request id"),
            "line-break-free:1"
        );
        headers.insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_static("contains space"),
        );
        assert!(request_id(&headers).is_err());
    }
}
