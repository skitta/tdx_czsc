pub mod analyze;
pub mod health;
pub mod version;

use anyhow::{Result, anyhow};

use crate::proto::analysis::{RequestEnvelope, ResponseEnvelope};
use crate::proto::analysis::request_envelope::Body as RequestBody;
use crate::proto::analysis::response_envelope::Body as ResponseBody;
use crate::state::AppState;

pub async fn handle_request(
    state: &std::sync::Arc<AppState>,
    request: RequestEnvelope,
) -> Result<ResponseEnvelope> {
    let body = request
        .body
        .ok_or_else(|| anyhow!("request body is missing"))?;

    let protocol_version = state.config.protocol_version.clone();
    let request_id = request.request_id;

    let response_body = match body {
        RequestBody::Analyze(req) => ResponseBody::Analyze(analyze::handle(state, req).await?),
        RequestBody::Health(req) => ResponseBody::Health(health::handle(state, req).await?),
        RequestBody::Version(req) => ResponseBody::Version(version::handle(state, req).await?),
    };

    Ok(ResponseEnvelope {
        protocol_version,
        request_id,
        status: crate::proto::analysis::StatusCode::StatusOk as i32,
        error_message: String::new(),
        body: Some(response_body),
    })
}

pub fn build_error_response(
    state: &AppState,
    request_id: u64,
    status: crate::proto::analysis::StatusCode,
    error_message: impl Into<String>,
) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: state.config.protocol_version.clone(),
        request_id,
        status: status as i32,
        error_message: error_message.into(),
        body: None,
    }
}
