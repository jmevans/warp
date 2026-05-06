use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api_keys::ApiKeys;
use crate::provider_validator::{DiscoveredModelInfo, ValidationCacheEntry, ValidationResult};

pub const WARP_HOSTED_PROVIDER_ID: &str = "warp-hosted";
pub const LEGACY_OPENAI_PROVIDER_ID: &str = "openai-main";
pub const LEGACY_ANTHROPIC_PROVIDER_ID: &str = "anthropic-main";

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    WarpHosted,
    OpenAI,
    Anthropic,
    OpenAICompatible,
    AnthropicCompatible,
}

impl ProviderKind {
    pub fn is_direct(&self) -> bool {
        !matches!(self, ProviderKind::WarpHosted)
    }

    pub fn is_compatible(&self) -> bool {
        matches!(
            self,
            ProviderKind::OpenAICompatible | ProviderKind::AnthropicCompatible
        )
    }

    fn default_base_url(&self) -> String {
        match self {
            ProviderKind::WarpHosted => String::new(),
            ProviderKind::OpenAI => "https://api.openai.com/v1".to_string(),
            ProviderKind::Anthropic => "https://api.anthropic.com/v1".to_string(),
            ProviderKind::OpenAICompatible | ProviderKind::AnthropicCompatible => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AuthSource {
    Keychain,
    Env,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthConfig {
    None,
    ApiKey {
        source: AuthSource,
        #[serde(rename = "secretRef", skip_serializing_if = "Option::is_none")]
        secret_ref: Option<String>,
        #[serde(rename = "envVar", skip_serializing_if = "Option::is_none")]
        env_var: Option<String>,
    },
    BearerToken {
        source: AuthSource,
        #[serde(rename = "secretRef")]
        secret_ref: String,
    },
}

impl AuthConfig {
    pub fn keychain_api_key(secret_ref: impl Into<String>) -> Self {
        Self::ApiKey {
            source: AuthSource::Keychain,
            secret_ref: Some(secret_ref.into()),
            env_var: None,
        }
    }

    pub fn env_api_key(env_var: impl Into<String>) -> Self {
        Self::ApiKey {
            source: AuthSource::Env,
            secret_ref: None,
            env_var: Some(env_var.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum CapabilitySetting {
    Automatic(String),
    Explicit(bool),
}

impl CapabilitySetting {
    pub fn automatic() -> Self {
        Self::Automatic("auto".to_string())
    }

    pub fn enabled() -> Self {
        Self::Explicit(true)
    }

    pub fn disabled() -> Self {
        Self::Explicit(false)
    }

    pub fn is_enabled(&self) -> Option<bool> {
        match self {
            CapabilitySetting::Automatic(_) => None,
            CapabilitySetting::Explicit(enabled) => Some(*enabled),
        }
    }
}

impl Default for CapabilitySetting {
    fn default() -> Self {
        Self::automatic()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    #[serde(default)]
    pub tools: CapabilitySetting,
    #[serde(default)]
    pub vision: CapabilitySetting,
    #[serde(default)]
    pub structured_outputs: CapabilitySetting,
    #[serde(default)]
    pub prompt_caching: CapabilitySetting,
    #[serde(default)]
    pub reasoning_controls: CapabilitySetting,
    #[serde(default)]
    pub model_discovery: CapabilitySetting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPolicy {
    pub allow_fallback_to_warp_hosted: bool,
    pub allow_manual_model_ids: bool,
    pub redact_telemetry: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_in_managed_teams: Option<bool>,
}

impl ProviderPolicy {
    pub fn for_kind(kind: &ProviderKind) -> Self {
        Self {
            allow_fallback_to_warp_hosted: false,
            allow_manual_model_ids: kind.is_compatible(),
            redact_telemetry: true,
            local_only: None,
            allow_in_managed_teams: None,
        }
    }

    pub fn warp_hosted() -> Self {
        Self {
            allow_fallback_to_warp_hosted: false,
            allow_manual_model_ids: false,
            redact_telemetry: true,
            local_only: Some(false),
            allow_in_managed_teams: Some(true),
        }
    }

    pub fn allows_warp_hosted_fallback(&self) -> bool {
        self.allow_fallback_to_warp_hosted && self.local_only != Some(true)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(rename = "topP", skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(rename = "timeoutMs", skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveryStrategy {
    ModelsEndpoint,
    ManualOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiscovery {
    pub strategy: DiscoveryStrategy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_models: Vec<String>,
}

impl ProviderDiscovery {
    pub fn models_endpoint() -> Self {
        Self {
            strategy: DiscoveryStrategy::ModelsEndpoint,
            fallback_models: vec![],
        }
    }

    pub fn manual_only(models: Vec<String>) -> Self {
        Self {
            strategy: DiscoveryStrategy::ManualOnly,
            fallback_models: models,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoveredModelSource {
    WarpHosted,
    Discovered,
    Manual,
    Legacy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub source: DiscoveredModelSource,
    #[serde(default)]
    pub capabilities: ProviderCapabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub kind: ProviderKind,
    pub enabled: bool,
    pub display_name: String,
    #[serde(rename = "baseURL")]
    pub base_url: String,
    pub auth: AuthConfig,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub defaults: ProviderDefaults,
    #[serde(default)]
    pub capabilities: ProviderCapabilities,
    pub policy: ProviderPolicy,
    pub discovery: ProviderDiscovery,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovered_models: Vec<DiscoveredModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_cache: Option<ValidationCacheEntry>,
}

impl ProviderProfile {
    pub fn warp_hosted() -> Self {
        Self {
            id: WARP_HOSTED_PROVIDER_ID.to_string(),
            kind: ProviderKind::WarpHosted,
            enabled: true,
            display_name: "Warp-hosted".to_string(),
            base_url: String::new(),
            auth: AuthConfig::None,
            headers: BTreeMap::new(),
            defaults: ProviderDefaults {
                model: Some("auto".to_string()),
                stream: Some(true),
                ..ProviderDefaults::default()
            },
            capabilities: ProviderCapabilities::default(),
            policy: ProviderPolicy::warp_hosted(),
            discovery: ProviderDiscovery::models_endpoint(),
            discovered_models: vec![],
            validation_cache: None,
        }
    }

    pub fn direct(
        id: impl Into<String>,
        kind: ProviderKind,
        display_name: impl Into<String>,
        base_url: Option<String>,
        auth: AuthConfig,
    ) -> Self {
        let id = id.into();
        let base_url = base_url.unwrap_or_else(|| kind.default_base_url());
        let mut policy = ProviderPolicy::for_kind(&kind);
        policy.local_only = infer_local_only(&base_url);
        Self {
            id,
            kind,
            enabled: true,
            display_name: display_name.into(),
            base_url,
            auth,
            headers: BTreeMap::new(),
            defaults: ProviderDefaults {
                stream: Some(true),
                ..ProviderDefaults::default()
            },
            capabilities: ProviderCapabilities::default(),
            policy,
            discovery: ProviderDiscovery::models_endpoint(),
            discovered_models: vec![],
            validation_cache: None,
        }
    }

    pub fn from_legacy_openai_key() -> Self {
        let mut profile = Self::direct(
            LEGACY_OPENAI_PROVIDER_ID,
            ProviderKind::OpenAI,
            "OpenAI direct",
            None,
            AuthConfig::keychain_api_key("legacy://AiApiKeys/openai"),
        );
        profile.discovery.fallback_models = vec!["gpt-4.1".to_string()];
        profile
    }

    pub fn from_legacy_anthropic_key() -> Self {
        let mut profile = Self::direct(
            LEGACY_ANTHROPIC_PROVIDER_ID,
            ProviderKind::Anthropic,
            "Anthropic direct",
            None,
            AuthConfig::keychain_api_key("legacy://AiApiKeys/anthropic"),
        );
        profile.discovery.fallback_models = vec!["claude-sonnet-4".to_string()];
        profile
    }

    pub fn model_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        if let Some(model) = &self.defaults.model {
            ids.push(model.clone());
        }
        ids.extend(self.discovered_models.iter().map(|model| model.id.clone()));
        ids.extend(self.discovery.fallback_models.iter().cloned());
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn is_stale(&self) -> bool {
        const DEFAULT_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
        self.validation_cache
            .as_ref()
            .is_some_and(|cache| cache.is_stale(DEFAULT_MAX_AGE))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRegistryDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRegistry {
    #[serde(default)]
    pub defaults: ProviderRegistryDefaults,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderProfile>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            WARP_HOSTED_PROVIDER_ID.to_string(),
            ProviderProfile::warp_hosted(),
        );
        Self {
            defaults: ProviderRegistryDefaults {
                agent_provider: Some(WARP_HOSTED_PROVIDER_ID.to_string()),
                agent_model: Some("auto".to_string()),
            },
            providers,
        }
    }

    pub fn from_existing(existing: Option<Self>, legacy_keys: &ApiKeys) -> Self {
        let mut registry = existing.unwrap_or_default();
        registry.ensure_warp_hosted();

        if legacy_keys.openai.is_some()
            && !registry.providers.contains_key(LEGACY_OPENAI_PROVIDER_ID)
        {
            registry.providers.insert(
                LEGACY_OPENAI_PROVIDER_ID.to_string(),
                ProviderProfile::from_legacy_openai_key(),
            );
        }

        if legacy_keys.anthropic.is_some()
            && !registry
                .providers
                .contains_key(LEGACY_ANTHROPIC_PROVIDER_ID)
        {
            registry.providers.insert(
                LEGACY_ANTHROPIC_PROVIDER_ID.to_string(),
                ProviderProfile::from_legacy_anthropic_key(),
            );
        }

        registry
    }

    pub fn ensure_warp_hosted(&mut self) {
        self.providers
            .entry(WARP_HOSTED_PROVIDER_ID.to_string())
            .or_insert_with(ProviderProfile::warp_hosted);
        self.defaults
            .agent_provider
            .get_or_insert_with(|| WARP_HOSTED_PROVIDER_ID.to_string());
        self.defaults
            .agent_model
            .get_or_insert_with(|| "auto".to_string());
    }

    pub fn upsert_provider(&mut self, mut profile: ProviderProfile) {
        // Preserve existing validation cache when updating a provider
        if let Some(existing) = self.providers.get(&profile.id) {
            if profile.validation_cache.is_none() {
                profile.validation_cache = existing.validation_cache.clone();
            }
        }
        profile.policy.local_only = profile
            .policy
            .local_only
            .or_else(|| infer_local_only(&profile.base_url));
        self.providers.insert(profile.id.clone(), profile);
    }

    pub fn apply_validation_result(&mut self, provider_id: &str, result: &ValidationResult) {
        let Some(provider) = self.providers.get_mut(provider_id) else {
            return;
        };

        provider.validation_cache = Some(ValidationCacheEntry::from_validation_result(result));

        if let Some(models) = &result.discovered_models {
            provider.discovered_models = discovered_model_infos_to_models(models);
            if !provider.discovered_models.is_empty()
                && provider.discovery.strategy != DiscoveryStrategy::ManualOnly
            {
                provider.discovery.strategy = DiscoveryStrategy::ModelsEndpoint;
            }
        }

        if let Some(model) = &result.model {
            let model_known = provider.model_ids().iter().any(|id| id == model);
            if !model_known {
                provider.discovery.fallback_models.push(model.clone());
                provider.discovery.fallback_models.sort();
                provider.discovery.fallback_models.dedup();
            }
        }
    }

    pub fn apply_discovered_models(&mut self, provider_id: &str, models: Vec<DiscoveredModel>) {
        let Some(provider) = self.providers.get_mut(provider_id) else {
            return;
        };

        provider.discovered_models = models;
        if !provider.discovered_models.is_empty() {
            provider.discovery.strategy = DiscoveryStrategy::ModelsEndpoint;
        }
    }

    pub fn enabled_model_choices(&self) -> Vec<ProviderModelChoice> {
        self.providers
            .values()
            .filter(|provider| provider.enabled)
            .flat_map(|provider| {
                provider
                    .model_ids()
                    .into_iter()
                    .map(|model_id| ProviderModelChoice::new(provider, model_id))
            })
            .collect()
    }

    pub fn resolve_agent_provider(
        &self,
        requested_model: Option<&str>,
    ) -> Result<ResolvedProvider, ProviderResolutionError> {
        let selection = match requested_model {
            Some(model) => ProviderQualifiedModelId::parse(model)
                .unwrap_or_else(|| ProviderQualifiedModelId::new(WARP_HOSTED_PROVIDER_ID, model)),
            None => {
                let provider_id = self
                    .defaults
                    .agent_provider
                    .clone()
                    .unwrap_or_else(|| WARP_HOSTED_PROVIDER_ID.to_string());
                let model_id = self
                    .defaults
                    .agent_model
                    .clone()
                    .unwrap_or_else(|| "auto".to_string());
                ProviderQualifiedModelId {
                    provider_id,
                    model_id,
                }
            }
        };

        let provider = self.providers.get(&selection.provider_id).ok_or_else(|| {
            ProviderResolutionError::UnknownProvider {
                provider_id: selection.provider_id.clone(),
            }
        })?;

        if !provider.enabled {
            return Err(ProviderResolutionError::DisabledProvider {
                provider_id: provider.id.clone(),
            });
        }

        let model_known = provider
            .model_ids()
            .iter()
            .any(|id| id == &selection.model_id);
        let model_from_discovery = provider
            .discovered_models
            .iter()
            .any(|m| m.id == selection.model_id);
        let discovery_is_stale = provider.is_stale();

        if model_from_discovery && discovery_is_stale && !provider.policy.allow_manual_model_ids {
            return Err(ProviderResolutionError::StaleModel {
                provider_id: provider.id.clone(),
                model_id: selection.model_id,
            });
        }

        if provider.kind.is_direct() && !model_known && !provider.policy.allow_manual_model_ids {
            return Err(ProviderResolutionError::UnknownModel {
                provider_id: provider.id.clone(),
                model_id: selection.model_id,
            });
        }

        Ok(ResolvedProvider {
            profile_id: provider.id.clone(),
            kind: provider.kind.clone(),
            model: selection.model_id,
            base_url: provider.base_url.clone(),
            auth: ResolvedAuth::from(&provider.auth),
            headers: provider.headers.clone(),
            capabilities: provider.capabilities.clone(),
            policy: provider.policy.clone(),
            defaults: provider.defaults.clone(),
        })
    }
}

fn discovered_model_infos_to_models(models: &[DiscoveredModelInfo]) -> Vec<DiscoveredModel> {
    models
        .iter()
        .map(|model| DiscoveredModel {
            id: model.id.clone(),
            display_name: model.display_name.clone(),
            source: DiscoveredModelSource::Discovered,
            capabilities: ProviderCapabilities::default(),
        })
        .collect()
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl settings_value::SettingsValue for ProviderRegistry {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderQualifiedModelId {
    pub provider_id: String,
    pub model_id: String,
}

impl ProviderQualifiedModelId {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let (provider_id, model_id) = value.split_once(':')?;
        if provider_id.is_empty() || model_id.is_empty() {
            return None;
        }
        Some(Self::new(provider_id, model_id))
    }
}

impl std::fmt::Display for ProviderQualifiedModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.provider_id, self.model_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModelChoice {
    pub qualified_id: ProviderQualifiedModelId,
    pub provider_display_name: String,
    pub model_id: String,
    pub provider_kind: ProviderKind,
    pub local_only: bool,
    pub manual: bool,
}

impl ProviderModelChoice {
    fn new(provider: &ProviderProfile, model_id: String) -> Self {
        let manual = provider
            .discovery
            .fallback_models
            .iter()
            .any(|fallback| fallback == &model_id);
        Self {
            qualified_id: ProviderQualifiedModelId::new(&provider.id, &model_id),
            provider_display_name: provider.display_name.clone(),
            model_id,
            provider_kind: provider.kind.clone(),
            local_only: provider.policy.local_only == Some(true),
            manual,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedAuth {
    None,
    KeychainApiKey { secret_ref: Option<String> },
    EnvApiKey { env_var: Option<String> },
    KeychainBearerToken { secret_ref: String },
}

impl From<&AuthConfig> for ResolvedAuth {
    fn from(auth: &AuthConfig) -> Self {
        match auth {
            AuthConfig::None => ResolvedAuth::None,
            AuthConfig::ApiKey {
                source: AuthSource::Keychain,
                secret_ref,
                ..
            } => ResolvedAuth::KeychainApiKey {
                secret_ref: secret_ref.clone(),
            },
            AuthConfig::ApiKey {
                source: AuthSource::Env,
                env_var,
                ..
            } => ResolvedAuth::EnvApiKey {
                env_var: env_var.clone(),
            },
            AuthConfig::BearerToken { secret_ref, .. } => ResolvedAuth::KeychainBearerToken {
                secret_ref: secret_ref.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProvider {
    pub profile_id: String,
    pub kind: ProviderKind,
    pub model: String,
    pub base_url: String,
    pub auth: ResolvedAuth,
    pub headers: BTreeMap<String, String>,
    pub capabilities: ProviderCapabilities,
    pub policy: ProviderPolicy,
    pub defaults: ProviderDefaults,
}

impl ResolvedProvider {
    /// Validates this resolved provider against team policy constraints.
    ///
    /// `is_direct_allowed`, `is_compatible_allowed`, and `is_endpoint_origin_allowed`
    /// should come from `UserWorkspaces::is_direct_provider_allowed()`,
    /// `UserWorkspaces::is_compatible_endpoint_allowed()`, and
    /// `UserWorkspaces::is_endpoint_origin_allowed()`.
    /// `is_managed_team` should come from `UserWorkspaces::is_on_managed_team()`.
    pub fn validate_team_policy(
        &self,
        is_direct_allowed: bool,
        is_compatible_allowed: bool,
        is_endpoint_origin_allowed: bool,
        is_managed_team: bool,
    ) -> Result<(), ProviderResolutionError> {
        if self.kind.is_direct() && !is_direct_allowed {
            return Err(ProviderResolutionError::TeamPolicyBlocked {
                provider_id: self.profile_id.clone(),
                reason: "Direct providers are disabled by team policy".to_string(),
            });
        }
        if self.kind.is_compatible() && !is_compatible_allowed {
            return Err(ProviderResolutionError::TeamPolicyBlocked {
                provider_id: self.profile_id.clone(),
                reason: "Custom compatible endpoints are disabled by team policy".to_string(),
            });
        }
        if self.kind.is_compatible() && !is_endpoint_origin_allowed {
            return Err(ProviderResolutionError::TeamPolicyBlocked {
                provider_id: self.profile_id.clone(),
                reason: "Provider endpoint origin is not allowed by team policy".to_string(),
            });
        }
        if is_managed_team && self.policy.allow_in_managed_teams != Some(true) {
            return Err(ProviderResolutionError::TeamPolicyBlocked {
                provider_id: self.profile_id.clone(),
                reason: "This provider is not allowed in managed teams".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderResolutionError {
    UnknownProvider {
        provider_id: String,
    },
    DisabledProvider {
        provider_id: String,
    },
    UnknownModel {
        provider_id: String,
        model_id: String,
    },
    TeamPolicyBlocked {
        provider_id: String,
        reason: String,
    },
    StaleModel {
        provider_id: String,
        model_id: String,
    },
}

impl std::fmt::Display for ProviderResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderResolutionError::UnknownProvider { provider_id } => {
                write!(f, "Unknown provider '{provider_id}'")
            }
            ProviderResolutionError::DisabledProvider { provider_id } => {
                write!(f, "Provider '{provider_id}' is disabled")
            }
            ProviderResolutionError::UnknownModel {
                provider_id,
                model_id,
            } => {
                write!(
                    f,
                    "Provider '{provider_id}' does not know model '{model_id}'"
                )
            }
            ProviderResolutionError::TeamPolicyBlocked {
                provider_id,
                reason,
            } => {
                write!(
                    f,
                    "Provider '{provider_id}' is blocked by team policy: {reason}"
                )
            }
            ProviderResolutionError::StaleModel {
                provider_id,
                model_id,
            } => {
                write!(
                    f,
                    "Provider '{provider_id}' model '{model_id}' has stale discovery data"
                )
            }
        }
    }
}

impl std::error::Error for ProviderResolutionError {}

pub fn infer_local_only(base_url: &str) -> Option<bool> {
    let url = url::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    Some(matches!(
        host,
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"
    ))
}

#[cfg(test)]
#[path = "provider_registry_tests.rs"]
mod tests;
