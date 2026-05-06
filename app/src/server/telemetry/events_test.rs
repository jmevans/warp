use warp_core::telemetry::TelemetryEventDesc;

use super::TelemetryEvent;

#[derive(Debug)]
enum TelemetryEventPropertyError {
    // The variant data is never directly read, but it's used for error formatting if the test
    // below fails.
    EmptyName(#[expect(dead_code)] Box<dyn TelemetryEventDesc>),
    EmptyDescription(#[expect(dead_code)] Box<dyn TelemetryEventDesc>),
}

/// Checks that all telemetry events have a non-empty name and description.
///
/// The name and description are intended to be user-facing and are used to populate
/// our [exhaustive telemetry table](https://docs.warp.dev/support-and-community/privacy-and-security/privacy#exhaustive-telemetry-table).
#[test]
#[cfg(not(target_family = "wasm"))]
fn telemetry_events_have_nonempty_name_and_description() -> Result<(), TelemetryEventPropertyError>
{
    for event in warp_core::telemetry::all_events() {
        if event.name().is_empty() {
            return Err(TelemetryEventPropertyError::EmptyName(event));
        }
        if event.description().is_empty() {
            return Err(TelemetryEventPropertyError::EmptyDescription(event));
        }
    }
    Ok(())
}

#[test]
fn direct_provider_started_telemetry_excludes_sensitive_values() {
    let event = TelemetryEvent::DirectProviderRequestStarted {
        provider_kind: "OpenAICompatible".to_string(),
        endpoint_origin_hash: "sha256:endpoint".to_string(),
        model_id_hash: "sha256:model".to_string(),
        is_local: Some(true),
    };

    assert!(!event.contains_ugc());
    let payload = event.payload().expect("started event should have payload");
    assert_eq!(payload["provider_kind"], "OpenAICompatible");
    assert_eq!(payload["endpoint_origin_hash"], "sha256:endpoint");
    assert_eq!(payload["model_id_hash"], "sha256:model");
    assert_eq!(payload["is_local"], true);

    let serialized = serde_json::to_string(&payload).unwrap();
    for forbidden in [
        "qwen2.5-coder:14b",
        "http://127.0.0.1:11434/v1",
        "Authorization",
        "sk-local-secret",
        "user prompt",
        "tool argument",
        "/Users/test/project",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "direct-provider started telemetry leaked {forbidden}"
        );
    }
}

#[test]
fn direct_provider_completed_telemetry_excludes_sensitive_values() {
    let event = TelemetryEvent::DirectProviderRequestCompleted {
        provider_kind: "OpenAICompatible".to_string(),
        endpoint_origin_hash: "sha256:endpoint".to_string(),
        model_id_hash: "sha256:model".to_string(),
        success: false,
        error_class: Some("AuthFailure".to_string()),
        duration_ms: 25,
        stream_duration_ms: Some(40),
        http_status_family: Some("4xx".to_string()),
        is_local: Some(false),
        tokens_prompt: Some(10),
        tokens_completion: Some(5),
        tokens_total: Some(15),
    };

    assert!(!event.contains_ugc());
    let payload = event
        .payload()
        .expect("completed event should have payload");
    assert_eq!(payload["provider_kind"], "OpenAICompatible");
    assert_eq!(payload["endpoint_origin_hash"], "sha256:endpoint");
    assert_eq!(payload["model_id_hash"], "sha256:model");
    assert_eq!(payload["error_class"], "AuthFailure");
    assert_eq!(payload["http_status_family"], "4xx");

    let serialized = serde_json::to_string(&payload).unwrap();
    for forbidden in [
        "https://proxy.example.com/v1",
        "claude-enterprise-secret-model",
        "x-api-key",
        "sk-live-secret",
        "assistant response body",
        r#"{"path":"/Users/test/project"}"#,
    ] {
        assert!(
            !serialized.contains(forbidden),
            "direct-provider completed telemetry leaked {forbidden}"
        );
    }
}
