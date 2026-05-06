use crate::provider_import::*;
use crate::provider_registry::*;

fn make_test_profile(id: &str, kind: ProviderKind, auth: AuthConfig) -> ProviderProfile {
    let mut profile = ProviderProfile::direct(
        id,
        kind,
        id,
        Some("http://localhost:11434/v1".to_string()),
        auth,
    );
    profile.discovery = ProviderDiscovery::manual_only(vec!["test-model".to_string()]);
    profile
}

#[test]
fn export_redacts_nothing_for_keychain_refs() {
    let profile = make_test_profile(
        "ollama",
        ProviderKind::OpenAICompatible,
        AuthConfig::keychain_api_key("keychain://warp/providers/ollama"),
    );
    let exported = ExportedProviderProfile::from(&profile);
    assert_eq!(exported.id, "ollama");
    assert!(matches!(
        exported.auth,
        ExportedAuthConfig::ApiKey {
            source: AuthSource::Keychain,
            ref secret_ref,
            ..
        } if secret_ref.as_ref().unwrap() == "keychain://warp/providers/ollama"
    ));
}

#[test]
fn export_preserves_env_var_names() {
    let profile = make_test_profile(
        "openai-env",
        ProviderKind::OpenAI,
        AuthConfig::env_api_key("MY_OPENAI_KEY"),
    );
    let exported = ExportedProviderProfile::from(&profile);
    assert!(matches!(
        exported.auth,
        ExportedAuthConfig::ApiKey {
            source: AuthSource::Env,
            ref env_var,
            ..
        } if env_var.as_ref().unwrap() == "MY_OPENAI_KEY"
    ));
}

#[test]
fn export_bearer_token_keeps_ref_only() {
    let auth = AuthConfig::BearerToken {
        source: AuthSource::Keychain,
        secret_ref: "keychain://warp/providers/bearer".to_string(),
    };
    let profile = make_test_profile("bearer", ProviderKind::Anthropic, auth);
    let exported = ExportedProviderProfile::from(&profile);
    assert!(matches!(
        exported.auth,
        ExportedAuthConfig::BearerToken {
            source: AuthSource::Keychain,
            ref secret_ref,
        } if secret_ref.as_ref().unwrap() == "keychain://warp/providers/bearer"
    ));
}

#[test]
fn export_no_auth() {
    let profile = make_test_profile("no-auth", ProviderKind::OpenAICompatible, AuthConfig::None);
    let exported = ExportedProviderProfile::from(&profile);
    assert!(matches!(exported.auth, ExportedAuthConfig::None));
}

#[test]
fn json_roundtrip() {
    let profile = make_test_profile(
        "roundtrip",
        ProviderKind::AnthropicCompatible,
        AuthConfig::env_api_key("TEST_KEY"),
    );
    let exported = ExportedProviderProfile::from(&profile);
    let template = ProviderExportTemplate::new(vec![exported]);
    let json = template.to_json().unwrap();
    let parsed = ProviderExportTemplate::from_json(&json).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.providers.len(), 1);
    assert_eq!(parsed.providers[0].id, "roundtrip");
}

#[test]
fn import_succeeds_with_no_collision() {
    let registry = ProviderRegistry::new();
    let exported = ExportedProviderProfile {
        id: "imported-provider".to_string(),
        kind: ProviderKind::OpenAICompatible,
        display_name: "Imported".to_string(),
        base_url: "http://localhost:11434/v1".to_string(),
        auth: ExportedAuthConfig::None,
        headers: Default::default(),
        defaults: Default::default(),
        capabilities: Default::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAICompatible),
        discovery: ProviderDiscovery::manual_only(vec!["llama3".to_string()]),
        discovered_models: vec![],
    };
    let result = import_provider(&exported, &registry).unwrap();
    assert!(!result.needs_secret);
    assert!(result.secret_hint.is_none());
    assert_eq!(result.profile.id, "imported-provider");
    assert_eq!(result.profile.kind, ProviderKind::OpenAICompatible);
}

#[test]
fn import_detects_keychain_secret_needed() {
    let registry = ProviderRegistry::new();
    let exported = ExportedProviderProfile {
        id: "keychain-provider".to_string(),
        kind: ProviderKind::OpenAI,
        display_name: "Keychain".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        auth: ExportedAuthConfig::ApiKey {
            source: AuthSource::Keychain,
            secret_ref: Some("keychain://warp/providers/keychain".to_string()),
            env_var: None,
        },
        headers: Default::default(),
        defaults: Default::default(),
        capabilities: Default::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAI),
        discovery: ProviderDiscovery::models_endpoint(),
        discovered_models: vec![],
    };
    let result = import_provider(&exported, &registry).unwrap();
    assert!(result.needs_secret);
    assert!(result.secret_hint.is_some());
}

#[test]
fn import_detects_bearer_token_secret_needed() {
    let registry = ProviderRegistry::new();
    let exported = ExportedProviderProfile {
        id: "bearer-provider".to_string(),
        kind: ProviderKind::Anthropic,
        display_name: "Bearer".to_string(),
        base_url: "https://api.anthropic.com/v1".to_string(),
        auth: ExportedAuthConfig::BearerToken {
            source: AuthSource::Keychain,
            secret_ref: Some("keychain://warp/providers/bearer".to_string()),
        },
        headers: Default::default(),
        defaults: Default::default(),
        capabilities: Default::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::Anthropic),
        discovery: ProviderDiscovery::models_endpoint(),
        discovered_models: vec![],
    };
    let result = import_provider(&exported, &registry).unwrap();
    assert!(result.needs_secret);
    assert!(result.secret_hint.is_some());
}

#[test]
fn import_rejects_id_collision() {
    let mut registry = ProviderRegistry::new();
    let profile = make_test_profile(
        "colliding-id",
        ProviderKind::OpenAICompatible,
        AuthConfig::None,
    );
    registry.upsert_provider(profile);

    let exported = ExportedProviderProfile {
        id: "colliding-id".to_string(),
        kind: ProviderKind::OpenAICompatible,
        display_name: "Collision".to_string(),
        base_url: "http://localhost:11434/v1".to_string(),
        auth: ExportedAuthConfig::None,
        headers: Default::default(),
        defaults: Default::default(),
        capabilities: Default::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAICompatible),
        discovery: ProviderDiscovery::manual_only(vec![]),
        discovered_models: vec![],
    };
    let result = import_provider(&exported, &registry);
    assert!(matches!(result, Err(ImportError::IdCollision(_))));
}

#[test]
fn import_env_var_already_set() {
    // Use a known env var that likely exists (PATH is always set)
    std::env::set_var("WARP_TEST_IMPORT_KEY", "test-value");
    let registry = ProviderRegistry::new();
    let exported = ExportedProviderProfile {
        id: "env-set-provider".to_string(),
        kind: ProviderKind::OpenAI,
        display_name: "Env Set".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        auth: ExportedAuthConfig::ApiKey {
            source: AuthSource::Env,
            secret_ref: None,
            env_var: Some("WARP_TEST_IMPORT_KEY".to_string()),
        },
        headers: Default::default(),
        defaults: Default::default(),
        capabilities: Default::default(),
        policy: ProviderPolicy::for_kind(&ProviderKind::OpenAI),
        discovery: ProviderDiscovery::models_endpoint(),
        discovered_models: vec![],
    };
    let result = import_provider(&exported, &registry).unwrap();
    assert!(!result.needs_secret);
    std::env::remove_var("WARP_TEST_IMPORT_KEY");
}

#[test]
fn export_filters_warp_hosted_models() {
    let mut profile = ProviderProfile::warp_hosted();
    profile.discovered_models.push(DiscoveredModel {
        id: "gpt-4".to_string(),
        display_name: Some("GPT-4".to_string()),
        source: DiscoveredModelSource::WarpHosted,
        capabilities: Default::default(),
    });
    let exported = ExportedProviderProfile::from(&profile);
    // Warp hosted provider itself is filtered at the call site, but
    // the From impl also filters out WarpHosted-sourced models.
    assert!(exported.discovered_models.is_empty());
}
