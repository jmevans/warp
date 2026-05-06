use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Client;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::provider_registry::{
    AuthConfig, AuthSource, DiscoveredModel, DiscoveredModelSource, ProviderCapabilities,
    ProviderKind, ProviderProfile,
};

/// Validation error classified by failure mode.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ValidationError {
    #[error("Authentication failed — {detail}")]
    AuthFailure { detail: String, http_status: u16 },

    #[error("Provider endpoint unreachable — {detail}")]
    EndpointUnreachable { detail: String },

    #[error("Request timed out")]
    Timeout,

    #[error("Invalid response schema — {detail}")]
    InvalidSchema { detail: String },

    #[error("Unsupported capability — {detail}")]
    UnsupportedCapability { detail: String },

    #[error("Rate limited by provider — {detail}")]
    RateLimit {
        detail: String,
        retry_after_secs: Option<u64>,
    },

    #[error("Stream interrupted — {detail}")]
    StreamInterrupted { detail: String },

    #[error("Unknown provider error — {detail}")]
    UnknownProviderError { detail: String, http_status: u16 },
}

impl ValidationError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::EndpointUnreachable { .. }
                | Self::Timeout
                | Self::StreamInterrupted { .. }
                | Self::RateLimit { .. }
        )
    }
}

/// Result of a validation attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResult {
    pub status: ValidationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ValidationError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_models: Option<Vec<DiscoveredModelInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ValidationStatus {
    Success,
    Failed,
    Partial,
}

/// Minimal model info returned from discovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModelInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// The level of validation to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationLevel {
    /// Quick check: GET /v1/models or equivalent to verify connectivity and auth.
    Cheap,
    /// Deep check: send a minimal inference request to verify the model works end-to-end.
    Deep,
}

/// Provider validator that can validate profiles and discover models.
pub struct ProviderValidator {
    client: Client,
}

impl ProviderValidator {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Resolve the actual credential value for an auth config.
    /// Callers pass a resolver closure that maps secret refs / env vars to values.
    pub async fn validate(
        &self,
        profile: &ProviderProfile,
        level: ValidationLevel,
        resolve_secret: impl Fn(&str) -> Option<String>,
    ) -> ValidationResult {
        let auth_value = resolve_auth(&profile.auth, &resolve_secret);

        let start = instant::Instant::now();

        let result = match profile.kind {
            ProviderKind::WarpHosted => ValidationResult {
                status: ValidationStatus::Success,
                error: None,
                discovered_models: None,
                response_time_ms: None,
                model: None,
            },
            ProviderKind::OpenAI => {
                validate_openai(&self.client, &profile.base_url, &auth_value, level).await
            }
            ProviderKind::Anthropic => {
                validate_anthropic(&self.client, &profile.base_url, &auth_value, level).await
            }
            ProviderKind::OpenAICompatible => {
                validate_openai_compatible(&self.client, &profile.base_url, &auth_value, level)
                    .await
            }
            ProviderKind::AnthropicCompatible => {
                validate_anthropic_compatible(&self.client, &profile.base_url, &auth_value, level)
                    .await
            }
        };

        ValidationResult {
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            ..result
        }
    }

    /// Discover available models from the provider's /v1/models endpoint.
    pub async fn discover_models(
        &self,
        profile: &ProviderProfile,
        resolve_secret: impl Fn(&str) -> Option<String>,
    ) -> Result<Vec<DiscoveredModel>, ValidationError> {
        let auth_value = resolve_auth(&profile.auth, &resolve_secret);

        let models = match &profile.kind {
            ProviderKind::WarpHosted => {
                return Ok(profile.discovered_models.clone());
            }
            ProviderKind::OpenAI | ProviderKind::OpenAICompatible => {
                discover_openai_models(&self.client, &profile.base_url, &auth_value).await?
            }
            ProviderKind::Anthropic | ProviderKind::AnthropicCompatible => {
                discover_anthropic_models(&self.client, &profile.base_url, &auth_value).await?
            }
        };

        let source = if matches!(
            profile.discovery.strategy,
            crate::provider_registry::DiscoveryStrategy::ManualOnly
        ) {
            DiscoveredModelSource::Manual
        } else {
            DiscoveredModelSource::Discovered
        };

        Ok(models
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.id,
                display_name: m.display_name,
                source: source.clone(),
                capabilities: ProviderCapabilities::default(),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Auth resolution
// ---------------------------------------------------------------------------

struct ResolvedAuthValue {
    kind: AuthValueKind,
    value: Option<String>,
}

enum AuthValueKind {
    None,
    ApiKey,
    BearerToken,
}

fn resolve_auth(auth: &AuthConfig, resolve: impl Fn(&str) -> Option<String>) -> ResolvedAuthValue {
    match auth {
        AuthConfig::None => ResolvedAuthValue {
            kind: AuthValueKind::None,
            value: None,
        },
        AuthConfig::ApiKey {
            source: AuthSource::Keychain,
            secret_ref,
            ..
        } => ResolvedAuthValue {
            kind: AuthValueKind::ApiKey,
            value: secret_ref.as_ref().and_then(|r| resolve(r)),
        },
        AuthConfig::ApiKey {
            source: AuthSource::Env,
            env_var,
            ..
        } => ResolvedAuthValue {
            kind: AuthValueKind::ApiKey,
            value: env_var.as_ref().and_then(|v| resolve(v)),
        },
        AuthConfig::BearerToken { secret_ref, .. } => ResolvedAuthValue {
            kind: AuthValueKind::BearerToken,
            value: Some(resolve(secret_ref)).flatten(),
        },
    }
}

// ---------------------------------------------------------------------------
// OpenAI validation
// ---------------------------------------------------------------------------

async fn validate_openai(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
    level: ValidationLevel,
) -> ValidationResult {
    // Cheap: GET /v1/models
    match cheap_validate_openai_models(client, base_url, auth).await {
        Ok(models) => {
            if matches!(level, ValidationLevel::Deep) {
                // Deep: minimal POST /v1/responses
                match deep_validate_openai_responses(client, base_url, auth).await {
                    Ok(model) => ValidationResult {
                        status: ValidationStatus::Success,
                        error: None,
                        discovered_models: Some(models),
                        response_time_ms: None,
                        model: Some(model),
                    },
                    Err(e) => ValidationResult {
                        status: ValidationStatus::Partial,
                        error: Some(e),
                        discovered_models: Some(models),
                        response_time_ms: None,
                        model: None,
                    },
                }
            } else {
                ValidationResult {
                    status: ValidationStatus::Success,
                    error: None,
                    discovered_models: Some(models),
                    response_time_ms: None,
                    model: None,
                }
            }
        }
        Err(e) => ValidationResult {
            status: ValidationStatus::Failed,
            error: Some(e),
            discovered_models: None,
            response_time_ms: None,
            model: None,
        },
    }
}

async fn cheap_validate_openai_models(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
) -> Result<Vec<DiscoveredModelInfo>, ValidationError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let mut request = client.get(&url);
    request = apply_bearer_auth(request, auth);

    let response = request
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_http_error(status, &body));
    }

    let body: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ValidationError::InvalidSchema {
                detail: format!("Failed to parse models response: {e}"),
            })?;

    let models: Vec<DiscoveredModelInfo> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let id = entry.get("id")?.as_str()?.to_string();
                    let display_name = entry
                        .get("object")
                        .and_then(|o| o.as_str())
                        .map(String::from);
                    Some(DiscoveredModelInfo { id, display_name })
                })
                .collect()
        })
        .unwrap_or_default();

    if models.is_empty() {
        return Err(ValidationError::InvalidSchema {
            detail: "No models found in response".to_string(),
        });
    }

    Ok(models)
}

async fn deep_validate_openai_responses(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
) -> Result<String, ValidationError> {
    let url = format!("{}/responses", base_url.trim_end_matches('/'));

    let body = serde_json::json!({
        "model": "gpt-4o-mini",
        "input": "Say OK",
        "max_output_tokens": 1,
    });

    let mut request = client.post(&url).json(&body);
    request = apply_bearer_auth(request, auth);

    let response = request
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(classify_http_error(status, &error_body));
    }

    let resp: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ValidationError::InvalidSchema {
                detail: format!("Failed to parse responses response: {e}"),
            })?;

    let model = resp
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("gpt-4o-mini")
        .to_string();

    Ok(model)
}

// ---------------------------------------------------------------------------
// Anthropic validation
// ---------------------------------------------------------------------------

async fn validate_anthropic(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
    level: ValidationLevel,
) -> ValidationResult {
    match cheap_validate_anthropic_models(client, base_url, auth).await {
        Ok(models) => {
            if matches!(level, ValidationLevel::Deep) {
                match deep_validate_anthropic_messages(client, base_url, auth).await {
                    Ok(model) => ValidationResult {
                        status: ValidationStatus::Success,
                        error: None,
                        discovered_models: Some(models),
                        response_time_ms: None,
                        model: Some(model),
                    },
                    Err(e) => ValidationResult {
                        status: ValidationStatus::Partial,
                        error: Some(e),
                        discovered_models: Some(models),
                        response_time_ms: None,
                        model: None,
                    },
                }
            } else {
                ValidationResult {
                    status: ValidationStatus::Success,
                    error: None,
                    discovered_models: Some(models),
                    response_time_ms: None,
                    model: None,
                }
            }
        }
        Err(e) => ValidationResult {
            status: ValidationStatus::Failed,
            error: Some(e),
            discovered_models: None,
            response_time_ms: None,
            model: None,
        },
    }
}

async fn cheap_validate_anthropic_models(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
) -> Result<Vec<DiscoveredModelInfo>, ValidationError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let mut request = client.get(&url);
    request = apply_anthropic_auth(request, auth);

    let response = request
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(classify_http_error(status, &body));
    }

    let body: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ValidationError::InvalidSchema {
                detail: format!("Failed to parse models response: {e}"),
            })?;

    let models: Vec<DiscoveredModelInfo> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let id = entry.get("id")?.as_str()?.to_string();
                    let display_name = entry
                        .get("display_name")
                        .and_then(|o| o.as_str())
                        .map(String::from);
                    Some(DiscoveredModelInfo { id, display_name })
                })
                .collect()
        })
        .unwrap_or_default();

    if models.is_empty() {
        return Err(ValidationError::InvalidSchema {
            detail: "No models found in response".to_string(),
        });
    }

    Ok(models)
}

async fn deep_validate_anthropic_messages(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
) -> Result<String, ValidationError> {
    let url = format!("{}/messages", base_url.trim_end_matches('/'));

    let body = serde_json::json!({
        "model": "claude-3-haiku-20240307",
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "OK"}],
    });

    let mut request = client.post(&url).json(&body);
    request = apply_anthropic_auth(request, auth);
    request = request.header("anthropic-version", "2023-06-01");

    let response = request
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(classify_http_error(status, &error_body));
    }

    let resp: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ValidationError::InvalidSchema {
                detail: format!("Failed to parse messages response: {e}"),
            })?;

    let model = resp
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("claude-3-haiku-20240307")
        .to_string();

    Ok(model)
}

// ---------------------------------------------------------------------------
// OpenAI-compatible validation
// ---------------------------------------------------------------------------

async fn validate_openai_compatible(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
    level: ValidationLevel,
) -> ValidationResult {
    if matches!(level, ValidationLevel::Cheap) {
        return match discover_openai_models(client, base_url, auth).await {
            Ok(models) => ValidationResult {
                status: ValidationStatus::Success,
                error: None,
                discovered_models: Some(
                    models
                        .into_iter()
                        .map(|m| DiscoveredModelInfo {
                            id: m.id,
                            display_name: m.display_name,
                        })
                        .collect(),
                ),
                response_time_ms: None,
                model: None,
            },
            Err(error) => ValidationResult {
                status: ValidationStatus::Partial,
                error: Some(error),
                discovered_models: None,
                response_time_ms: None,
                model: None,
            },
        };
    }

    let models = (discover_openai_models(client, base_url, auth).await).ok(); // Compatible providers may not support /models — that's OK

    // Deep check: minimal POST /v1/chat/completions
    let deep_result =
        deep_validate_openai_compatible_chat(client, base_url, auth, models.as_ref()).await;

    match deep_result {
        Ok(model) => ValidationResult {
            status: ValidationStatus::Success,
            error: None,
            discovered_models: models.map(|m| {
                m.into_iter()
                    .map(|m| DiscoveredModelInfo {
                        id: m.id,
                        display_name: m.display_name,
                    })
                    .collect()
            }),
            response_time_ms: None,
            model: Some(model),
        },
        Err(e) => {
            // If models were discovered but deep validation failed, mark partial
            let status = if models.is_some() {
                ValidationStatus::Partial
            } else {
                ValidationStatus::Failed
            };
            ValidationResult {
                status,
                error: Some(e),
                discovered_models: models.map(|m| {
                    m.into_iter()
                        .map(|m| DiscoveredModelInfo {
                            id: m.id,
                            display_name: m.display_name,
                        })
                        .collect()
                }),
                response_time_ms: None,
                model: None,
            }
        }
    }
}

async fn deep_validate_openai_compatible_chat(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
    known_models: Option<&Vec<DiscoveredModelInfo>>,
) -> Result<String, ValidationError> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    // Use a known model if available, otherwise try a common default
    let model = known_models
        .and_then(|m| m.first())
        .map(|m| m.id.clone())
        .unwrap_or_else(|| "gpt-3.5-turbo".to_string());

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "OK"}],
        "max_tokens": 1,
    });

    let mut request = client.post(&url).json(&body);
    request = apply_bearer_auth(request, auth);

    let response = request
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(classify_http_error(status, &error_body));
    }

    let resp: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ValidationError::InvalidSchema {
                detail: format!("Failed to parse chat completions response: {e}"),
            })?;

    let actual_model = resp
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or(&model)
        .to_string();

    Ok(actual_model)
}

// ---------------------------------------------------------------------------
// Anthropic-compatible validation
// ---------------------------------------------------------------------------

async fn validate_anthropic_compatible(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
    level: ValidationLevel,
) -> ValidationResult {
    if matches!(level, ValidationLevel::Cheap) {
        return match discover_anthropic_models(client, base_url, auth).await {
            Ok(models) => ValidationResult {
                status: ValidationStatus::Success,
                error: None,
                discovered_models: Some(
                    models
                        .into_iter()
                        .map(|m| DiscoveredModelInfo {
                            id: m.id,
                            display_name: m.display_name,
                        })
                        .collect(),
                ),
                response_time_ms: None,
                model: None,
            },
            Err(error) => ValidationResult {
                status: ValidationStatus::Partial,
                error: Some(error),
                discovered_models: None,
                response_time_ms: None,
                model: None,
            },
        };
    }

    let models = (discover_anthropic_models(client, base_url, auth).await).ok();

    let deep_result =
        deep_validate_anthropic_compatible_messages(client, base_url, auth, models.as_ref()).await;

    match deep_result {
        Ok(model) => ValidationResult {
            status: ValidationStatus::Success,
            error: None,
            discovered_models: models.map(|m| {
                m.into_iter()
                    .map(|m| DiscoveredModelInfo {
                        id: m.id,
                        display_name: m.display_name,
                    })
                    .collect()
            }),
            response_time_ms: None,
            model: Some(model),
        },
        Err(e) => {
            let status = if models.is_some() {
                ValidationStatus::Partial
            } else {
                ValidationStatus::Failed
            };
            ValidationResult {
                status,
                error: Some(e),
                discovered_models: models.map(|m| {
                    m.into_iter()
                        .map(|m| DiscoveredModelInfo {
                            id: m.id,
                            display_name: m.display_name,
                        })
                        .collect()
                }),
                response_time_ms: None,
                model: None,
            }
        }
    }
}

async fn deep_validate_anthropic_compatible_messages(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
    known_models: Option<&Vec<DiscoveredModelInfo>>,
) -> Result<String, ValidationError> {
    let url = format!("{}/messages", base_url.trim_end_matches('/'));

    let model = known_models
        .and_then(|m| m.first())
        .map(|m| m.id.clone())
        .unwrap_or_else(|| "claude-3-haiku-20240307".to_string());

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "OK"}],
    });

    let mut request = client.post(&url).json(&body);
    request = apply_anthropic_auth(request, auth);
    request = request.header("anthropic-version", "2023-06-01");

    let response = request
        .send()
        .await
        .map_err(|e| classify_reqwest_error(&e))?;

    let status = response.status().as_u16();
    if !response.status().is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(classify_http_error(status, &error_body));
    }

    let resp: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| ValidationError::InvalidSchema {
                detail: format!("Failed to parse messages response: {e}"),
            })?;

    let actual_model = resp
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or(&model)
        .to_string();

    Ok(actual_model)
}

// ---------------------------------------------------------------------------
// Model discovery (shared helpers)
// ---------------------------------------------------------------------------

async fn discover_openai_models(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
) -> Result<Vec<DiscoveredModelInfo>, ValidationError> {
    cheap_validate_openai_models(client, base_url, auth).await
}

async fn discover_anthropic_models(
    client: &Client,
    base_url: &str,
    auth: &ResolvedAuthValue,
) -> Result<Vec<DiscoveredModelInfo>, ValidationError> {
    cheap_validate_anthropic_models(client, base_url, auth).await
}

// ---------------------------------------------------------------------------
// Auth application helpers
// ---------------------------------------------------------------------------

fn apply_bearer_auth(
    mut builder: reqwest::RequestBuilder,
    auth: &ResolvedAuthValue,
) -> reqwest::RequestBuilder {
    match (&auth.kind, &auth.value) {
        (AuthValueKind::BearerToken, Some(token)) | (AuthValueKind::ApiKey, Some(token)) => {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        _ => {}
    }
    builder
}

fn apply_anthropic_auth(
    mut builder: reqwest::RequestBuilder,
    auth: &ResolvedAuthValue,
) -> reqwest::RequestBuilder {
    match (&auth.kind, &auth.value) {
        (AuthValueKind::BearerToken, Some(token)) => {
            builder = builder.header("x-api-key", token);
        }
        (AuthValueKind::ApiKey, Some(key)) => {
            builder = builder.header("x-api-key", key);
        }
        _ => {}
    }
    builder
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

fn classify_reqwest_error(e: &reqwest::Error) -> ValidationError {
    if e.is_timeout() {
        ValidationError::Timeout
    } else {
        ValidationError::EndpointUnreachable {
            detail: e.to_string(),
        }
    }
}

fn classify_http_error(status: u16, body: &str) -> ValidationError {
    // Try to extract error message from common provider error shapes
    let detail = extract_error_detail(body);

    match status {
        401 | 403 => ValidationError::AuthFailure {
            detail,
            http_status: status,
        },
        429 => {
            let retry_after_secs = None; // Could parse Retry-After header later
            ValidationError::RateLimit {
                detail,
                retry_after_secs,
            }
        }
        500..=599 => ValidationError::UnknownProviderError {
            detail,
            http_status: status,
        },
        _ => ValidationError::UnknownProviderError {
            detail,
            http_status: status,
        },
    }
}

fn extract_error_detail(body: &str) -> String {
    // Try common error response shapes
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        // OpenAI: { "error": { "message": "..." } }
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
        // Anthropic: { "error": { "type": "...", "message": "..." } }
        // Generic: { "message": "..." }
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
        // Generic: { "error": "..." }
        if let Some(msg) = json.get("error").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    // Fallback: truncate body to reasonable length
    if body.len() > 500 {
        format!("{}... (truncated)", &body[..500])
    } else {
        body.to_string()
    }
}

// ---------------------------------------------------------------------------
// Timestamp wrapper for validation cache
// ---------------------------------------------------------------------------

/// Validation status cached on a provider profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidationCacheEntry {
    pub last_validated: DateTime<Utc>,
    pub status: ValidationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ValidationError>,
    #[serde(default)]
    pub retry_count: u32,
}

impl ValidationCacheEntry {
    pub fn is_stale(&self, max_age: Duration) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.last_validated)
            .to_std()
            .unwrap_or(Duration::MAX);
        elapsed > max_age
    }

    #[allow(dead_code)]
    pub fn from_validation_result(result: &ValidationResult) -> Self {
        Self {
            last_validated: Utc::now(),
            status: result.status.clone(),
            error: result.error.clone(),
            retry_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_retryable_classification() {
        assert!(ValidationError::Timeout.is_retryable());
        assert!(ValidationError::EndpointUnreachable {
            detail: "test".to_string()
        }
        .is_retryable());
        assert!(ValidationError::StreamInterrupted {
            detail: "test".to_string()
        }
        .is_retryable());
        assert!(ValidationError::RateLimit {
            detail: "test".to_string(),
            retry_after_secs: None
        }
        .is_retryable());
        assert!(!ValidationError::AuthFailure {
            detail: "bad key".to_string(),
            http_status: 401
        }
        .is_retryable());
        assert!(!ValidationError::InvalidSchema {
            detail: "bad json".to_string()
        }
        .is_retryable());
    }

    #[test]
    fn classify_http_errors() {
        let err = classify_http_error(401, r#"{"error":{"message":"Invalid API key"}}"#);
        assert!(matches!(
            err,
            ValidationError::AuthFailure {
                http_status: 401,
                ..
            }
        ));

        let err = classify_http_error(429, r#"{"error":{"message":"Rate limit exceeded"}}"#);
        assert!(matches!(err, ValidationError::RateLimit { .. }));

        let err = classify_http_error(500, "Internal server error");
        assert!(matches!(
            err,
            ValidationError::UnknownProviderError {
                http_status: 500,
                ..
            }
        ));
    }

    #[test]
    fn extract_error_detail_openai() {
        let body =
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#;
        assert_eq!(extract_error_detail(body), "Incorrect API key provided");
    }

    #[test]
    fn extract_error_detail_anthropic() {
        let body = r#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(extract_error_detail(body), "invalid x-api-key");
    }

    #[test]
    fn extract_error_detail_generic() {
        let body = "Something went wrong";
        assert_eq!(extract_error_detail(body), "Something went wrong");
    }

    #[test]
    fn extract_error_detail_long_body() {
        let body = "x".repeat(600);
        let detail = extract_error_detail(&body);
        assert!(detail.contains("... (truncated)"));
        assert!(detail.len() < 600);
    }
}
