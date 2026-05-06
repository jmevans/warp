use std::sync::Arc;

use ai::api_keys::ApiKeys;
use ai::provider_registry::{
    infer_local_only, ProviderDefaults, ProviderKind, ProviderResolutionError, ResolvedAuth,
    ResolvedProvider,
};
use anyhow::anyhow;
use futures_util::stream::BoxStream;
use futures_util::StreamExt as _;
use instant::Instant;
use reqwest::StatusCode;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use warp_multi_agent_api as api;

use crate::ai::agent::api::McpToolExecutor;
use crate::ai::agent::AIAgentInput;
use crate::server::server_api::AIApiError;
use crate::server::telemetry::TelemetryEvent;

use super::{ConvertToAPITypeError, RequestParams, ResponseStream};

/// Serialized snapshot of a previous exchange for direct-provider message history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentExchangeSnapshot {
    pub user_query: String,
    pub assistant_text: String,
    /// Images attached to the user query, serialized as base64 + mime type.
    #[serde(default)]
    pub user_images: Vec<SerializedImage>,
}

/// Base64-encoded image with mime type for provider message building.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SerializedImage {
    pub data_base64: String,
    pub mime_type: String,
}

/// Metadata passed to the direct-provider stream for telemetry emission.
#[derive(Clone)]
pub(super) struct DirectProviderTelemetry {
    pub user_id: Option<String>,
    pub anonymous_id: String,
    pub start_time: Instant,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub model: String,
}

/// Outcome of a direct-provider request, captured for telemetry.
#[derive(Clone)]
pub(super) struct DirectProviderOutcome {
    pub success: bool,
    pub error_class: Option<String>,
    pub http_status: Option<StatusCode>,
    pub duration_ms: u64,
    pub stream_duration_ms: Option<u64>,
    pub tokens_prompt: Option<u64>,
    pub tokens_completion: Option<u64>,
    pub tokens_total: Option<u64>,
}

/// Emits a `DirectProviderRequestCompleted` telemetry event using the global
/// telemetry store. This is called from the background tokio task where the
/// actual HTTP request runs.
fn emit_direct_provider_completed(
    telemetry: &DirectProviderTelemetry,
    outcome: DirectProviderOutcome,
) {
    let is_local = infer_local_only(&telemetry.base_url);
    let endpoint_origin_hash = direct_provider_endpoint_origin_hash(&telemetry.base_url);
    let model_id_hash = direct_provider_telemetry_hash(&telemetry.model);
    let http_status_family = outcome.http_status.map(|s| match s.as_u16() {
        200..=299 => "2xx".to_string(),
        300..=399 => "3xx".to_string(),
        400..=499 => "4xx".to_string(),
        500..=599 => "5xx".to_string(),
        _ => "unknown".to_string(),
    });

    let event = TelemetryEvent::DirectProviderRequestCompleted {
        provider_kind: format!("{:?}", telemetry.provider_kind),
        endpoint_origin_hash,
        model_id_hash,
        success: outcome.success,
        error_class: outcome.error_class,
        duration_ms: outcome.duration_ms,
        stream_duration_ms: outcome.stream_duration_ms,
        http_status_family,
        is_local,
        tokens_prompt: outcome.tokens_prompt,
        tokens_completion: outcome.tokens_completion,
        tokens_total: outcome.tokens_total,
    };

    warpui::telemetry::record_event(
        telemetry.user_id.clone(),
        telemetry.anonymous_id.clone(),
        event.name().into(),
        event.payload(),
        event.contains_ugc(),
        warpui::time::get_current_time(),
    );
}

fn emit_direct_provider_started(telemetry: &DirectProviderTelemetry) {
    let event = TelemetryEvent::DirectProviderRequestStarted {
        provider_kind: format!("{:?}", telemetry.provider_kind),
        endpoint_origin_hash: direct_provider_endpoint_origin_hash(&telemetry.base_url),
        model_id_hash: direct_provider_telemetry_hash(&telemetry.model),
        is_local: infer_local_only(&telemetry.base_url),
    };

    warpui::telemetry::record_event(
        telemetry.user_id.clone(),
        telemetry.anonymous_id.clone(),
        event.name().into(),
        event.payload(),
        event.contains_ugc(),
        warpui::time::get_current_time(),
    );
}

fn direct_provider_endpoint_origin_hash(base_url: &str) -> String {
    let origin = url::Url::parse(base_url)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?;
            let port = url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            Some(format!("{}://{host}{port}", url.scheme()))
        })
        .unwrap_or_else(|| base_url.to_string());
    direct_provider_telemetry_hash(&origin)
}

fn direct_provider_telemetry_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(super) async fn generate_direct_provider_output(
    params: RequestParams,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    let provider = match &params.resolved_provider {
        Ok(provider) => provider.clone(),
        Err(err) => {
            let message = match err {
                ProviderResolutionError::UnknownProvider { provider_id } => {
                    format!("The provider '{provider_id}' used in this conversation has been deleted. Please select a different model to continue.")
                }
                ProviderResolutionError::DisabledProvider { provider_id } => {
                    format!("The provider '{provider_id}' used in this conversation has been disabled. Please select a different model to continue.")
                }
                ProviderResolutionError::UnknownModel {
                    provider_id,
                    model_id,
                } => {
                    format!("The model '{model_id}' from provider '{provider_id}' is no longer available. Please select a different model to continue.")
                }
                ProviderResolutionError::TeamPolicyBlocked {
                    provider_id,
                    reason,
                } => {
                    format!("The provider '{provider_id}' is blocked by team policy: {reason}. Please select a different model to continue.")
                }
                ProviderResolutionError::StaleModel {
                    provider_id,
                    model_id,
                } => {
                    format!("The model '{model_id}' from provider '{provider_id}' needs to be refreshed before it can be used. Please validate the provider or select a different model to continue.")
                }
            };
            return error_stream(message).await;
        }
    };

    let (query, images) = match latest_direct_provider_query(&params.input) {
        Some(result) => result,
        None => {
            return error_stream(
                "Direct providers currently support text user-query turns without attachments only"
                    .to_string(),
            )
            .await;
        }
    };

    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let request_id = uuid::Uuid::new_v4().to_string();
    let task_id = params
        .tasks
        .first()
        .map(|task| task.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let stream = build_stream(
        provider.clone(),
        query,
        images,
        &conversation_id,
        &request_id,
        &task_id,
        &params,
        DirectProviderTelemetry {
            user_id: None,
            anonymous_id: String::new(),
            start_time: Instant::now(),
            provider_kind: provider.kind,
            base_url: provider.base_url,
            model: provider.model,
        },
    )
    .await;

    Ok(Box::pin(stream.take_until(cancellation_rx)))
}

async fn build_stream(
    provider: ResolvedProvider,
    query: String,
    images: Vec<SerializedImage>,
    conversation_id: &str,
    request_id: &str,
    task_id: &str,
    params: &RequestParams,
    telemetry: DirectProviderTelemetry,
) -> BoxStream<'static, Result<api::ResponseEvent, Arc<AIApiError>>> {
    let description = latest_user_query(&params.input).unwrap_or_default();
    let conv_id = conversation_id.to_string();
    let req_id = request_id.to_string();
    let tsk_id = task_id.to_string();
    let tasks_empty = params.tasks.is_empty();

    let auth = match resolve_auth(&provider, &params.legacy_api_keys, |ref_| {
        (params.resolve_secret)(ref_)
    }) {
        Ok(auth) => auth,
        Err(err) => return single_error(Arc::new(AIApiError::Other(anyhow!("{err}")))),
    };

    let cwd = params.session_context.current_working_directory().clone();
    let mut messages = build_conversation_messages(
        &provider,
        cwd.as_deref(),
        &params.conversation_history,
        query,
        images,
    );

    let tools = if provider.capabilities.tools.is_enabled() != Some(false) {
        collect_tools(params)
    } else {
        Vec::new()
    };

    let mcp_executor = params.mcp_tool_executor.clone();
    let kind = provider.kind.clone();
    let base_url = provider.base_url.clone();
    let headers = provider.headers.clone();
    let model = provider.model.clone();
    let defaults = provider.defaults.clone();
    let policy = provider.policy.clone();

    let (tx, rx) = async_channel::unbounded::<Result<api::ResponseEvent, Arc<AIApiError>>>();
    let tx = Arc::new(tokio::sync::Mutex::new(tx));

    tokio::spawn(async move {
        emit_direct_provider_started(&telemetry);

        let mut accumulated_text = String::new();
        let mut sent_init = false;
        let mut sent_actions = false;
        let message_id = uuid::Uuid::new_v4().to_string();
        let mut outcome = DirectProviderOutcome {
            success: false,
            error_class: None,
            http_status: None,
            duration_ms: 0,
            stream_duration_ms: None,
            tokens_prompt: None,
            tokens_completion: None,
            tokens_total: None,
        };

        let mut request =
            match DirectProviderRequest::new_with_messages(&provider, messages.clone(), &tools) {
                Ok(req) => req,
                Err(err) => {
                    let error_str = format!("{err}");
                    let error_class = match &err {
                        DirectProviderError { class, .. } => format!("{class:?}"),
                    };
                    outcome.error_class = Some(error_class);
                    outcome.http_status = err.status;
                    let _ = tx
                        .lock()
                        .await
                        .send(Err(Arc::new(AIApiError::Other(anyhow!("{error_str}")))))
                        .await;
                    emit_direct_provider_completed(&telemetry, outcome);
                    return;
                }
            };

        let max_turns = 10;
        let mut last_http_status: Option<StatusCode> = None;
        for turn in 0..max_turns {
            let client = reqwest::Client::new();
            let mut builder = client.post(&request.url).json(&request.body);
            for (name, value) in &headers {
                builder = builder.header(name, value);
            }
            builder = apply_auth(builder, &kind, auth.clone());

            let turn_start = Instant::now();
            let es = match EventSource::new(builder) {
                Ok(es) => es,
                Err(e) => {
                    outcome.error_class = Some("Unknown".to_string());
                    let _ = tx
                        .lock()
                        .await
                        .send(Err(Arc::new(AIApiError::Other(anyhow!(
                            "Failed to create SSE stream: {e}"
                        )))))
                        .await;
                    emit_direct_provider_completed(&telemetry, outcome);
                    return;
                }
            };

            let result = match stream_to_result(es, kind.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    if let Some(status) = last_http_status {
                        outcome.http_status = Some(status);
                    }
                    outcome.duration_ms = turn_start.elapsed().as_millis() as u64;
                    outcome.stream_duration_ms =
                        Some(telemetry.start_time.elapsed().as_millis() as u64);
                    let _ = tx.lock().await.send(Err(e)).await;
                    emit_direct_provider_completed(&telemetry, outcome);
                    return;
                }
            };

            // Track HTTP status for potential error reporting.
            if let Some(status) = result.http_status {
                last_http_status = Some(status);
                outcome.http_status = Some(status);
            }

            // Emit Init on first turn.
            if !sent_init {
                sent_init = true;
                let _ = tx
                    .lock()
                    .await
                    .send(Ok(api::ResponseEvent {
                        r#type: Some(api::response_event::Type::Init(
                            api::response_event::StreamInit {
                                conversation_id: conv_id.clone(),
                                request_id: req_id.clone(),
                                run_id: conv_id.clone(),
                            },
                        )),
                    }))
                    .await;
            }

            // Emit incremental text events.
            if !result.accumulated_text.is_empty() {
                accumulated_text.push_str(&result.accumulated_text);
                if !sent_actions && tasks_empty {
                    sent_actions = true;
                    let _ = tx
                        .lock()
                        .await
                        .send(Ok(api::ResponseEvent {
                            r#type: Some(api::response_event::Type::ClientActions(
                                api::response_event::ClientActions {
                                    actions: vec![api::ClientAction {
                                        action: Some(api::client_action::Action::CreateTask(
                                            api::client_action::CreateTask {
                                                task: Some(api::Task {
                                                    id: tsk_id.clone(),
                                                    description: description.clone(),
                                                    ..Default::default()
                                                }),
                                            },
                                        )),
                                    }],
                                },
                            )),
                        }))
                        .await;
                }

                let _ = tx
                    .lock()
                    .await
                    .send(Ok(api::ResponseEvent {
                        r#type: Some(api::response_event::Type::ClientActions(
                            api::response_event::ClientActions {
                                actions: vec![api::ClientAction {
                                    action: Some(api::client_action::Action::AddMessagesToTask(
                                        api::client_action::AddMessagesToTask {
                                            task_id: tsk_id.clone(),
                                            messages: vec![api::Message {
                                                id: message_id.clone(),
                                                request_id: req_id.clone(),
                                                message: Some(api::message::Message::AgentOutput(
                                                    api::message::AgentOutput {
                                                        text: accumulated_text.clone(),
                                                    },
                                                )),
                                                ..Default::default()
                                            }],
                                        },
                                    )),
                                }],
                            },
                        )),
                    }))
                    .await;
            }

            // If no tool calls, we're done.
            if result.tool_calls.is_empty() {
                break;
            }

            // Execute tool calls.
            let tool_results =
                match execute_tool_calls(&result.tool_calls, &mcp_executor, &kind).await {
                    Ok(r) => r,
                    Err(e) => {
                        outcome.duration_ms = turn_start.elapsed().as_millis() as u64;
                        outcome.stream_duration_ms =
                            Some(telemetry.start_time.elapsed().as_millis() as u64);
                        let _ = tx.lock().await.send(Err(e)).await;
                        emit_direct_provider_completed(&telemetry, outcome);
                        return;
                    }
                };

            // On last turn, just finish without sending tool results.
            if turn + 1 >= max_turns {
                break;
            }

            // Build follow-up request with tool results.
            let followup = build_followup_request(
                &kind,
                &base_url,
                &headers,
                &model,
                &defaults,
                &policy,
                &tools,
                &mut messages,
                &tool_results,
            );

            match followup {
                Ok(req) => request = req,
                Err(e) => {
                    outcome.duration_ms = turn_start.elapsed().as_millis() as u64;
                    outcome.stream_duration_ms =
                        Some(telemetry.start_time.elapsed().as_millis() as u64);
                    let _ = tx
                        .lock()
                        .await
                        .send(Err(Arc::new(AIApiError::Other(anyhow!("{e}")))))
                        .await;
                    emit_direct_provider_completed(&telemetry, outcome);
                    return;
                }
            }
        }

        // Stream complete.
        outcome.success = true;
        outcome.duration_ms = telemetry.start_time.elapsed().as_millis() as u64;
        outcome.stream_duration_ms = Some(outcome.duration_ms);
        let _ = tx
            .lock()
            .await
            .send(Ok(api::ResponseEvent {
                r#type: Some(api::response_event::Type::Finished(
                    api::response_event::StreamFinished {
                        reason: Some(api::response_event::stream_finished::Reason::Done(
                            api::response_event::stream_finished::Done {},
                        )),
                        ..Default::default()
                    },
                )),
            }))
            .await;
        emit_direct_provider_completed(&telemetry, outcome);
    });

    Box::pin(async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => yield event,
                Err(_) => break,
            }
        }
    })
}

fn single_error(
    err: Arc<AIApiError>,
) -> BoxStream<'static, Result<api::ResponseEvent, Arc<AIApiError>>> {
    Box::pin(futures_util::stream::once(async { Err(err) }))
}

async fn error_stream(message: String) -> Result<ResponseStream, ConvertToAPITypeError> {
    let (tx, rx) = async_channel::unbounded();
    let _ = tx
        .send(Err(Arc::new(AIApiError::Other(anyhow!(message)))))
        .await;
    Ok(Box::pin(rx))
}

/// Result of executing a single tool call.
struct ToolResult {
    call_id: String,
    output: String,
}

/// Execute a batch of tool calls via the MCP executor.
async fn execute_tool_calls(
    tool_calls: &[ToolCall],
    mcp_executor: &Option<Arc<dyn McpToolExecutor>>,
    kind: &ProviderKind,
) -> Result<Vec<ToolResult>, Arc<AIApiError>> {
    let mut results = Vec::new();
    for tc in tool_calls {
        let output = execute_single_tool(tc, mcp_executor, kind).await;
        results.push(ToolResult {
            call_id: tc.id.clone(),
            output,
        });
    }
    Ok(results)
}

/// Execute a single tool call. MCP tools use the executor; built-in Warp tools
/// return a not-available message.
async fn execute_single_tool(
    tool_call: &ToolCall,
    mcp_executor: &Option<Arc<dyn McpToolExecutor>>,
    _kind: &ProviderKind,
) -> String {
    if let Some(ref executor) = mcp_executor {
        if let Value::Object(args) = tool_call.input.clone() {
            match executor.execute(tool_call.name.clone(), args).await {
                Ok(output) => return output,
                Err(e) => return format!("Tool execution error: {e}"),
            }
        }
    }
    format!(
        "This tool is not available in direct-provider mode: {}",
        tool_call.name
    )
}

/// Build a follow-up request with tool results appended to the conversation.
#[allow(clippy::too_many_arguments)]
fn build_followup_request(
    kind: &ProviderKind,
    base_url: &str,
    _headers: &std::collections::BTreeMap<String, String>,
    model: &str,
    defaults: &ProviderDefaults,
    policy: &ai::provider_registry::ProviderPolicy,
    tools: &[ToolDefinition],
    messages: &mut Vec<ProviderMessage>,
    tool_results: &[ToolResult],
) -> Result<DirectProviderRequest, DirectProviderError> {
    let tools_value: Value = if tools.is_empty() {
        Value::Null
    } else {
        Value::Array(tools.iter().map(|t| t.to_openai_tool()).collect::<Vec<_>>())
    };

    let stream = defaults.stream.unwrap_or(true);
    let temperature = defaults.temperature;
    let top_p = defaults.top_p;

    match kind {
        ProviderKind::OpenAI => {
            for tr in tool_results {
                messages.push(ProviderMessage {
                    role: "user".to_string(),
                    content: MessageContent::Text(
                        serde_json::to_string(&json!({
                            "type": "function_call_output",
                            "call_id": tr.call_id,
                            "output": tr.output,
                        }))
                        .unwrap_or_else(|_| tr.output.clone()),
                    ),
                    tool_call_id: None,
                });
            }

            let mut body = json!({
                "model": model,
                "input": messages.iter().map(|m| {
                    json!({
                        "role": m.role,
                        "content": m.content.to_openai_content_value(),
                    })
                }).collect::<Vec<_>>(),
                "stream": stream,
            });
            if let Some(t) = temperature {
                body.as_object_mut()
                    .unwrap()
                    .insert("temperature".to_string(), Value::from(t));
            }
            if let Some(p) = top_p {
                body.as_object_mut()
                    .unwrap()
                    .insert("top_p".to_string(), Value::from(p));
            }
            if !tools_value.is_null() {
                body.as_object_mut()
                    .unwrap()
                    .insert("tools".to_string(), tools_value);
            }

            Ok(DirectProviderRequest {
                kind: kind.clone(),
                url: endpoint_url(base_url, "/responses")?,
                body,
                tools: Vec::new(),
            })
        }
        ProviderKind::OpenAICompatible => {
            for tr in tool_results {
                messages.push(ProviderMessage {
                    role: "tool".to_string(),
                    content: MessageContent::Text(tr.output.clone()),
                    tool_call_id: Some(tr.call_id.clone()),
                });
            }

            let mut body = json!({
                "model": model,
                "messages": messages.iter().map(|m| m.to_openai_chat_message()).collect::<Vec<_>>(),
                "stream": stream,
            });
            if let Some(t) = temperature {
                body.as_object_mut()
                    .unwrap()
                    .insert("temperature".to_string(), Value::from(t));
            }
            if let Some(p) = top_p {
                body.as_object_mut()
                    .unwrap()
                    .insert("top_p".to_string(), Value::from(p));
            }
            if !tools_value.is_null() {
                body.as_object_mut()
                    .unwrap()
                    .insert("tools".to_string(), tools_value);
            }

            Ok(DirectProviderRequest {
                kind: kind.clone(),
                url: endpoint_url(base_url, "/chat/completions")?,
                body,
                tools: Vec::new(),
            })
        }
        ProviderKind::Anthropic | ProviderKind::AnthropicCompatible => {
            let (system_msg, rest): (Vec<_>, Vec<_>) =
                messages.iter().partition(|m| m.role == "system");
            let system_content = system_msg.first().and_then(|m| m.content.as_str());

            let anthropic_tools: Value = if tools.is_empty() {
                Value::Null
            } else {
                Value::Array(
                    tools
                        .iter()
                        .map(|t| t.to_anthropic_tool())
                        .collect::<Vec<_>>(),
                )
            };

            let mut anthropic_messages: Vec<Value> =
                rest.iter().map(|m| m.to_anthropic_message()).collect();

            for tr in tool_results {
                anthropic_messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tr.call_id,
                        "content": tr.output,
                    }]
                }));
            }

            let max_tokens = defaults
                .max_output_tokens
                .unwrap_or_else(|| default_anthropic_max_tokens(policy));

            let mut body = json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": anthropic_messages,
                "stream": stream,
            });
            if let Some(t) = temperature {
                body.as_object_mut()
                    .unwrap()
                    .insert("temperature".to_string(), Value::from(t));
            }
            if let Some(p) = top_p {
                body.as_object_mut()
                    .unwrap()
                    .insert("top_p".to_string(), Value::from(p));
            }
            if let Some(content) = system_content {
                body.as_object_mut()
                    .unwrap()
                    .insert("system".to_string(), Value::String(content.to_string()));
            }
            if !anthropic_tools.is_null() {
                body.as_object_mut()
                    .unwrap()
                    .insert("tools".to_string(), anthropic_tools);
            }

            Ok(DirectProviderRequest {
                kind: kind.clone(),
                url: endpoint_url(base_url, "/messages")?,
                body,
                tools: Vec::new(),
            })
        }
        ProviderKind::WarpHosted => Err(DirectProviderError::new(
            DirectProviderErrorClass::UnsupportedCapability,
            "warp-hosted providers do not use direct HTTP transport",
        )),
    }
}

/// Collect tool definitions from MCP context for direct provider requests.
fn collect_tools(params: &RequestParams) -> Vec<ToolDefinition> {
    let mut tools = Vec::new();
    if let Some(mcp_context) = &params.mcp_context {
        // For grouped MCP servers (the modern path)
        for server in &mcp_context.servers {
            for tool in &server.tools {
                tools.push(ToolDefinition {
                    name: tool.name.clone().into_owned(),
                    description: tool
                        .description
                        .as_ref()
                        .map(|d| d.clone().into_owned())
                        .unwrap_or_default(),
                    input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
                });
            }
        }
        // For flat MCP tool lists (legacy path)
        #[allow(deprecated)]
        for tool in &mcp_context.tools {
            if !tools.iter().any(|t| t.name == tool.name) {
                tools.push(ToolDefinition {
                    name: tool.name.clone().into_owned(),
                    description: tool
                        .description
                        .as_ref()
                        .map(|d| d.clone().into_owned())
                        .unwrap_or_default(),
                    input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
                });
            }
        }
    }
    tools
}

fn latest_user_query(inputs: &[AIAgentInput]) -> Option<String> {
    inputs.iter().rev().find_map(AIAgentInput::user_query)
}

fn latest_direct_provider_query(inputs: &[AIAgentInput]) -> Option<(String, Vec<SerializedImage>)> {
    inputs.iter().rev().find_map(|input| match input {
        AIAgentInput::UserQuery {
            query,
            referenced_attachments,
            user_query_mode,
            context,
            ..
        } if referenced_attachments.is_empty()
            && matches!(user_query_mode, crate::ai::agent::UserQueryMode::Normal) =>
        {
            let images = extract_images_from_context(context);
            Some((query.clone(), images))
        }
        AIAgentInput::AutoCodeDiffQuery { query, context, .. } => {
            let images = extract_images_from_context(context);
            Some((query.clone(), images))
        }
        AIAgentInput::CodeReview {
            review_comments, ..
        } => {
            let query = review_comments
                .comments
                .iter()
                .map(|c| c.content.clone())
                .collect::<Vec<_>>()
                .join("\n\n");
            if query.is_empty() {
                None
            } else {
                Some((query, vec![]))
            }
        }
        AIAgentInput::InvokeSkill {
            skill,
            user_query,
            context,
            ..
        } => {
            let query = match user_query {
                Some(uq) if !uq.query.is_empty() => {
                    format!("{} {}", skill.name, uq.query)
                }
                _ => skill.name.clone(),
            };
            let images = extract_images_from_context(context);
            Some((query, images))
        }
        AIAgentInput::StartFromAmbientRunPrompt {
            ambient_run_id,
            context,
            runtime_skill,
            ..
        } => {
            let query = match runtime_skill {
                Some(skill) => {
                    format!("(ambient run: {ambient_run_id}) {}", skill.name)
                }
                None => format!("(ambient run: {ambient_run_id})"),
            };
            let images = extract_images_from_context(context);
            Some((query, images))
        }
        AIAgentInput::UserQuery { .. }
        | AIAgentInput::ResumeConversation { .. }
        | AIAgentInput::InitProjectRules { .. }
        | AIAgentInput::CreateEnvironment { .. }
        | AIAgentInput::TriggerPassiveSuggestion { .. }
        | AIAgentInput::CreateNewProject { .. }
        | AIAgentInput::CloneRepository { .. }
        | AIAgentInput::FetchReviewComments { .. }
        | AIAgentInput::SummarizeConversation { .. }
        | AIAgentInput::ActionResult { .. }
        | AIAgentInput::MessagesReceivedFromAgents { .. }
        | AIAgentInput::EventsFromAgents { .. }
        | AIAgentInput::PassiveSuggestionResult { .. } => None,
    })
}

fn extract_images_from_context(
    context: &[crate::ai::agent::AIAgentContext],
) -> Vec<SerializedImage> {
    context
        .iter()
        .filter_map(|ctx| match ctx {
            crate::ai::agent::AIAgentContext::Image(image_ctx) => Some(SerializedImage {
                data_base64: image_ctx.data.clone(),
                mime_type: image_ctx.mime_type.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn stream_to_result(
    mut es: EventSource,
    kind: ProviderKind,
) -> impl futures_util::Future<Output = Result<StreamingResult, Arc<AIApiError>>> {
    let mut accumulated_text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut current_tool_id: Option<String> = None;
    let mut current_tool_name: Option<String> = None;
    let mut current_tool_input = String::new();
    let http_status: Option<StatusCode> = None;

    async move {
        loop {
            match es.next().await {
                Some(Ok(Event::Open)) => continue,
                Some(Ok(Event::Message(message))) => {
                    if message.data.trim() == "[DONE]" {
                        break;
                    }

                    let parsed = match parse_sse_chunk(&kind, &message.event, &message.data) {
                        Ok(chunk) => chunk,
                        Err(err) => {
                            return Err(Arc::new(AIApiError::Other(anyhow!("{err}"))));
                        }
                    };

                    match parsed {
                        SseChunk::TextDelta(text) => {
                            accumulated_text.push_str(&text);
                        }
                        SseChunk::ToolCallStart { id, name } => {
                            if !current_tool_input.is_empty() {
                                flush_current_tool_call(
                                    &mut tool_calls,
                                    &mut current_tool_id,
                                    &mut current_tool_name,
                                    &mut current_tool_input,
                                );
                            }
                            current_tool_id = Some(id.clone());
                            current_tool_name = Some(name.clone());
                            current_tool_input.clear();
                        }
                        SseChunk::ToolCallDelta { partial_json, .. } => {
                            current_tool_input.push_str(&partial_json);
                        }
                        SseChunk::ToolCallStop { id } => {
                            let call_id = if id.is_empty() {
                                current_tool_id.take().unwrap_or_default()
                            } else {
                                current_tool_id.take().unwrap_or(id)
                            };
                            let name = current_tool_name.take().unwrap_or_default();
                            let input = serde_json::from_str(&current_tool_input)
                                .unwrap_or(Value::String(current_tool_input.clone()));
                            tool_calls.push(ToolCall {
                                id: call_id,
                                name,
                                input,
                            });
                            current_tool_input.clear();
                        }
                        SseChunk::Done => {
                            break;
                        }
                        SseChunk::Skip => {
                            continue;
                        }
                    }
                }
                Some(Err(err)) => {
                    if matches!(err, reqwest_eventsource::Error::StreamEnded) {
                        break;
                    }
                    let dp_error = match &err {
                        reqwest_eventsource::Error::Transport(reqwest_err) => {
                            DirectProviderError::from_transport(reqwest_err)
                        }
                        _ => DirectProviderError::new(
                            DirectProviderErrorClass::Unknown,
                            err.to_string(),
                        ),
                    };
                    return Err(Arc::new(AIApiError::Other(anyhow!("{dp_error}"))));
                }
                None => break,
            }
        }

        if !current_tool_input.is_empty() {
            flush_current_tool_call(
                &mut tool_calls,
                &mut current_tool_id,
                &mut current_tool_name,
                &mut current_tool_input,
            );
        }

        Ok(StreamingResult {
            accumulated_text,
            tool_calls,
            http_status,
        })
    }
}

fn flush_current_tool_call(
    tool_calls: &mut Vec<ToolCall>,
    current_tool_id: &mut Option<String>,
    current_tool_name: &mut Option<String>,
    current_tool_input: &mut String,
) {
    let input = serde_json::from_str(current_tool_input)
        .unwrap_or(Value::String(current_tool_input.clone()));
    tool_calls.push(ToolCall {
        id: current_tool_id.take().unwrap_or_default(),
        name: current_tool_name.take().unwrap_or_default(),
        input,
    });
    current_tool_input.clear();
}

enum SseChunk {
    TextDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { _id: String, partial_json: String },
    ToolCallStop { id: String },
    Done,
    /// Empty content chunk — skip and continue (common in OpenAI-compatible streams).
    Skip,
}

fn parse_sse_chunk(
    kind: &ProviderKind,
    event: &str,
    data: &str,
) -> Result<SseChunk, DirectProviderError> {
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(SseChunk::Done);
    }

    let value: Value = serde_json::from_str(data).map_err(|_| {
        DirectProviderError::new(
            DirectProviderErrorClass::InvalidResponseShape,
            "provider returned invalid SSE JSON",
        )
    })?;

    match kind {
        ProviderKind::OpenAI => parse_openai_responses_chunk(&value),
        ProviderKind::OpenAICompatible => parse_openai_chat_chunk(&value),
        ProviderKind::Anthropic | ProviderKind::AnthropicCompatible => {
            parse_anthropic_chunk(event, &value)
        }
        ProviderKind::WarpHosted => Err(DirectProviderError::new(
            DirectProviderErrorClass::UnsupportedCapability,
            "warp-hosted providers do not use direct HTTP transport",
        )),
    }
}

fn parse_openai_responses_chunk(value: &Value) -> Result<SseChunk, DirectProviderError> {
    // OpenAI Responses API streaming uses events like:
    // "response.output_text.delta" with delta: { text: "..." }
    // "response.output_item.added" with item: { type: "function_call", id: "...", name: "..." }
    // "response.function_call_arguments.delta" with delta: { text: "..." }
    // "response.output_item.done" for tool call completion
    // "response.completed" for the final event
    let r#type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match r#type {
        "response.output_text.delta" => {
            let text = value
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                Ok(SseChunk::Done)
            } else {
                Ok(SseChunk::TextDelta(text))
            }
        }
        "response.output_item.added" => {
            // Check if this is a function call
            let item = value.get("item").and_then(|i| i.as_object());
            if let Some(item) = item {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    return Ok(SseChunk::ToolCallStart { id, name });
                }
            }
            Ok(SseChunk::Done)
        }
        "response.function_call_arguments.delta" => {
            let partial = value
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if partial.is_empty() {
                Ok(SseChunk::Done)
            } else {
                Ok(SseChunk::ToolCallDelta {
                    _id: String::new(),
                    partial_json: partial,
                })
            }
        }
        "response.output_item.done" => {
            let item = value.get("item").and_then(|i| i.as_object());
            if let Some(item) = item {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    return Ok(SseChunk::ToolCallStop { id });
                }
            }
            Ok(SseChunk::Done)
        }
        "response.completed" => Ok(SseChunk::Done),
        // Reasoning items: "response.reasoning_text.delta" — treat as text for now
        "response.reasoning_text.delta" => {
            let text = value
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                Ok(SseChunk::Done)
            } else {
                Ok(SseChunk::TextDelta(text))
            }
        }
        _ => Ok(SseChunk::Done),
    }
}

fn parse_openai_chat_chunk(value: &Value) -> Result<SseChunk, DirectProviderError> {
    // OpenAI Chat Completions streaming:
    // {"choices":[{"delta":{"content":"..."},"index":0,"finish_reason":null}]}
    // Tool calls: {"choices":[{"delta":{"tool_calls":[{"id":"...","type":"function","function":{"name":"...","arguments":"..."}}]},"finish_reason":"tool_calls"}]}
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DirectProviderError::new(
                DirectProviderErrorClass::InvalidResponseShape,
                "chat completion chunk missing choices",
            )
        })?;

    let first = choices.first().ok_or_else(|| {
        DirectProviderError::new(
            DirectProviderErrorClass::InvalidResponseShape,
            "chat completion chunk has empty choices",
        )
    })?;

    // Check for tool_calls in delta
    if let Some(tool_calls) = first
        .get("delta")
        .and_then(|d| d.get("tool_calls"))
        .and_then(Value::as_array)
    {
        if let Some(tc) = tool_calls.first() {
            let id = tc
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(name) = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
            {
                return Ok(SseChunk::ToolCallStart {
                    id,
                    name: name.to_string(),
                });
            }
            if let Some(partial) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
            {
                if !partial.is_empty() {
                    return Ok(SseChunk::ToolCallDelta {
                        _id: id,
                        partial_json: partial.to_string(),
                    });
                }
            }
        }
    }

    // Check for finish_reason — signals end of stream
    if let Some(reason) = first.get("finish_reason") {
        if !reason.is_null() {
            return Ok(SseChunk::Done);
        }
    }

    let content = first
        .get("delta")
        .and_then(|d| d.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");

    if content.is_empty() {
        return Ok(SseChunk::Skip);
    }

    Ok(SseChunk::TextDelta(content.to_string()))
}

fn parse_anthropic_chunk(event_type: &str, value: &Value) -> Result<SseChunk, DirectProviderError> {
    // Anthropic SSE events have named event types:
    // message_start, content_block_start, content_block_delta, content_block_stop, message_delta, message_stop
    // Tool calls: content_block_start with type="tool_use", content_block_delta with type="input_json_delta"
    let event = event_type;
    match event {
        "content_block_start" => {
            // Check if this is a tool_use block
            if let Some(content_type) = value
                .get("content_block")
                .and_then(|cb| cb.get("type"))
                .and_then(Value::as_str)
            {
                if content_type == "tool_use" {
                    let id = value
                        .get("content_block")
                        .and_then(|cb| cb.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = value
                        .get("content_block")
                        .and_then(|cb| cb.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    return Ok(SseChunk::ToolCallStart { id, name });
                }
            }
            Ok(SseChunk::Done)
        }
        "content_block_delta" => {
            let delta_type = value
                .get("delta")
                .and_then(|d| d.get("type"))
                .and_then(Value::as_str);
            match delta_type {
                Some("text_delta") => {
                    let text = value
                        .get("delta")
                        .and_then(|d| d.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if text.is_empty() {
                        Ok(SseChunk::Done)
                    } else {
                        Ok(SseChunk::TextDelta(text.to_string()))
                    }
                }
                Some("input_json_delta") => {
                    let partial = value
                        .get("delta")
                        .and_then(|d| d.get("partial_json"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if partial.is_empty() {
                        Ok(SseChunk::Done)
                    } else {
                        Ok(SseChunk::ToolCallDelta {
                            _id: String::new(),
                            partial_json: partial.to_string(),
                        })
                    }
                }
                _ => Ok(SseChunk::Done),
            }
        }
        "content_block_stop" => {
            // Tool call ended — we'd need the index to match start/stop, but for now
            // emit a generic stop. The streaming_events handler tracks the current tool id.
            Ok(SseChunk::ToolCallStop { id: String::new() })
        }
        "message_stop" | "message_delta" => Ok(SseChunk::Done),
        _ => Ok(SseChunk::Done),
    }
}

/// A provider-agnostic tool definition for direct provider requests.
#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolDefinition {
    /// Convert to OpenAI Responses / Chat Completions tool format.
    fn to_openai_tool(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    /// Convert to Anthropic Messages tool format.
    fn to_anthropic_tool(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        })
    }
}

/// A tool call returned from a provider response.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Result of a single streaming turn: accumulated text plus any tool calls.
struct StreamingResult {
    #[allow(dead_code)]
    accumulated_text: String,
    tool_calls: Vec<ToolCall>,
    http_status: Option<StatusCode>,
}

#[allow(dead_code)]
struct DirectProviderRequest {
    kind: ProviderKind,
    url: String,
    body: Value,
    tools: Vec<ToolCall>,
}

/// A single message in a provider-specific format.
#[derive(Clone, Debug)]
struct ProviderMessage {
    role: String,
    content: MessageContent,
    tool_call_id: Option<String>,
}

/// Multi-modal message content: plain text or an array of text/image parts.
#[derive(Clone, Debug, PartialEq)]
enum MessageContent {
    /// Simple string content (system messages, assistant messages).
    Text(String),
    /// Multi-part content (user messages with images).
    Multi(Vec<ContentPart>),
}

impl Serialize for MessageContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Multi(parts) => parts.serialize(serializer),
        }
    }
}

impl MessageContent {
    fn as_str(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Multi(_) => None,
        }
    }

    /// Convert to OpenAI Responses / Chat Completions content format.
    fn to_openai_content_value(&self) -> Value {
        match self {
            MessageContent::Text(s) => Value::String(s.clone()),
            MessageContent::Multi(parts) => {
                Value::Array(parts.iter().map(|p| p.to_openai_part()).collect())
            }
        }
    }

    /// Convert to Anthropic Messages content format.
    fn to_anthropic_content_value(&self) -> Value {
        match self {
            MessageContent::Text(s) => Value::String(s.clone()),
            MessageContent::Multi(parts) => {
                Value::Array(parts.iter().map(|p| p.to_anthropic_part()).collect())
            }
        }
    }
}

impl PartialEq<&str> for MessageContent {
    fn eq(&self, other: &&str) -> bool {
        self.as_str().is_some_and(|s| s == *other)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
enum ContentPart {
    Text(String),
    Image {
        data_base64: String,
        mime_type: String,
    },
}

impl ContentPart {
    fn to_openai_part(&self) -> Value {
        match self {
            ContentPart::Text(text) => json!({ "type": "text", "text": text }),
            ContentPart::Image {
                data_base64,
                mime_type,
            } => {
                json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{mime_type};base64,{data_base64}")
                    }
                })
            }
        }
    }

    fn to_anthropic_part(&self) -> Value {
        match self {
            ContentPart::Text(text) => json!({ "type": "text", "text": text }),
            ContentPart::Image {
                data_base64,
                mime_type,
            } => {
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type,
                        "data": data_base64
                    }
                })
            }
        }
    }
}

impl ProviderMessage {
    fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
        }
    }

    fn user_with_images(text: String, images: Vec<SerializedImage>) -> Self {
        if images.is_empty() {
            return Self::user(text);
        }
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(ContentPart::Text(text));
        }
        for img in images {
            parts.push(ContentPart::Image {
                data_base64: img.data_base64,
                mime_type: img.mime_type,
            });
        }
        Self {
            role: "user".to_string(),
            content: MessageContent::Multi(parts),
            tool_call_id: None,
        }
    }

    fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
        }
    }

    fn to_openai_chat_message(&self) -> Value {
        let mut msg = json!({
            "role": self.role,
            "content": self.content.to_openai_content_value(),
        });
        if let Some(ref tool_call_id) = self.tool_call_id {
            msg.as_object_mut().unwrap().insert(
                "tool_call_id".to_string(),
                Value::String(tool_call_id.clone()),
            );
        }
        msg
    }

    fn to_anthropic_message(&self) -> Value {
        json!({
            "role": self.role,
            "content": self.content.to_anthropic_content_value(),
        })
    }
}

/// Builds a list of messages from conversation history plus the current user query.
fn build_conversation_messages(
    _provider: &ResolvedProvider,
    cwd: Option<&str>,
    history: &[AgentExchangeSnapshot],
    current_query: String,
    current_images: Vec<SerializedImage>,
) -> Vec<ProviderMessage> {
    let mut messages = Vec::new();

    // Build system message from context (OS, working directory, time)
    if let Some(system_content) = build_system_message(cwd) {
        messages.push(ProviderMessage {
            role: "system".to_string(),
            content: MessageContent::Text(system_content),
            tool_call_id: None,
        });
    }

    // Add previous exchanges as alternating user/assistant messages
    for snapshot in history {
        messages.push(ProviderMessage::user_with_images(
            snapshot.user_query.clone(),
            snapshot.user_images.clone(),
        ));
        messages.push(ProviderMessage::assistant(&snapshot.assistant_text));
    }

    // Add the current user query with images
    messages.push(ProviderMessage::user_with_images(
        current_query,
        current_images,
    ));

    messages
}

/// Builds a system message with available context.
fn build_system_message(cwd: Option<&str>) -> Option<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let time_str = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");

    let mut parts = Vec::new();
    parts.push(
        "You are an AI coding assistant running inside Warp, the AI terminal and code editor."
            .to_string(),
    );
    parts.push(format!("Operating system: {os} ({arch})"));
    parts.push(format!("Current time: {time_str}"));

    if let Some(cwd) = cwd {
        parts.push(format!("Working directory: {cwd}"));
    }

    parts.push(
        "You help the user with coding tasks: writing, editing, debugging, and explaining code. \
         When writing commands or file paths, use the correct syntax for the user's operating system."
            .to_string(),
    );

    Some(parts.join("\n"))
}

impl DirectProviderRequest {
    #[allow(dead_code)]
    fn new(provider: &ResolvedProvider, query: String) -> Result<Self, DirectProviderError> {
        let messages = build_conversation_messages(provider, None, &[], query, vec![]);
        Self::new_with_messages(provider, messages, &[])
    }

    fn new_with_messages(
        provider: &ResolvedProvider,
        messages: Vec<ProviderMessage>,
        tools: &[ToolDefinition],
    ) -> Result<Self, DirectProviderError> {
        let tools_value: Value = if tools.is_empty() {
            Value::Null
        } else {
            Value::Array(tools.iter().map(|t| t.to_openai_tool()).collect::<Vec<_>>())
        };

        let stream = provider.defaults.stream.unwrap_or(true);
        let temperature = provider.defaults.temperature;
        let top_p = provider.defaults.top_p;

        let (path, body) = match provider.kind {
            ProviderKind::OpenAI => {
                let mut body = json!({
                    "model": provider.model,
                    "input": messages.into_iter().map(|m| {
                        json!({
                            "role": m.role,
                            "content": m.content.to_openai_content_value(),
                        })
                    }).collect::<Vec<_>>(),
                    "stream": stream,
                });
                if let Some(t) = temperature {
                    body.as_object_mut()
                        .unwrap()
                        .insert("temperature".to_string(), Value::from(t));
                }
                if let Some(p) = top_p {
                    body.as_object_mut()
                        .unwrap()
                        .insert("top_p".to_string(), Value::from(p));
                }
                if !tools_value.is_null() {
                    body.as_object_mut()
                        .unwrap()
                        .insert("tools".to_string(), tools_value);
                }
                ("/responses", body)
            }
            ProviderKind::OpenAICompatible => {
                let mut body = json!({
                    "model": provider.model,
                    "messages": messages.into_iter().map(|m| {
                        json!({
                            "role": m.role,
                            "content": m.content.to_openai_content_value(),
                        })
                    }).collect::<Vec<_>>(),
                    "stream": stream,
                });
                if let Some(t) = temperature {
                    body.as_object_mut()
                        .unwrap()
                        .insert("temperature".to_string(), Value::from(t));
                }
                if let Some(p) = top_p {
                    body.as_object_mut()
                        .unwrap()
                        .insert("top_p".to_string(), Value::from(p));
                }
                if !tools_value.is_null() {
                    body.as_object_mut()
                        .unwrap()
                        .insert("tools".to_string(), tools_value);
                }
                ("/chat/completions", body)
            }
            ProviderKind::Anthropic | ProviderKind::AnthropicCompatible => {
                let (system_msg, rest): (Vec<_>, Vec<_>) =
                    messages.into_iter().partition(|m| m.role == "system");
                let system_content = system_msg.first().and_then(|m| m.content.as_str());

                let anthropic_tools: Value = if tools.is_empty() {
                    Value::Null
                } else {
                    Value::Array(
                        tools
                            .iter()
                            .map(|t| t.to_anthropic_tool())
                            .collect::<Vec<_>>(),
                    )
                };

                let max_tokens = provider
                    .defaults
                    .max_output_tokens
                    .unwrap_or_else(|| default_anthropic_max_tokens(&provider.policy));

                let mut body = json!({
                    "model": provider.model,
                    "max_tokens": max_tokens,
                    "messages": rest.into_iter().map(|m| {
                        json!({
                            "role": m.role,
                            "content": m.content.to_anthropic_content_value(),
                        })
                    }).collect::<Vec<_>>(),
                    "stream": stream,
                });
                if let Some(t) = temperature {
                    body.as_object_mut()
                        .unwrap()
                        .insert("temperature".to_string(), Value::from(t));
                }
                if let Some(p) = top_p {
                    body.as_object_mut()
                        .unwrap()
                        .insert("top_p".to_string(), Value::from(p));
                }
                if let Some(content) = system_content {
                    body.as_object_mut()
                        .unwrap()
                        .insert("system".to_string(), Value::String(content.to_string()));
                }
                if !anthropic_tools.is_null() {
                    body.as_object_mut()
                        .unwrap()
                        .insert("tools".to_string(), anthropic_tools);
                }
                ("/messages", body)
            }
            ProviderKind::WarpHosted => {
                return Err(DirectProviderError::new(
                    DirectProviderErrorClass::UnsupportedCapability,
                    "warp-hosted providers do not use direct HTTP transport",
                ));
            }
        };

        Ok(Self {
            kind: provider.kind.clone(),
            url: endpoint_url(&provider.base_url, path)?,
            body,
            tools: Vec::new(),
        })
    }

    #[allow(dead_code)]
    fn extract_output_text(&self, body: &str) -> Result<String, DirectProviderError> {
        let value: Value = serde_json::from_str(body).map_err(|_| {
            DirectProviderError::new(
                DirectProviderErrorClass::InvalidResponseShape,
                "provider returned invalid JSON",
            )
        })?;

        let output = match self.kind {
            ProviderKind::OpenAI => openai_response_output_text(&value),
            ProviderKind::OpenAICompatible => openai_chat_output_text(&value),
            ProviderKind::Anthropic | ProviderKind::AnthropicCompatible => {
                anthropic_output_text(&value)
            }
            ProviderKind::WarpHosted => None,
        };

        output.filter(|text| !text.is_empty()).ok_or_else(|| {
            DirectProviderError::new(
                DirectProviderErrorClass::InvalidResponseShape,
                "provider response did not contain output text",
            )
        })
    }
}

fn endpoint_url(base_url: &str, path: &str) -> Result<String, DirectProviderError> {
    if base_url.trim().is_empty() {
        return Err(DirectProviderError::new(
            DirectProviderErrorClass::EndpointUnreachable,
            "provider base URL is empty",
        ));
    }
    Ok(format!("{}{}", base_url.trim_end_matches('/'), path))
}

#[derive(Clone)]
enum AuthMaterial {
    None,
    ApiKey(String),
    BearerToken(String),
}

fn resolve_auth(
    provider: &ResolvedProvider,
    legacy_api_keys: &ApiKeys,
    resolve_secret: impl Fn(&str) -> Option<String>,
) -> Result<AuthMaterial, DirectProviderError> {
    match &provider.auth {
        ResolvedAuth::None => Ok(AuthMaterial::None),
        ResolvedAuth::EnvApiKey { env_var } => {
            let env_var = env_var.as_deref().ok_or_else(|| {
                DirectProviderError::new(
                    DirectProviderErrorClass::MissingSecret,
                    "provider is missing an API key environment variable",
                )
            })?;
            std::env::var(env_var)
                .map(AuthMaterial::ApiKey)
                .map_err(|_| {
                    DirectProviderError::new(
                        DirectProviderErrorClass::MissingSecret,
                        format!("environment variable {env_var} is not set"),
                    )
                })
        }
        ResolvedAuth::KeychainApiKey { secret_ref } => {
            // Try legacy refs first (backward compat), then resolve via secure storage
            let key = match (provider.kind.clone(), secret_ref.as_deref()) {
                (ProviderKind::OpenAI, Some("legacy://AiApiKeys/openai")) => {
                    legacy_api_keys.openai.clone()
                }
                (ProviderKind::Anthropic, Some("legacy://AiApiKeys/anthropic")) => {
                    legacy_api_keys.anthropic.clone()
                }
                _ => None,
            };
            if let Some(k) = key {
                return Ok(AuthMaterial::ApiKey(k));
            }
            // Resolve through secure storage / env
            if let Some(ref_) = secret_ref.as_deref() {
                if let Some(value) = resolve_secret(ref_) {
                    return Ok(AuthMaterial::ApiKey(value));
                }
            }
            Err(DirectProviderError::new(
                DirectProviderErrorClass::MissingSecret,
                "provider API key secret is unavailable",
            ))
        }
        ResolvedAuth::KeychainBearerToken { secret_ref } => {
            let value = resolve_secret(secret_ref).ok_or_else(|| {
                DirectProviderError::new(
                    DirectProviderErrorClass::MissingSecret,
                    "bearer token is not available in secure storage",
                )
            })?;
            Ok(AuthMaterial::BearerToken(value))
        }
    }
}

fn apply_auth(
    builder: reqwest::RequestBuilder,
    provider_kind: &ProviderKind,
    auth: AuthMaterial,
) -> reqwest::RequestBuilder {
    match (provider_kind, auth) {
        (_, AuthMaterial::None) => builder,
        (
            ProviderKind::Anthropic | ProviderKind::AnthropicCompatible,
            AuthMaterial::ApiKey(key),
        ) => builder
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        (_, AuthMaterial::ApiKey(key)) => builder.bearer_auth(key),
        (_, AuthMaterial::BearerToken(token)) => builder.bearer_auth(token),
    }
}

fn openai_response_output_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }

    let mut text = String::new();
    for item in value.get("output")?.as_array()? {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    text.push_str(part_text);
                }
            }
        }
    }
    Some(text)
}

fn openai_chat_output_text(value: &Value) -> Option<String> {
    value
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
        .map(ToString::to_string)
}

fn anthropic_output_text(value: &Value) -> Option<String> {
    let mut text = String::new();
    for item in value.get("content")?.as_array()? {
        if item.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(part_text) = item.get("text").and_then(Value::as_str) {
                text.push_str(part_text);
            }
        }
    }
    Some(text)
}

#[allow(dead_code)]
fn direct_response_events(params: &RequestParams, output: String) -> Vec<api::ResponseEvent> {
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let request_id = uuid::Uuid::new_v4().to_string();
    let task_id = params
        .tasks
        .first()
        .map(|task| task.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let message_id = uuid::Uuid::new_v4().to_string();

    let mut actions = Vec::new();
    if params.tasks.is_empty() {
        actions.push(api::ClientAction {
            action: Some(api::client_action::Action::CreateTask(
                api::client_action::CreateTask {
                    task: Some(api::Task {
                        id: task_id.clone(),
                        description: latest_user_query(&params.input).unwrap_or_default(),
                        ..Default::default()
                    }),
                },
            )),
        });
    }

    actions.push(api::ClientAction {
        action: Some(api::client_action::Action::AddMessagesToTask(
            api::client_action::AddMessagesToTask {
                task_id,
                messages: vec![api::Message {
                    id: message_id,
                    request_id: request_id.clone(),
                    message: Some(api::message::Message::AgentOutput(
                        api::message::AgentOutput { text: output },
                    )),
                    ..Default::default()
                }],
            },
        )),
    });

    vec![
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::Init(
                api::response_event::StreamInit {
                    conversation_id: conversation_id.clone(),
                    request_id,
                    run_id: conversation_id,
                },
            )),
        },
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::ClientActions(
                api::response_event::ClientActions { actions },
            )),
        },
        api::ResponseEvent {
            r#type: Some(api::response_event::Type::Finished(
                api::response_event::StreamFinished {
                    reason: Some(api::response_event::stream_finished::Reason::Done(
                        api::response_event::stream_finished::Done {},
                    )),
                    ..Default::default()
                },
            )),
        },
    ]
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectProviderErrorClass {
    AuthFailure,
    EndpointUnreachable,
    Timeout,
    InvalidResponseShape,
    MissingModel,
    UnsupportedCapability,
    RateLimit,
    ProviderStatus,
    MissingSecret,
    Unknown,
}

#[derive(Debug)]
pub struct DirectProviderError {
    class: DirectProviderErrorClass,
    message: String,
    status: Option<StatusCode>,
}

impl DirectProviderError {
    fn new(class: DirectProviderErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            status: None,
        }
    }

    #[allow(dead_code)]
    fn from_status(status: StatusCode, body: String) -> Self {
        let class = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                DirectProviderErrorClass::AuthFailure
            }
            StatusCode::NOT_FOUND => DirectProviderErrorClass::MissingModel,
            StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
                DirectProviderErrorClass::Timeout
            }
            StatusCode::TOO_MANY_REQUESTS => DirectProviderErrorClass::RateLimit,
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
                DirectProviderErrorClass::UnsupportedCapability
            }
            _ if status.is_server_error() => DirectProviderErrorClass::ProviderStatus,
            _ => DirectProviderErrorClass::ProviderStatus,
        };
        Self {
            class,
            message: if body.trim().is_empty() {
                format!("provider returned HTTP {status}")
            } else {
                format!("provider returned HTTP {status}: {body}")
            },
            status: Some(status),
        }
    }

    fn from_transport(error: &reqwest::Error) -> Self {
        let class = if error.is_timeout() {
            DirectProviderErrorClass::Timeout
        } else if error.is_connect() {
            DirectProviderErrorClass::EndpointUnreachable
        } else if error.is_decode() {
            DirectProviderErrorClass::InvalidResponseShape
        } else {
            DirectProviderErrorClass::Unknown
        };
        Self {
            class,
            message: error.to_string(),
            status: error.status(),
        }
    }

    #[cfg(test)]
    fn class(&self) -> DirectProviderErrorClass {
        self.class.clone()
    }
}

impl std::fmt::Display for DirectProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(
                f,
                "Direct provider error ({:?}, HTTP {status}): {}",
                self.class, self.message
            ),
            None => write!(
                f,
                "Direct provider error ({:?}): {}",
                self.class, self.message
            ),
        }
    }
}

impl std::error::Error for DirectProviderError {}

fn default_anthropic_max_tokens(policy: &ai::provider_registry::ProviderPolicy) -> u32 {
    if policy.allow_manual_model_ids {
        4096
    } else {
        1024
    }
}

#[cfg(test)]
#[path = "direct_provider_tests.rs"]
mod tests;
