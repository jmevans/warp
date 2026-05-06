//! Import/export support for provider profiles.
//!
//! Export produces a redacted JSON representation of one or more provider profiles,
//! with secrets stripped (keychain refs are kept as references, not values).
//!
//! Import parses a JSON provider template, validates its structure, and returns
//! the parsed profiles so the caller can merge them into the registry.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::provider_registry::{
    AuthConfig, AuthSource, DiscoveredModel, DiscoveredModelSource, ProviderCapabilities,
    ProviderDefaults, ProviderDiscovery, ProviderKind, ProviderPolicy, ProviderProfile,
    ProviderRegistry,
};

/// Redacted provider profile suitable for sharing.
/// Secrets are stripped — only references (keychain paths, env var names) remain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedProviderProfile {
    pub id: String,
    pub kind: ProviderKind,
    pub display_name: String,
    pub base_url: String,
    pub auth: ExportedAuthConfig,
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
}

/// Redacted auth config — never contains raw secret values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ExportedAuthConfig {
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
        #[serde(rename = "secretRef", skip_serializing_if = "Option::is_none")]
        secret_ref: Option<String>,
    },
}

impl From<&AuthConfig> for ExportedAuthConfig {
    fn from(auth: &AuthConfig) -> Self {
        match auth {
            AuthConfig::None => ExportedAuthConfig::None,
            AuthConfig::ApiKey {
                source,
                secret_ref,
                env_var,
            } => ExportedAuthConfig::ApiKey {
                source: source.clone(),
                secret_ref: secret_ref.clone(),
                env_var: env_var.clone(),
            },
            AuthConfig::BearerToken { source, secret_ref } => ExportedAuthConfig::BearerToken {
                source: source.clone(),
                secret_ref: Some(secret_ref.clone()),
            },
        }
    }
}

impl From<&ProviderProfile> for ExportedProviderProfile {
    fn from(profile: &ProviderProfile) -> Self {
        Self {
            id: profile.id.clone(),
            kind: profile.kind.clone(),
            display_name: profile.display_name.clone(),
            base_url: profile.base_url.clone(),
            auth: ExportedAuthConfig::from(&profile.auth),
            headers: profile.headers.clone(),
            defaults: profile.defaults.clone(),
            capabilities: profile.capabilities.clone(),
            policy: profile.policy.clone(),
            discovery: profile.discovery.clone(),
            discovered_models: profile
                .discovered_models
                .iter()
                .filter(|m| m.source != DiscoveredModelSource::WarpHosted)
                .cloned()
                .collect(),
        }
    }
}

/// Collection of exported provider profiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderExportTemplate {
    pub version: u32,
    pub providers: Vec<ExportedProviderProfile>,
}

impl ProviderExportTemplate {
    pub fn new(providers: Vec<ExportedProviderProfile>) -> Self {
        Self {
            version: 1,
            providers,
        }
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Result of importing a provider profile, indicating whether the user
/// needs to provide a secret before the provider can be used.
#[derive(Debug)]
pub struct ImportResult {
    pub profile: ProviderProfile,
    pub needs_secret: bool,
    pub secret_hint: Option<String>,
}

/// Validates and imports a provider profile from an exported template.
/// Returns an `ImportResult` indicating whether a secret needs to be provided.
pub fn import_provider(
    exported: &ExportedProviderProfile,
    registry: &ProviderRegistry,
) -> Result<ImportResult, ImportError> {
    // Check for ID collision
    if registry.providers.contains_key(&exported.id) {
        return Err(ImportError::IdCollision(exported.id.clone()));
    }

    let (needs_secret, secret_hint) = match &exported.auth {
        ExportedAuthConfig::None => (false, None),
        ExportedAuthConfig::ApiKey {
            source: AuthSource::Env,
            env_var,
            ..
        } => {
            // Check if the env var is already set
            let env_var = env_var.clone().unwrap_or_default();
            let needs = std::env::var(&env_var).is_err();
            let hint = if needs {
                Some(format!("Set the `{env_var}` environment variable"))
            } else {
                None
            };
            (needs, hint)
        }
        ExportedAuthConfig::ApiKey {
            source: AuthSource::Keychain,
            ..
        } => (
            true,
            Some("Enter the API key to store in your keychain".to_string()),
        ),
        ExportedAuthConfig::BearerToken { .. } => (
            true,
            Some("Enter the bearer token to store in your keychain".to_string()),
        ),
    };

    let auth = match &exported.auth {
        ExportedAuthConfig::None => AuthConfig::None,
        ExportedAuthConfig::ApiKey {
            source,
            secret_ref,
            env_var,
        } => AuthConfig::ApiKey {
            source: source.clone(),
            secret_ref: secret_ref.clone(),
            env_var: env_var.clone(),
        },
        ExportedAuthConfig::BearerToken { source, secret_ref } => AuthConfig::BearerToken {
            source: source.clone(),
            secret_ref: secret_ref.clone().unwrap_or_default(),
        },
    };

    let mut profile = ProviderProfile::direct(
        &exported.id,
        exported.kind.clone(),
        &exported.display_name,
        Some(exported.base_url.clone()),
        auth,
    );
    profile.headers = exported.headers.clone();
    profile.defaults = exported.defaults.clone();
    profile.capabilities = exported.capabilities.clone();
    profile.policy = exported.policy.clone();
    profile.discovery = exported.discovery.clone();
    profile.discovered_models = exported.discovered_models.clone();

    Ok(ImportResult {
        profile,
        needs_secret,
        secret_hint,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("Provider ID '{0}' already exists in the registry")]
    IdCollision(String),
}

#[cfg(test)]
#[path = "provider_import_tests.rs"]
mod tests;
