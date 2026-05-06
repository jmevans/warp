use std::collections::BTreeMap;

use ai::provider_registry::{
    ProviderCapabilities, ProviderDefaults, ProviderKind, ProviderPolicy, ResolvedAuth,
    ResolvedProvider,
};
use reqwest::StatusCode;

use super::*;

fn resolved_provider(kind: ProviderKind, base_url: &str) -> ResolvedProvider {
    ResolvedProvider {
        profile_id: "test-provider".to_string(),
        kind: kind.clone(),
        model: "test-model".to_string(),
        base_url: base_url.to_string(),
        auth: ResolvedAuth::None,
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        policy: ProviderPolicy::for_kind(&kind),
        defaults: ProviderDefaults::default(),
    }
}

#[test]
fn openai_compatible_request_uses_chat_completions_payload() {
    let provider = resolved_provider(ProviderKind::OpenAICompatible, "http://localhost:11434/v1/");

    let request = DirectProviderRequest::new(&provider, "hello".to_string()).unwrap();

    assert_eq!(request.url, "http://localhost:11434/v1/chat/completions");
    assert_eq!(request.body["model"], "test-model");
    // System message + user message
    assert_eq!(request.body["messages"].as_array().unwrap().len(), 2);
    assert_eq!(request.body["messages"][1]["role"], "user");
    assert_eq!(request.body["messages"][1]["content"], "hello");
    assert_eq!(request.body["stream"], true);
}

#[test]
fn openai_request_uses_responses_payload() {
    let provider = resolved_provider(ProviderKind::OpenAI, "https://api.openai.com/v1");

    let request = DirectProviderRequest::new(&provider, "hello".to_string()).unwrap();

    assert_eq!(request.url, "https://api.openai.com/v1/responses");
    assert_eq!(request.body["model"], "test-model");
    // System message + user message
    assert_eq!(request.body["input"].as_array().unwrap().len(), 2);
    assert_eq!(request.body["input"][1]["role"], "user");
    assert_eq!(request.body["input"][1]["content"], "hello");
    assert_eq!(request.body["stream"], true);
}

#[test]
fn extracts_openai_responses_output_text() {
    let provider = resolved_provider(ProviderKind::OpenAI, "https://api.openai.com/v1");
    let request = DirectProviderRequest::new(&provider, "hello".to_string()).unwrap();

    assert_eq!(
        request
            .extract_output_text(r#"{"output_text":"direct answer"}"#)
            .unwrap(),
        "direct answer"
    );
}

#[test]
fn extracts_openai_chat_output_text() {
    let provider = resolved_provider(ProviderKind::OpenAICompatible, "http://localhost:11434/v1");
    let request = DirectProviderRequest::new(&provider, "hello".to_string()).unwrap();

    assert_eq!(
        request
            .extract_output_text(r#"{"choices":[{"message":{"content":"chat answer"}}]}"#)
            .unwrap(),
        "chat answer"
    );
}

#[test]
fn extracts_anthropic_output_text() {
    let provider = resolved_provider(ProviderKind::Anthropic, "https://api.anthropic.com/v1");
    let request = DirectProviderRequest::new(&provider, "hello".to_string()).unwrap();

    assert_eq!(
        request
            .extract_output_text(r#"{"content":[{"type":"text","text":"anthropic answer"}]}"#)
            .unwrap(),
        "anthropic answer"
    );
}

#[test]
fn classifies_provider_status_errors() {
    assert_eq!(
        DirectProviderError::from_status(StatusCode::UNAUTHORIZED, String::new()).class(),
        DirectProviderErrorClass::AuthFailure
    );
    assert_eq!(
        DirectProviderError::from_status(StatusCode::NOT_FOUND, String::new()).class(),
        DirectProviderErrorClass::MissingModel
    );
    assert_eq!(
        DirectProviderError::from_status(StatusCode::TOO_MANY_REQUESTS, String::new()).class(),
        DirectProviderErrorClass::RateLimit
    );
}

#[test]
fn telemetry_hashes_model_id_and_endpoint_origin() {
    let model_hash = direct_provider_telemetry_hash("qwen2.5-coder:14b");
    assert!(model_hash.starts_with("sha256:"));
    assert_eq!(model_hash.len(), "sha256:".len() + 64);
    assert!(!model_hash.contains("qwen"));

    let origin_hash =
        direct_provider_endpoint_origin_hash("http://127.0.0.1:11434/v1/chat/completions");
    assert_eq!(
        origin_hash,
        direct_provider_telemetry_hash("http://127.0.0.1:11434")
    );
    assert!(!origin_hash.contains("127.0.0.1"));
    assert!(!origin_hash.contains("11434"));
}

// ---------------------------------------------------------------------------
// Multi-turn conversation message building tests
// ---------------------------------------------------------------------------

#[test]
fn multi_turn_conversation_builds_alternating_messages() {
    let provider = resolved_provider(ProviderKind::OpenAICompatible, "http://localhost:11434/v1/");
    let history = vec![
        AgentExchangeSnapshot {
            user_query: "first question".to_string(),
            assistant_text: "first answer".to_string(),
            user_images: vec![],
        },
        AgentExchangeSnapshot {
            user_query: "second question".to_string(),
            assistant_text: "second answer".to_string(),
            user_images: vec![],
        },
    ];

    let messages = build_conversation_messages(
        &provider,
        None,
        &history,
        "current query".to_string(),
        vec![],
    );

    // System message + 2 history pairs (user/assistant) + current query
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "first question");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[2].content, "first answer");
    assert_eq!(messages[3].role, "user");
    assert_eq!(messages[3].content, "second question");
    assert_eq!(messages[4].role, "assistant");
    assert_eq!(messages[4].content, "second answer");
    assert_eq!(messages[5].role, "user");
    assert_eq!(messages[5].content, "current query");
}

#[test]
fn multi_turn_empty_history_only_current_query() {
    let provider = resolved_provider(ProviderKind::OpenAI, "https://api.openai.com/v1");
    let history: Vec<AgentExchangeSnapshot> = vec![];

    let messages =
        build_conversation_messages(&provider, None, &history, "hello".to_string(), vec![]);

    // System message + current query
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[1].content, "hello");
}

#[test]
fn multi_turn_openai_request_uses_conversation_history() {
    let provider = resolved_provider(ProviderKind::OpenAI, "https://api.openai.com/v1");
    let history = vec![AgentExchangeSnapshot {
        user_query: "what is rust?".to_string(),
        assistant_text: "Rust is a systems programming language.".to_string(),
        user_images: vec![],
    }];

    let request = DirectProviderRequest::new_with_messages(
        &provider,
        build_conversation_messages(
            &provider,
            None,
            &history,
            "tell me more".to_string(),
            vec![],
        ),
        &[],
    )
    .unwrap();

    assert_eq!(request.body["input"].as_array().unwrap().len(), 4);
    assert_eq!(request.body["input"][0]["role"], "system");
    assert_eq!(request.body["input"][1]["role"], "user");
    assert_eq!(request.body["input"][1]["content"], "what is rust?");
    assert_eq!(request.body["input"][2]["role"], "assistant");
    assert_eq!(
        request.body["input"][2]["content"],
        "Rust is a systems programming language."
    );
    assert_eq!(request.body["input"][3]["role"], "user");
    assert_eq!(request.body["input"][3]["content"], "tell me more");
}

#[test]
fn multi_turn_anthropic_request_uses_system_message() {
    let provider = resolved_provider(ProviderKind::Anthropic, "https://api.anthropic.com/v1");
    let history: Vec<AgentExchangeSnapshot> = vec![];

    let request = DirectProviderRequest::new_with_messages(
        &provider,
        build_conversation_messages(&provider, None, &history, "hello".to_string(), vec![]),
        &[],
    )
    .unwrap();

    // System message is passed as the `system` field in Anthropic API
    assert!(request.body.get("system").is_some());
    assert_eq!(request.body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(request.body["messages"][0]["role"], "user");
    assert_eq!(request.body["messages"][0]["content"], "hello");
}
