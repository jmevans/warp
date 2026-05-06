use std::collections::BTreeMap;

use crate::provider_validator::{
    DiscoveredModelInfo, ValidationError, ValidationResult, ValidationStatus,
};

use super::*;

#[test]
fn default_registry_synthesizes_warp_hosted_provider() {
    let registry = ProviderRegistry::default();

    let provider = registry
        .providers
        .get(WARP_HOSTED_PROVIDER_ID)
        .expect("warp-hosted provider should exist");
    assert_eq!(provider.kind, ProviderKind::WarpHosted);
    assert_eq!(provider.defaults.model.as_deref(), Some("auto"));
    assert_eq!(
        registry.defaults.agent_provider.as_deref(),
        Some(WARP_HOSTED_PROVIDER_ID)
    );
}

#[test]
fn direct_provider_policy_defaults_disable_warp_fallback() {
    let openai = ProviderProfile::direct(
        "openai-main",
        ProviderKind::OpenAI,
        "OpenAI direct",
        None,
        AuthConfig::env_api_key("OPENAI_API_KEY"),
    );

    assert!(!openai.policy.allow_fallback_to_warp_hosted);
    assert!(!openai.policy.allow_manual_model_ids);
    assert!(openai.policy.redact_telemetry);

    let compatible = ProviderProfile::direct(
        "ollama-local",
        ProviderKind::OpenAICompatible,
        "Ollama local",
        Some("http://127.0.0.1:11434/v1".to_string()),
        AuthConfig::None,
    );

    assert!(compatible.policy.allow_manual_model_ids);
    assert_eq!(compatible.policy.local_only, Some(true));
    assert!(!compatible.policy.allows_warp_hosted_fallback());
}

#[test]
fn legacy_migration_creates_first_party_profiles_without_google_or_openrouter() {
    let legacy_keys = ApiKeys {
        openai: Some("sk-openai".to_string()),
        anthropic: Some("sk-ant".to_string()),
        google: Some("google".to_string()),
        open_router: Some("openrouter".to_string()),
    };

    let registry = ProviderRegistry::from_existing(None, &legacy_keys);

    assert!(registry.providers.contains_key(WARP_HOSTED_PROVIDER_ID));
    assert!(registry.providers.contains_key(LEGACY_OPENAI_PROVIDER_ID));
    assert!(registry
        .providers
        .contains_key(LEGACY_ANTHROPIC_PROVIDER_ID));
    assert_eq!(registry.providers.len(), 3);
    assert_eq!(
        registry.providers[LEGACY_OPENAI_PROVIDER_ID].auth,
        AuthConfig::keychain_api_key("legacy://AiApiKeys/openai")
    );
}

#[test]
fn qualified_model_id_preserves_colons_in_provider_model_id() {
    let qualified = ProviderQualifiedModelId::parse("ollama-local:qwen2.5-coder:14b")
        .expect("qualified model id should parse");

    assert_eq!(qualified.provider_id, "ollama-local");
    assert_eq!(qualified.model_id, "qwen2.5-coder:14b");
    assert_eq!(qualified.to_string(), "ollama-local:qwen2.5-coder:14b");
}

#[test]
fn resolver_prefers_requested_provider_qualified_model() {
    let mut registry = ProviderRegistry::default();
    let mut ollama = ProviderProfile::direct(
        "ollama-local",
        ProviderKind::OpenAICompatible,
        "Ollama local",
        Some("http://localhost:11434/v1".to_string()),
        AuthConfig::None,
    );
    ollama.discovery = ProviderDiscovery::manual_only(vec!["llama3.1:8b".to_string()]);
    registry.upsert_provider(ollama);
    registry.defaults = ProviderRegistryDefaults {
        agent_provider: Some(WARP_HOSTED_PROVIDER_ID.to_string()),
        agent_model: Some("auto".to_string()),
    };

    let resolved = registry
        .resolve_agent_provider(Some("ollama-local:llama3.1:8b"))
        .expect("manual compatible model should resolve");

    assert_eq!(resolved.profile_id, "ollama-local");
    assert_eq!(resolved.kind, ProviderKind::OpenAICompatible);
    assert_eq!(resolved.model, "llama3.1:8b");
    assert_eq!(resolved.auth, ResolvedAuth::None);
    assert_eq!(resolved.policy.local_only, Some(true));
}

#[test]
fn resolver_uses_registry_defaults_when_no_override_is_present() {
    let mut registry = ProviderRegistry::default();
    let mut openai = ProviderProfile::direct(
        "openai-main",
        ProviderKind::OpenAI,
        "OpenAI direct",
        None,
        AuthConfig::env_api_key("OPENAI_API_KEY"),
    );
    openai.defaults.model = Some("gpt-4.1".to_string());
    registry.upsert_provider(openai);
    registry.defaults = ProviderRegistryDefaults {
        agent_provider: Some("openai-main".to_string()),
        agent_model: Some("gpt-4.1".to_string()),
    };

    let resolved = registry
        .resolve_agent_provider(None)
        .expect("registry default should resolve");

    assert_eq!(resolved.profile_id, "openai-main");
    assert_eq!(resolved.model, "gpt-4.1");
    assert_eq!(
        resolved.auth,
        ResolvedAuth::EnvApiKey {
            env_var: Some("OPENAI_API_KEY".to_string())
        }
    );
}

#[test]
fn resolver_treats_unqualified_existing_model_ids_as_warp_hosted() {
    let registry = ProviderRegistry::default();

    let resolved = registry
        .resolve_agent_provider(Some("claude-sonnet"))
        .expect("legacy unqualified model ids should resolve through warp-hosted");

    assert_eq!(resolved.profile_id, WARP_HOSTED_PROVIDER_ID);
    assert_eq!(resolved.kind, ProviderKind::WarpHosted);
    assert_eq!(resolved.model, "claude-sonnet");
}

#[test]
fn resolver_rejects_disabled_provider() {
    let mut registry = ProviderRegistry::default();
    let mut profile = ProviderProfile::direct(
        "openai-main",
        ProviderKind::OpenAI,
        "OpenAI direct",
        None,
        AuthConfig::env_api_key("OPENAI_API_KEY"),
    );
    profile.enabled = false;
    profile.defaults.model = Some("gpt-4.1".to_string());
    registry.upsert_provider(profile);

    let error = registry
        .resolve_agent_provider(Some("openai-main:gpt-4.1"))
        .expect_err("disabled provider should not resolve");

    assert_eq!(
        error,
        ProviderResolutionError::DisabledProvider {
            provider_id: "openai-main".to_string()
        }
    );
}

#[test]
fn resolver_rejects_unknown_first_party_model_without_manual_policy() {
    let mut registry = ProviderRegistry::default();
    let profile = ProviderProfile::direct(
        "openai-main",
        ProviderKind::OpenAI,
        "OpenAI direct",
        None,
        AuthConfig::env_api_key("OPENAI_API_KEY"),
    );
    registry.upsert_provider(profile);

    let error = registry
        .resolve_agent_provider(Some("openai-main:made-up-model"))
        .expect_err("first-party unknown model should be rejected");

    assert_eq!(
        error,
        ProviderResolutionError::UnknownModel {
            provider_id: "openai-main".to_string(),
            model_id: "made-up-model".to_string()
        }
    );
}

#[test]
fn enabled_choices_include_provider_provenance() {
    let mut registry = ProviderRegistry {
        defaults: ProviderRegistryDefaults::default(),
        providers: BTreeMap::new(),
    };
    let mut profile = ProviderProfile::direct(
        "proxy",
        ProviderKind::AnthropicCompatible,
        "Enterprise proxy",
        Some("https://llm-proxy.example.com/v1".to_string()),
        AuthConfig::keychain_api_key("keychain://warp/providers/proxy"),
    );
    profile.discovery = ProviderDiscovery::manual_only(vec!["claude-proxy".to_string()]);
    registry.upsert_provider(profile);

    let choices = registry.enabled_model_choices();

    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].provider_display_name, "Enterprise proxy");
    assert_eq!(choices[0].provider_kind, ProviderKind::AnthropicCompatible);
    assert!(choices[0].manual);
    assert!(!choices[0].local_only);
}

#[test]
fn auth_config_serializes_to_cross_language_contract_shape() {
    let auth = AuthConfig::keychain_api_key("keychain://warp/providers/openai-main");

    let value = serde_json::to_value(auth).expect("auth should serialize");

    assert_eq!(
        value,
        serde_json::json!({
            "type": "apiKey",
            "source": "keychain",
            "secretRef": "keychain://warp/providers/openai-main"
        })
    );
}

#[test]
fn team_policy_allows_direct_when_allowed() {
    let resolved = ResolvedProvider {
        profile_id: "openai-main".to_string(),
        kind: ProviderKind::OpenAI,
        model: "gpt-4.1".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        auth: ResolvedAuth::EnvApiKey {
            env_var: Some("OPENAI_API_KEY".to_string()),
        },
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAI),
        defaults: ProviderDefaults::default(),
    };
    assert!(resolved.validate_team_policy(true, true, true, false).is_ok());
}

#[test]
fn team_policy_blocks_direct_when_disallowed() {
    let resolved = ResolvedProvider {
        profile_id: "openai-main".to_string(),
        kind: ProviderKind::OpenAI,
        model: "gpt-4.1".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        auth: ResolvedAuth::EnvApiKey {
            env_var: Some("OPENAI_API_KEY".to_string()),
        },
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAI),
        defaults: ProviderDefaults::default(),
    };
    let err = resolved
        .validate_team_policy(false, true, true, false)
        .expect_err("should be blocked");
    assert!(matches!(
        err,
        ProviderResolutionError::TeamPolicyBlocked { .. }
    ));
}

#[test]
fn team_policy_blocks_compatible_when_disallowed() {
    let resolved = ResolvedProvider {
        profile_id: "ollama-local".to_string(),
        kind: ProviderKind::OpenAICompatible,
        model: "llama3".to_string(),
        base_url: "http://localhost:11434/v1".to_string(),
        auth: ResolvedAuth::None,
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAICompatible),
        defaults: ProviderDefaults::default(),
    };
    let err = resolved
        .validate_team_policy(true, false, true, false)
        .expect_err("should be blocked");
    assert!(matches!(
        err,
        ProviderResolutionError::TeamPolicyBlocked { .. }
    ));
}

#[test]
fn team_policy_allows_warp_hosted_regardless() {
    let registry = ProviderRegistry::default();
    let resolved = registry
        .resolve_agent_provider(Some("auto"))
        .expect("warp-hosted should resolve");
    // Warp-hosted is neither direct nor compatible, so it should always pass
    assert!(resolved.validate_team_policy(false, false, false, false).is_ok());
}

#[test]
fn team_policy_blocks_compatible_when_endpoint_origin_disallowed() {
    let resolved = ResolvedProvider {
        profile_id: "enterprise-proxy".to_string(),
        kind: ProviderKind::OpenAICompatible,
        model: "proxy-model".to_string(),
        base_url: "https://unapproved.example.com/v1".to_string(),
        auth: ResolvedAuth::None,
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAICompatible),
        defaults: ProviderDefaults::default(),
    };

    let err = resolved
        .validate_team_policy(true, true, false, false)
        .expect_err("endpoint origin should be blocked");

    assert!(matches!(
        err,
        ProviderResolutionError::TeamPolicyBlocked { reason, .. }
            if reason.contains("endpoint origin")
    ));
}

#[test]
fn team_policy_blocks_byo_when_managed_team_disallows() {
    let resolved = ResolvedProvider {
        profile_id: "openai-main".to_string(),
        kind: ProviderKind::OpenAI,
        model: "gpt-4.1".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        auth: ResolvedAuth::EnvApiKey {
            env_var: Some("OPENAI_API_KEY".to_string()),
        },
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        // Default policy has allow_in_managed_teams = None
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAI),
        defaults: ProviderDefaults::default(),
    };
    // Passes for solo users
    assert!(resolved.validate_team_policy(true, true, true, false).is_ok());
    // Blocked for managed teams when allow_in_managed_teams is None
    let err = resolved
        .validate_team_policy(true, true, true, true)
        .expect_err("should be blocked for managed team");
    assert!(matches!(
        err,
        ProviderResolutionError::TeamPolicyBlocked { .. }
    ));
}

#[test]
fn team_policy_allows_byo_when_managed_team_explicitly_allows() {
    let mut policy = ProviderPolicy::for_kind(&ProviderKind::OpenAI);
    policy.allow_in_managed_teams = Some(true);
    let resolved = ResolvedProvider {
        profile_id: "openai-enterprise".to_string(),
        kind: ProviderKind::OpenAI,
        model: "gpt-4.1".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        auth: ResolvedAuth::EnvApiKey {
            env_var: Some("OPENAI_API_KEY".to_string()),
        },
        headers: BTreeMap::new(),
        capabilities: ProviderCapabilities::default(),
        policy,
        defaults: ProviderDefaults::default(),
    };
    // Allowed when explicitly permitted even for managed teams
    assert!(resolved.validate_team_policy(true, true, true, true).is_ok());
}

#[test]
fn applying_validation_result_persists_cache_and_discovered_models() {
    let mut registry = ProviderRegistry::default();
    let provider = ProviderProfile::direct(
        "openai-main",
        ProviderKind::OpenAI,
        "OpenAI direct",
        None,
        AuthConfig::env_api_key("OPENAI_API_KEY"),
    );
    registry.upsert_provider(provider);

    let result = ValidationResult {
        status: ValidationStatus::Success,
        error: None,
        discovered_models: Some(vec![DiscoveredModelInfo {
            id: "gpt-4.1".to_string(),
            display_name: Some("GPT 4.1".to_string()),
        }]),
        response_time_ms: Some(42),
        model: None,
    };

    registry.apply_validation_result("openai-main", &result);

    let provider = registry.providers.get("openai-main").unwrap();
    assert_eq!(
        provider
            .validation_cache
            .as_ref()
            .map(|cache| &cache.status),
        Some(&ValidationStatus::Success)
    );
    assert_eq!(provider.discovered_models.len(), 1);
    assert_eq!(provider.discovered_models[0].id, "gpt-4.1");
    assert_eq!(
        provider.discovered_models[0].source,
        DiscoveredModelSource::Discovered
    );
    assert_eq!(
        provider.discovery.strategy,
        DiscoveryStrategy::ModelsEndpoint
    );
}

#[test]
fn applying_failed_validation_result_keeps_error_cache() {
    let mut registry = ProviderRegistry::default();
    let provider = ProviderProfile::direct(
        "openai-main",
        ProviderKind::OpenAI,
        "OpenAI direct",
        None,
        AuthConfig::env_api_key("OPENAI_API_KEY"),
    );
    registry.upsert_provider(provider);

    let result = ValidationResult {
        status: ValidationStatus::Failed,
        error: Some(ValidationError::AuthFailure {
            detail: "bad key".to_string(),
            http_status: 401,
        }),
        discovered_models: None,
        response_time_ms: Some(12),
        model: None,
    };

    registry.apply_validation_result("openai-main", &result);

    let cache = registry.providers["openai-main"]
        .validation_cache
        .as_ref()
        .expect("failed validation should still be cached");
    assert_eq!(cache.status, ValidationStatus::Failed);
    assert!(matches!(
        cache.error,
        Some(ValidationError::AuthFailure {
            http_status: 401,
            ..
        })
    ));
}

#[test]
fn applying_deep_validation_model_adds_manual_fallback_model() {
    let mut registry = ProviderRegistry::default();
    let provider = ProviderProfile::direct(
        "ollama-local",
        ProviderKind::OpenAICompatible,
        "Ollama local",
        Some("http://localhost:11434/v1".to_string()),
        AuthConfig::None,
    );
    registry.upsert_provider(provider);

    let result = ValidationResult {
        status: ValidationStatus::Success,
        error: None,
        discovered_models: None,
        response_time_ms: Some(30),
        model: Some("llama3.1:8b".to_string()),
    };

    registry.apply_validation_result("ollama-local", &result);

    assert_eq!(
        registry.providers["ollama-local"].discovery.fallback_models,
        vec!["llama3.1:8b".to_string()]
    );
}
