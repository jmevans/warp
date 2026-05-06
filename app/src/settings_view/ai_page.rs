#[cfg(not(target_family = "wasm"))]
use crate::ai::aws_credentials::refresh_aws_credentials;
use crate::ai::blocklist::agent_view::agent_input_footer::editor::{
    AgentToolbarEditorMode, AgentToolbarInlineEditor,
};
use crate::ai::blocklist::BlocklistAIPermissions;
use crate::ai::execution_profiles::model_menu_items::available_model_menu_items;
use crate::ai::execution_profiles::profiles::{
    AIExecutionProfilesModel, AIExecutionProfilesModelEvent, ClientProfileId,
};
use crate::ai::execution_profiles::{AIExecutionProfile, ActionPermission, WriteToPtyPermission};
use crate::ai::llms::{LLMContextWindow, LLMId, LLMPreferences, LLMPreferencesEvent};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::ai::paths::host_native_absolute_path;
use crate::auth::auth_manager::{AuthManager, LoginGatedFeature};
use crate::auth::auth_view_modal::AuthViewVariant;
use crate::auth::AuthStateProvider;
use crate::cloud_object::model::persistence::{CloudModel, CloudModelEvent};
use crate::cloud_object::GenericStringObjectFormat::Json;
use crate::cloud_object::JsonObjectType;
use crate::cloud_object::ObjectType;

use crate::editor::{EditorOptions, InteractionState, SingleLineEditorOptions, TextColors};
use crate::settings::InputSettings;
use crate::settings::{
    AIAutoDetectionEnabled, AICommandDenylist, AISettingsChangedEvent,
    AgentModeCodingPermissionsType, AgentModeCommandExecutionDenylist,
    AgentModeCommandExecutionPredicate, AgentModeQuerySuggestionsEnabled, AwsBedrockAutoLogin,
    AwsBedrockCredentialsEnabled, CodeSettings, CodebaseContextEnabled, FileBasedMcpEnabled,
    GitOperationsAutogenEnabled, IncludeAgentCommandsInHistory, IntelligentAutosuggestionsEnabled,
    MemoryEnabled, NLDInTerminalEnabled, NaturalLanguageAutosuggestionsEnabled,
    OrchestrationEnabled, RuleSuggestionsEnabled, SharedBlockTitleGenerationEnabled,
    ShouldRenderCLIAgentToolbar, ShouldRenderUseAgentToolbarForUserCommands,
    ShouldShowOzUpdatesInZeroState, ShowAgentTips, ShowConversationHistory, ShowHintText,
    ThinkingDisplayMode, VoiceInputEnabled, WarpDriveContextEnabled,
};
use crate::terminal::session_settings::{SessionSettings, SessionSettingsChangedEvent};
use crate::terminal::CLIAgent;
use crate::view_components::{
    action_button::{ActionButton, ButtonSize, SecondaryTheme},
    FilterableDropdown, SubmittableTextInput, SubmittableTextInputEvent,
};
use crate::workspaces::user_workspaces::UserWorkspacesEvent;
use ::ai::api_keys::{ApiKeyManager, ApiKeys};
use ::ai::provider_import::{
    import_provider, ExportedProviderProfile, ImportError, ProviderExportTemplate,
};
use ::ai::provider_registry::{
    AuthConfig, AuthSource, CapabilitySetting, DiscoveredModel, DiscoveryStrategy,
    ProviderCapabilities, ProviderDefaults, ProviderDiscovery, ProviderKind, ProviderPolicy,
    ProviderProfile, ProviderRegistry, ProviderRegistryDefaults, LEGACY_ANTHROPIC_PROVIDER_ID,
    LEGACY_OPENAI_PROVIDER_ID, WARP_HOSTED_PROVIDER_ID,
};
use ::ai::provider_validator::{
    ProviderValidator, ValidationError, ValidationLevel, ValidationResult, ValidationStatus,
};
use enum_iterator::all;
use itertools::Itertools;
use regex::Regex;
use settings::{Setting, ToggleableSetting};
use strum::IntoEnumIterator;
use warp_core::channel::ChannelState;
use warp_core::context_flag::ContextFlag;
use warp_core::features::FeatureFlag;
use warp_core::ui::theme::color::internal_colors;
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    Border, ChildView, ConstrainedBox, CornerRadius, CrossAxisAlignment, Dismiss, Expanded, Fill,
    HyperlinkLens, MainAxisAlignment, MainAxisSize, MouseStateHandle, Radius, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::id;
use warpui::keymap::ContextPredicate;
use warpui::ui_components::slider::SliderStateHandle;
use warpui::{
    elements::{
        Container, Flex, FormattedTextElement, HighlightedHyperlink, HyperlinkUrl, ParentElement,
    },
    ui_components::{
        button::ButtonVariant,
        components::{Coords, UiComponent, UiComponentStyles},
        switch::{SwitchStateHandle, TooltipConfig},
    },
    Action, AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};
use warpui_extras::secure_storage::AppContextExt as _;

use super::execution_profile_view::{ExecutionProfileView, ExecutionProfileViewEvent};
use super::settings_page::{render_custom_size_header, render_settings_info_banner};
use super::{
    flags,
    settings_page::{
        build_sub_header, build_toggle_element, render_body_item_label,
        render_body_item_label_with_icon, render_dropdown_item, render_dropdown_item_label,
        render_full_pane_width_ai_button, render_input_list, render_separator, InputListItem,
        LocalOnlyIconState, MatchData, PageType, SettingsPageMeta, SettingsPageViewHandle,
        SettingsWidget, ToggleState, HEADER_PADDING, TOGGLE_BUTTON_RIGHT_PADDING,
    },
    SettingActionPairContexts, SettingActionPairDescriptions, SettingsAction, SettingsSection,
    ToggleSettingActionPair,
};

/// Identifies which subpage of the AI settings the user is viewing.
/// When `None`, the page shows all widgets (legacy/full view).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AISubpage {
    /// The main "WarpAgent" page: global AI toggle + Active AI + Input + Other sections.
    WarpAgent,
    /// Agent profiles and permissions.
    Profiles,
    /// Knowledge / Rules settings.
    Knowledge,
    /// Third-party CLI agent settings.
    ThirdPartyCLIAgents,
}

impl AISubpage {
    pub fn from_section(section: SettingsSection) -> Option<Self> {
        match section {
            SettingsSection::WarpAgent => Some(Self::WarpAgent),
            SettingsSection::AgentProfiles => Some(Self::Profiles),
            SettingsSection::Knowledge => Some(Self::Knowledge),
            SettingsSection::ThirdPartyCLIAgents => Some(Self::ThirdPartyCLIAgents),
            // AgentMCPServers renders the standalone MCPServers page, not an AI subpage.
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderDraftAuthMode {
    None,
    EnvApiKey,
    KeychainApiKey,
    KeychainBearerToken,
}

impl ProviderDraftAuthMode {
    fn display_name(&self) -> &'static str {
        match self {
            Self::None => "No auth",
            Self::EnvApiKey => "API key from env var",
            Self::KeychainApiKey => "API key from keychain",
            Self::KeychainBearerToken => "Bearer token from keychain",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderDraftField {
    Id,
    DisplayName,
    BaseUrl,
    Model,
    EnvVar,
    SecretKey,
    // Defaults (stored as strings, parsed on save)
    Temperature,
    TopP,
    MaxOutputTokens,
    TimeoutMs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderDraftCapability {
    Tools,
    Vision,
    StructuredOutputs,
    PromptCaching,
    ReasoningControls,
    ModelDiscovery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderDraftPolicy {
    AllowFallback,
    RedactTelemetry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderDraftDefaultField {
    Stream,
    Temperature,
    TopP,
    MaxOutputTokens,
    TimeoutMs,
}

/// Draft representation of a CapabilitySetting (auto/on/off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilitySettingDraft {
    Auto,
    On,
    Off,
}

impl CapabilitySettingDraft {
    fn display_name(&self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::On => "On",
            Self::Off => "Off",
        }
    }

    fn to_capability_setting(&self) -> CapabilitySetting {
        match self {
            Self::Auto => CapabilitySetting::automatic(),
            Self::On => CapabilitySetting::enabled(),
            Self::Off => CapabilitySetting::disabled(),
        }
    }

    fn from_capability_setting(cs: &CapabilitySetting) -> Self {
        match cs {
            CapabilitySetting::Automatic(_) => Self::Auto,
            CapabilitySetting::Explicit(true) => Self::On,
            CapabilitySetting::Explicit(false) => Self::Off,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderDraft {
    /// When Some, we are editing an existing provider with this id.
    /// When None, we are creating a new provider.
    editing_provider_id: Option<String>,
    id: String,
    display_name: String,
    kind: ProviderKind,
    auth_mode: ProviderDraftAuthMode,
    base_url: String,
    model: String,
    env_var: String,
    secret_key: String,
    // Capabilities
    cap_tools: CapabilitySettingDraft,
    cap_vision: CapabilitySettingDraft,
    cap_structured_outputs: CapabilitySettingDraft,
    cap_prompt_caching: CapabilitySettingDraft,
    cap_reasoning_controls: CapabilitySettingDraft,
    cap_model_discovery: CapabilitySettingDraft,
    // Policy
    allow_fallback: bool,
    redact_telemetry: bool,
    // Defaults
    stream: bool,
    temperature: String,
    top_p: String,
    max_output_tokens: String,
    timeout_ms: String,
}

impl Default for ProviderDraft {
    fn default() -> Self {
        Self {
            editing_provider_id: None,
            id: "ollama-local".to_string(),
            display_name: "Ollama local".to_string(),
            kind: ProviderKind::OpenAICompatible,
            auth_mode: ProviderDraftAuthMode::None,
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            model: String::new(),
            env_var: String::new(),
            secret_key: String::new(),
            cap_tools: CapabilitySettingDraft::Auto,
            cap_vision: CapabilitySettingDraft::Auto,
            cap_structured_outputs: CapabilitySettingDraft::Auto,
            cap_prompt_caching: CapabilitySettingDraft::Auto,
            cap_reasoning_controls: CapabilitySettingDraft::Auto,
            cap_model_discovery: CapabilitySettingDraft::Auto,
            allow_fallback: false,
            redact_telemetry: true,
            stream: true,
            temperature: String::new(),
            top_p: String::new(),
            max_output_tokens: String::new(),
            timeout_ms: String::new(),
        }
    }
}

impl ProviderDraft {
    fn from_profile(profile: &ProviderProfile) -> Self {
        let (auth_mode, env_var, _secret_key) = match &profile.auth {
            AuthConfig::None => (ProviderDraftAuthMode::None, String::new(), String::new()),
            AuthConfig::ApiKey {
                source: AuthSource::Env,
                env_var: Some(ev),
                ..
            } => (ProviderDraftAuthMode::EnvApiKey, ev.clone(), String::new()),
            AuthConfig::ApiKey {
                source: AuthSource::Env,
                ..
            } => (
                ProviderDraftAuthMode::EnvApiKey,
                String::new(),
                String::new(),
            ),
            AuthConfig::ApiKey {
                source: AuthSource::Keychain,
                ..
            } => (
                ProviderDraftAuthMode::KeychainApiKey,
                String::new(),
                String::new(),
            ),
            AuthConfig::BearerToken {
                source: AuthSource::Keychain,
                ..
            } => (
                ProviderDraftAuthMode::KeychainBearerToken,
                String::new(),
                String::new(),
            ),
            AuthConfig::BearerToken { .. } => {
                (ProviderDraftAuthMode::None, String::new(), String::new())
            }
        };
        Self {
            editing_provider_id: Some(profile.id.clone()),
            id: profile.id.clone(),
            display_name: profile.display_name.clone(),
            kind: profile.kind.clone(),
            auth_mode,
            base_url: profile.base_url.clone(),
            model: profile
                .defaults
                .model
                .clone()
                .or_else(|| profile.discovery.fallback_models.first().cloned())
                .unwrap_or_default(),
            env_var,
            secret_key: String::new(),
            cap_tools: CapabilitySettingDraft::from_capability_setting(&profile.capabilities.tools),
            cap_vision: CapabilitySettingDraft::from_capability_setting(
                &profile.capabilities.vision,
            ),
            cap_structured_outputs: CapabilitySettingDraft::from_capability_setting(
                &profile.capabilities.structured_outputs,
            ),
            cap_prompt_caching: CapabilitySettingDraft::from_capability_setting(
                &profile.capabilities.prompt_caching,
            ),
            cap_reasoning_controls: CapabilitySettingDraft::from_capability_setting(
                &profile.capabilities.reasoning_controls,
            ),
            cap_model_discovery: CapabilitySettingDraft::from_capability_setting(
                &profile.capabilities.model_discovery,
            ),
            allow_fallback: profile.policy.allow_fallback_to_warp_hosted,
            redact_telemetry: profile.policy.redact_telemetry,
            stream: profile.defaults.stream.unwrap_or(true),
            temperature: profile
                .defaults
                .temperature
                .map(|v| v.to_string())
                .unwrap_or_default(),
            top_p: profile
                .defaults
                .top_p
                .map(|v| v.to_string())
                .unwrap_or_default(),
            max_output_tokens: profile
                .defaults
                .max_output_tokens
                .map(|v| v.to_string())
                .unwrap_or_default(),
            timeout_ms: profile
                .defaults
                .timeout_ms
                .map(|v| v.to_string())
                .unwrap_or_default(),
        }
    }

    fn is_editing(&self) -> bool {
        self.editing_provider_id.is_some()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            tools: self.cap_tools.to_capability_setting(),
            vision: self.cap_vision.to_capability_setting(),
            structured_outputs: self.cap_structured_outputs.to_capability_setting(),
            prompt_caching: self.cap_prompt_caching.to_capability_setting(),
            reasoning_controls: self.cap_reasoning_controls.to_capability_setting(),
            model_discovery: self.cap_model_discovery.to_capability_setting(),
        }
    }

    fn policy(&self) -> ProviderPolicy {
        let mut policy = ProviderPolicy::for_kind(&self.kind);
        policy.allow_fallback_to_warp_hosted = self.allow_fallback;
        policy.redact_telemetry = self.redact_telemetry;
        policy
    }

    fn defaults(&self) -> ProviderDefaults {
        ProviderDefaults {
            model: if self.model.trim().is_empty() {
                None
            } else {
                Some(self.model.trim().to_string())
            },
            stream: Some(self.stream),
            temperature: self.temperature.parse().ok(),
            top_p: self.top_p.parse().ok(),
            max_output_tokens: self.max_output_tokens.parse().ok(),
            timeout_ms: self.timeout_ms.parse().ok(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ProviderValidationUiState {
    Validating,
    Discovering,
    ValidationComplete(ValidationResult),
    DiscoveryComplete(Vec<DiscoveredModel>),
    DiscoveryFailed(ValidationError),
}

impl ProviderValidationUiState {
    fn title(&self) -> &'static str {
        match self {
            Self::Validating => "Validating",
            Self::Discovering => "Discovering models",
            Self::ValidationComplete(result) => match result.status {
                ValidationStatus::Success => "Validation succeeded",
                ValidationStatus::Partial => "Validation partially succeeded",
                ValidationStatus::Failed => "Validation failed",
            },
            Self::DiscoveryComplete(_) => "Model discovery succeeded",
            Self::DiscoveryFailed(_) => "Model discovery failed",
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::Validating => {
                "Checking reachability, credentials, and response shape.".to_string()
            }
            Self::Discovering => "Requesting model metadata from the provider.".to_string(),
            Self::ValidationComplete(result) => {
                let mut detail = match result.status {
                    ValidationStatus::Success => {
                        "Provider is reachable and compatible.".to_string()
                    }
                    ValidationStatus::Partial => {
                        "Provider validation was inconclusive.".to_string()
                    }
                    ValidationStatus::Failed => "Provider validation failed.".to_string(),
                };
                if let Some(models) = &result.discovered_models {
                    detail.push_str(&format!(" {} models discovered.", models.len()));
                }
                if let Some(error) = &result.error {
                    detail.push_str(&format!(" {error}"));
                }
                detail
            }
            Self::DiscoveryComplete(models) => format!("{} models discovered.", models.len()),
            Self::DiscoveryFailed(error) => format!("{error}"),
        }
    }

    fn is_success(&self) -> bool {
        matches!(
            self,
            Self::ValidationComplete(ValidationResult {
                status: ValidationStatus::Success,
                ..
            }) | Self::DiscoveryComplete(_)
        )
    }
}
use crate::ai::{AIRequestUsageModel, AIRequestUsageModelEvent};
use crate::menu::{MenuItem, MenuItemFields};
use crate::server::telemetry::{
    AgentModeAutoDetectionSettingOrigin, AutonomySettingToggleSource,
    ToggleCodeSuggestionsSettingSource,
};
use crate::ui_components::icons::Icon;
use crate::view_components::dropdown::DropdownAction;
use crate::workspaces::workspace::{AdminEnablementSetting, CustomerType};
use crate::{
    appearance::Appearance,
    editor::Event as EditorEvent,
    editor::{EditorView, TextOptions},
    settings::{AISettings, VoiceInputToggleKey},
    ui_components::blended_colors,
    util::bindings,
    view_components::{Dropdown, DropdownItem},
};
use crate::{report_error, report_if_error, send_telemetry_from_ctx};
use crate::{TelemetryEvent, UserWorkspaces};
use markdown_parser::{FormattedText, FormattedTextFragment, FormattedTextLine};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const CONTENT_FONT_SIZE: f32 = 12.;
const PRIMARY_HEADER_FONT_SIZE: f32 = 24.;

const AI_SETTINGS_DROPDOWN_WIDTH: f32 = 250.;
const AI_SETTINGS_DROPDOWN_MAX_HEIGHT: f32 = 250.;
const CONTEXT_WINDOW_SLIDER_WIDTH: f32 = 220.;
const CONTEXT_WINDOW_INPUT_BOX_WIDTH: f32 = 120.;

const NEXT_COMMAND_DESCRIPTION: &str = "Let AI suggest the next command to run based on your command history, outputs, and common workflows.";
const PROMPT_SUGGESTIONS_DESCRIPTION: &str = "Let AI suggest natural language prompts, as inline banners in the input, based on recent commands and their outputs.";
const SUGGESTED_CODE_BANNERS_DESCRIPTION: &str = "Let AI suggest code diffs and queries as inline banners in the blocklist, based on recent commands and their outputs.";
const NATURAL_LANGUAGE_AUTOSUGGESTIONS: &str =
    "Let AI suggest natural language autosuggestions, based on recent commands and their outputs.";
const SHARED_BLOCK_TITLE_GENERATION_DESCRIPTION: &str =
    "Let AI generate a title for your shared block based on the command and output.";
const GIT_OPERATIONS_AUTOGEN_DESCRIPTION: &str =
    "Let AI generate commit messages and pull request titles and descriptions.";
const WISPR_FLOW_URL: &str = "https://wisprflow.ai/";

pub fn init_actions_from_parent_view<T: Action + Clone>(
    app: &mut AppContext,
    context: &ContextPredicate,
    builder: fn(SettingsAction) -> T,
) {
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "AI",
            builder(SettingsAction::AI(AISettingsPageAction::ToggleGlobalAI)),
            context,
            flags::IS_ANY_AI_ENABLED,
        )
        .with_group(bindings::BindingGroup::WarpAi)],
        app,
    );

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Active AI",
            builder(SettingsAction::AI(AISettingsPageAction::ToggleActiveAI)),
            &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
            flags::IS_ACTIVE_AI_ENABLED,
        )
        .with_group(bindings::BindingGroup::WarpAi)],
        app,
    );

    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            if FeatureFlag::AgentView.is_enabled() {
                "terminal command autodetection in agent input"
            } else {
                "natural language detection"
            },
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleAIInputAutoDetection,
            )),
            &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
            flags::AI_INPUT_AUTODETECTION_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| FeatureFlag::AgentMode.is_enabled())],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "agent prompt autodetection in terminal input",
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleNLDInTerminal,
            )),
            &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
            flags::NLD_IN_TERMINAL_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| FeatureFlag::AgentView.is_enabled())],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "Next Command",
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleIntelligentAutosuggestions,
            )),
            &(context.clone() & id!(flags::IS_ACTIVE_AI_ENABLED)),
            flags::INTELLIGENT_AUTOSUGGESTIONS_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "prompt suggestions",
            builder(SettingsAction::AI(
                AISettingsPageAction::TogglePromptSuggestions,
            )),
            &(context.clone() & id!(flags::IS_ACTIVE_AI_ENABLED)),
            flags::PROMPT_SUGGESTIONS_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "code suggestions",
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleCodeSuggestions,
            )),
            &(context.clone()
                & id!(flags::IS_ACTIVE_AI_ENABLED)
                & id!(flags::PROMPT_SUGGESTIONS_FLAG)),
            flags::CODE_SUGGESTIONS_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::custom(
            SettingActionPairDescriptions::new("Show agent tips", "Hide agent tips"),
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleShowAgentTips,
            )),
            SettingActionPairContexts::new(
                context.clone() & id!(flags::IS_ANY_AI_ENABLED) & !id!(flags::SHOW_AGENT_TIPS_FLAG),
                context.clone() & id!(flags::IS_ANY_AI_ENABLED) & id!(flags::SHOW_AGENT_TIPS_FLAG),
            ),
            None,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| FeatureFlag::AgentTips.is_enabled())],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::custom(
            SettingActionPairDescriptions::new(
                "Show Oz changelog in new agent conversation view",
                "Hide Oz changelog in new agent conversation view",
            ),
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleShowOzUpdatesInZeroState,
            )),
            SettingActionPairContexts::new(
                context.clone()
                    & id!(flags::IS_ANY_AI_ENABLED)
                    & !id!(flags::SHOW_OZ_UPDATES_IN_ZERO_STATE_FLAG),
                context.clone()
                    & id!(flags::IS_ANY_AI_ENABLED)
                    & id!(flags::SHOW_OZ_UPDATES_IN_ZERO_STATE_FLAG),
            ),
            None,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| FeatureFlag::AgentView.is_enabled())],
        app,
    );
    {
        use crate::settings::ThinkingDisplayMode;
        use warpui::keymap::FixedBinding;

        let ai_context = context.clone() & id!(flags::IS_ANY_AI_ENABLED);
        let mode_bindings: Vec<FixedBinding> = ThinkingDisplayMode::iter()
            .map(|mode| {
                let context_flag = match mode {
                    ThinkingDisplayMode::ShowAndCollapse => {
                        flags::THINKING_DISPLAY_SHOW_AND_COLLAPSE
                    }
                    ThinkingDisplayMode::AlwaysShow => flags::THINKING_DISPLAY_ALWAYS_SHOW,
                    ThinkingDisplayMode::NeverShow => flags::THINKING_DISPLAY_NEVER_SHOW,
                };
                FixedBinding::empty(
                    mode.command_palette_description(),
                    builder(SettingsAction::AI(
                        AISettingsPageAction::SetThinkingDisplayMode(mode),
                    )),
                    ai_context.clone() & !id!(context_flag),
                )
                .with_group(bindings::BindingGroup::WarpAi.as_str())
            })
            .collect();
        app.register_fixed_bindings(mode_bindings);
    }
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "natural language autosuggestions",
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleNaturalLanguageAutosuggestions,
            )),
            &(context.clone() & id!(flags::IS_ACTIVE_AI_ENABLED)),
            flags::NATURAL_LANGUAGE_AUTOSUGGESTIONS_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| FeatureFlag::PredictAMQueries.is_enabled())],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "shared block title generation",
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleSharedTitleGeneration,
            )),
            &(context.clone() & id!(flags::IS_ACTIVE_AI_ENABLED)),
            flags::SHARED_BLOCK_TITLE_GENERATION_FLAG,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| FeatureFlag::SharedBlockTitleGeneration.is_enabled())],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::new(
            "voice input",
            builder(SettingsAction::AI(AISettingsPageAction::ToggleVoiceInput)),
            &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
            flags::IS_VOICE_INPUT_ENABLED,
        )
        .with_group(bindings::BindingGroup::WarpAi)
        .with_enabled(|| cfg!(feature = "voice_input"))],
        app,
    );
    ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
        vec![ToggleSettingActionPair::custom(
            SettingActionPairDescriptions::new(
                "Show \"Use Agent\" footer",
                "Hide \"Use Agent\" footer",
            ),
            builder(SettingsAction::AI(
                AISettingsPageAction::ToggleUseAgentToolbar,
            )),
            SettingActionPairContexts::new(
                context.clone()
                    & id!(flags::IS_ANY_AI_ENABLED)
                    & !id!(flags::USE_AGENT_FOOTER_FLAG),
                context.clone() & id!(flags::IS_ANY_AI_ENABLED) & id!(flags::USE_AGENT_FOOTER_FLAG),
            ),
            None,
        )
        .with_group(bindings::BindingGroup::WarpAi)],
        app,
    );
    if !FeatureFlag::FullSourceCodeEmbedding.is_enabled() {
        ToggleSettingActionPair::add_toggle_setting_action_pairs_as_bindings(
            vec![ToggleSettingActionPair::new(
                "codebase index",
                builder(SettingsAction::AI(
                    AISettingsPageAction::ToggleCodebaseContext,
                )),
                &(context.clone() & id!(flags::IS_ANY_AI_ENABLED)),
                flags::IS_CODEBASE_INDEXING_ENABLED,
            )],
            app,
        );
    }
}

pub struct AISettingsPageView {
    page: PageType<Self>,
    active_subpage: Option<AISubpage>,
    voice_input_toggle_key_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    local_only_icon_tooltip_states: RefCell<HashMap<String, MouseStateHandle>>,
    autodetection_denylist_editor: ViewHandle<EditorView>,
    autonomy_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,

    code_read_autonomy_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,

    code_read_allowlist_editor: ViewHandle<SubmittableTextInput>,
    code_read_allowlist_mouse_state_handles: Vec<MouseStateHandle>,

    command_execution_allowlist_editor: ViewHandle<SubmittableTextInput>,
    command_execution_allowlist_mouse_state_handles: Vec<MouseStateHandle>,
    command_execution_denylist_editor: ViewHandle<SubmittableTextInput>,
    command_execution_denylist_mouse_state_handles: Vec<MouseStateHandle>,
    cli_agent_footer_command_editor: ViewHandle<SubmittableTextInput>,
    cli_agent_footer_command_mouse_state_handles: Vec<MouseStateHandle>,
    cli_agent_footer_command_agent_dropdowns: Vec<ViewHandle<Dropdown<AISettingsPageAction>>>,
    agent_toolbar_inline_editor: ViewHandle<AgentToolbarInlineEditor>,
    cli_agent_toolbar_inline_editor: ViewHandle<AgentToolbarInlineEditor>,

    apply_code_diffs_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,
    read_files_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,
    execute_commands_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,
    write_to_pty_autonomy_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,
    mcp_permissions_dropdown_menu: ViewHandle<Dropdown<AISettingsPageAction>>,

    // Allowlisting directories (default profile)
    directory_allowlist_mouse_state_handles: Vec<MouseStateHandle>,
    directory_allowlist_editor: ViewHandle<SubmittableTextInput>,

    // Allowlisting commands (default profile)
    command_allowlist_mouse_state_handles: Vec<MouseStateHandle>,
    command_allowlist_editor: ViewHandle<SubmittableTextInput>,

    // Denylisting commands (default profile)
    command_denylist_mouse_state_handles: Vec<MouseStateHandle>,
    command_denylist_tooltip_mouse_state_handles: Vec<MouseStateHandle>,
    command_denylist_editor: ViewHandle<SubmittableTextInput>,

    mcp_allowlist_mouse_state_handles: Vec<MouseStateHandle>,
    mcp_allowlist_dropdown: ViewHandle<FilterableDropdown<AISettingsPageAction>>,

    mcp_denylist_mouse_state_handles: Vec<MouseStateHandle>,
    mcp_denylist_dropdown: ViewHandle<FilterableDropdown<AISettingsPageAction>>,

    base_model_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    coding_model_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,

    context_window_slider_state: SliderStateHandle,
    context_window_editor: ViewHandle<EditorView>,
    last_synced_context_window_editor_value: Option<u32>,

    thinking_display_mode_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    #[cfg(feature = "local_fs")]
    conversation_layout_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,

    // Profile views
    profile_views: Vec<ViewHandle<ExecutionProfileView>>,
    add_profile_button: ViewHandle<ActionButton>,

    provider_draft: ProviderDraft,
    provider_validation_states: HashMap<String, ProviderValidationUiState>,
    // Provider form editors — stored on view so save handler reads live buffer text
    provider_id_editor: ViewHandle<EditorView>,
    provider_display_name_editor: ViewHandle<EditorView>,
    provider_base_url_editor: ViewHandle<EditorView>,
    provider_model_editor: ViewHandle<EditorView>,
    provider_env_var_editor: ViewHandle<EditorView>,
    provider_secret_key_editor: ViewHandle<EditorView>,
    provider_defaults_temperature_editor: ViewHandle<EditorView>,
    provider_defaults_top_p_editor: ViewHandle<EditorView>,
    provider_defaults_max_output_tokens_editor: ViewHandle<EditorView>,
    provider_defaults_timeout_ms_editor: ViewHandle<EditorView>,

    import_dialog_visible: bool,
    import_json_text: String,
    import_pending_template: Option<ProviderExportTemplate>,
    import_collision: Option<(String, ImportCollisionAction)>,
    import_secret_prompt: Option<ImportSecretPromptState>,
    import_secret_value: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct ImportSecretPromptState {
    provider_id: String,
    secret_hint: String,
}

impl AISettingsPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let is_any_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);

        let workspace = UserWorkspaces::handle(ctx);
        let ai_autonomy_settings = workspace.as_ref(ctx).ai_autonomy_settings();
        ctx.subscribe_to_model(&workspace, |me, workspace, event, ctx| {
            if let UserWorkspacesEvent::TeamsChanged = event {
                me.refresh_all_execution_profile_ui(ctx);
                me.reset_execution_profile_mouse_state_handles(ctx);

                let is_any_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);
                let ai_autonomy_settings = workspace.as_ref(ctx).ai_autonomy_settings();

                Self::update_editor_interaction_state(
                    me.command_denylist_editor.as_ref(ctx).editor().clone(),
                    is_any_ai_enabled,
                    ctx,
                );

                Self::update_editor_interaction_state(
                    me.command_allowlist_editor.as_ref(ctx).editor().clone(),
                    is_any_ai_enabled
                        && !ai_autonomy_settings.has_override_for_execute_commands_allowlist(),
                    ctx,
                );

                Self::update_editor_interaction_state(
                    me.directory_allowlist_editor.as_ref(ctx).editor().clone(),
                    is_any_ai_enabled
                        && !ai_autonomy_settings.has_override_for_read_files_allowlist(),
                    ctx,
                );

                ctx.notify();
            }
        });

        let voice_input_toggle_key_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            if !AISettings::as_ref(ctx).is_voice_input_enabled(ctx) {
                dropdown.set_disabled(ctx);
            }

            let values = VoiceInputToggleKey::all_possible_values();
            let current_value = AISettings::as_ref(ctx).voice_input_toggle_key.value();
            let selected_index = values
                .iter()
                .position(|val| val == current_value)
                .unwrap_or_else(|| {
                    log::warn!(
                        "Could not find current VoiceInputToggleKey value in dropdown option list"
                    );
                    0
                });

            dropdown.add_items(
                values
                    .into_iter()
                    .map(|val| {
                        DropdownItem::new(
                            val.display_name(),
                            AISettingsPageAction::SetVoiceInputToggleKey(val),
                        )
                    })
                    .collect(),
                ctx,
            );
            dropdown.set_selected_by_index(selected_index, ctx);

            dropdown
        });

        let coding_model_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_max_height(AI_SETTINGS_DROPDOWN_MAX_HEIGHT, ctx);
            dropdown
        });
        Self::refresh_coding_model_menu(&coding_model_dropdown, ctx);

        let base_model_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_max_height(AI_SETTINGS_DROPDOWN_MAX_HEIGHT, ctx);

            dropdown
        });
        Self::refresh_base_model_menu(&base_model_dropdown, ctx);

        let initial_context_window_value = Self::initial_context_window_value(ctx);
        let clamped_initial = Self::configurable_context_window(ctx)
            .map(|cw| initial_context_window_value.clamp(cw.min, cw.max))
            .unwrap_or(initial_context_window_value);
        let context_window_slider_state = SliderStateHandle::default();

        let context_window_editor = ctx.add_typed_action_view(|ctx| {
            let options = SingleLineEditorOptions {
                text: TextOptions {
                    font_size_override: Some(Appearance::as_ref(ctx).ui_font_size()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_buffer_text(&clamped_initial.to_string(), ctx);
            editor
        });
        ctx.subscribe_to_view(&context_window_editor, |me, _, event, ctx| {
            me.handle_context_window_editor_event(event, ctx);
        });
        let last_synced_context_window_editor_value = Some(clamped_initial);

        let thinking_display_mode_dropdown =
            OtherAIWidget::create_thinking_display_mode_dropdown(ctx);
        // Set initial selection based on current setting value.
        {
            let current_mode = AISettings::as_ref(ctx).thinking_display_mode;
            thinking_display_mode_dropdown.update(ctx, |dropdown, ctx| {
                dropdown.set_selected_by_action(
                    AISettingsPageAction::SetThinkingDisplayMode(current_mode),
                    ctx,
                );
            });
        }

        let autonomy_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown
        });
        Self::refresh_autonomy_dropdown_menu(&autonomy_dropdown_menu, ctx);

        let code_read_autonomy_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown
        });
        Self::refresh_code_read_autonomy_dropdown_menu(&code_read_autonomy_dropdown_menu, ctx);

        // While the data model supports arbitrary files in the allowlist,
        // it's most intuitive to allowlist whole directories.
        let code_read_allowlist_editor = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx).validate_on_submit(|s| {
                let expanded = host_native_absolute_path(s, &None, &None);
                Path::new(&expanded).is_dir()
            });
            input.set_placeholder_text("e.g. ~/code-repos/repo", ctx);
            input
        });
        Self::update_editor_interaction_state(
            code_read_allowlist_editor.as_ref(ctx).editor().clone(),
            is_any_ai_enabled,
            ctx,
        );

        ctx.subscribe_to_view(&code_read_allowlist_editor, |_, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(s) = event {
                let expanded = host_native_absolute_path(s, &None, &None);
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    report_if_error!(
                        model.add_filepath_to_code_read_allowlist(PathBuf::from(expanded), ctx)
                    );
                });
            }
        });

        let autodetection_denylist_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = EditorOptions {
                autogrow: true,
                soft_wrap: true,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);

            editor.set_placeholder_text("Commands, comma separated", ctx);

            let current_value = AISettings::as_ref(ctx)
                .autodetection_command_denylist
                .value()
                .clone();
            editor.set_buffer_text(current_value.as_str(), ctx);
            editor
        });
        Self::update_editor_interaction_state(
            autodetection_denylist_editor.clone(),
            is_any_ai_enabled,
            ctx,
        );

        ctx.subscribe_to_view(&autodetection_denylist_editor, move |me, _, event, ctx| {
            me.handle_detection_denylist_editor_event(event, ctx);
        });

        let command_execution_allowlist_editor = ctx.add_typed_action_view(|ctx| {
            let mut input =
                SubmittableTextInput::new(ctx).validate_on_edit(|s| Regex::new(s).is_ok());
            input.set_placeholder_text("e.g. ls .*", ctx);
            input
        });
        Self::update_editor_interaction_state(
            command_execution_allowlist_editor
                .as_ref(ctx)
                .editor()
                .clone(),
            is_any_ai_enabled,
            ctx,
        );

        ctx.subscribe_to_view(&command_execution_allowlist_editor, |_, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(s) = event {
                let predicate = match AgentModeCommandExecutionPredicate::new_regex(s) {
                    Ok(regex) => regex,
                    Err(e) => {
                        log::warn!(
                            "Failed to convert string to regex for cmd execution allowlist: {e}"
                        );
                        return;
                    }
                };
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    report_if_error!(model.add_command_to_autoexecution_allowlist(predicate, ctx));
                })
            }
        });

        let command_execution_denylist_editor = ctx.add_typed_action_view(|ctx| {
            let mut input =
                SubmittableTextInput::new(ctx).validate_on_edit(|s| Regex::new(s).is_ok());
            input.set_placeholder_text("e.g. rm .*", ctx);
            input
        });
        Self::update_editor_interaction_state(
            command_execution_denylist_editor
                .as_ref(ctx)
                .editor()
                .clone(),
            is_any_ai_enabled,
            ctx,
        );

        ctx.subscribe_to_view(&command_execution_denylist_editor, |_, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(s) = event {
                let predicate = match AgentModeCommandExecutionPredicate::new_regex(s) {
                    Ok(regex) => regex,
                    Err(e) => {
                        log::warn!(
                            "Failed to convert string to regex for cmd execution denylist: {e}"
                        );
                        return;
                    }
                };
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    report_if_error!(model.add_command_to_autoexecution_denylist(predicate, ctx));
                })
            }
        });

        let cli_agent_footer_command_editor = ctx.add_typed_action_view(|ctx| {
            let mut input =
                SubmittableTextInput::new(ctx).validate_on_edit(|s| Regex::new(s).is_ok());
            input.set_placeholder_text("command (supports regex)", ctx);
            input
        });
        // The coding agent footer command editor is always enabled,
        // independent of the global AI toggle, because it controls
        // third-party coding agents rather than Warp's own AI.
        Self::update_editor_interaction_state(
            cli_agent_footer_command_editor.as_ref(ctx).editor().clone(),
            true,
            ctx,
        );
        ctx.subscribe_to_view(
            &cli_agent_footer_command_editor,
            |_, _, event, ctx| match event {
                SubmittableTextInputEvent::Submit(command) => {
                    AISettings::handle(ctx).update(ctx, |settings, ctx| {
                        settings.add_cli_agent_footer_enabled_command(command, ctx);
                    });
                }
                SubmittableTextInputEvent::Escape => ctx.emit(AISettingsPageEvent::FocusModal),
            },
        );

        let request_usage_model = AIRequestUsageModel::handle(ctx);
        ctx.subscribe_to_model(&request_usage_model, |_, _, _, ctx| {
            // The only event is RequestUsageUpdated
            ctx.notify();
        });

        ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), |me, _handle, _event, ctx| {
            // Re-render if teams-related data changed that may affect whether features such as voice input are enabled.
            Self::refresh_base_model_menu(&me.base_model_dropdown, ctx);
            Self::refresh_coding_model_menu(&me.coding_model_dropdown, ctx);
            ctx.notify();
        });

        ctx.subscribe_to_model(
            &AIExecutionProfilesModel::handle(ctx),
            |me, _, event, ctx| {
                match event {
                    AIExecutionProfilesModelEvent::ProfileCreated
                    | AIExecutionProfilesModelEvent::ProfileDeleted => {
                        me.refresh_profile_views(ctx);
                    }
                    AIExecutionProfilesModelEvent::ProfileUpdated(_) => {
                        me.refresh_all_execution_profile_ui(ctx);
                        me.reset_execution_profile_mouse_state_handles(ctx);
                        me.sync_context_window_editor(ctx, false);
                    }
                    AIExecutionProfilesModelEvent::UpdatedActiveProfile { .. } => (),
                }
                ctx.notify();
            },
        );

        let cloud_model = CloudModel::handle(ctx);
        ctx.subscribe_to_model(&cloud_model, |me, _, event, ctx| {
            let added_or_deleted_mcp_servers = matches!(
                event,
                CloudModelEvent::ObjectCreated { type_and_id } | CloudModelEvent::ObjectDeleted { type_and_id, .. }
                if matches!(
                    type_and_id.object_type(),
                    ObjectType::GenericStringObject(Json(JsonObjectType::MCPServer))
                )
            );

            if added_or_deleted_mcp_servers {
                Self::refresh_mcp_allowlist_dropdown(&me.mcp_allowlist_dropdown, ctx);
                Self::refresh_mcp_denylist_dropdown(&me.mcp_denylist_dropdown, ctx);
                ctx.notify();
            }
        });

        let templatable_manager = TemplatableMCPServerManager::handle(ctx);
        ctx.subscribe_to_model(&templatable_manager, |me, _, _event, ctx| {
            Self::refresh_mcp_allowlist_dropdown(&me.mcp_allowlist_dropdown, ctx);
            Self::refresh_mcp_denylist_dropdown(&me.mcp_denylist_dropdown, ctx);
            ctx.notify();
        });

        ctx.subscribe_to_model(
            &LLMPreferences::handle(ctx),
            |me, _, event, ctx| match event {
                LLMPreferencesEvent::UpdatedAvailableLLMs => {
                    Self::refresh_base_model_menu(&me.base_model_dropdown, ctx);
                    Self::refresh_coding_model_menu(&me.coding_model_dropdown, ctx);
                    me.sync_context_window_editor(ctx, false);
                }
                LLMPreferencesEvent::UpdatedActiveAgentModeLLM => {
                    Self::refresh_base_model_menu(&me.base_model_dropdown, ctx);
                    me.sync_context_window_editor(ctx, false);
                }
                LLMPreferencesEvent::UpdatedActiveCodingLLM => {
                    Self::refresh_coding_model_menu(&me.coding_model_dropdown, ctx);
                }
            },
        );

        // Refresh model dropdowns when BYO API keys update so key icons reflect latest state.
        ctx.subscribe_to_model(&ApiKeyManager::handle(ctx), |me, _model, _event, ctx| {
            Self::refresh_base_model_menu(&me.base_model_dropdown, ctx);
            Self::refresh_coding_model_menu(&me.coding_model_dropdown, ctx);
            me.sync_context_window_editor(ctx, false);
            ctx.notify();
        });

        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, _, event, ctx| {
            match event {
                AISettingsChangedEvent::AICommandDenylist { .. } => {
                    me.autodetection_denylist_editor.update(ctx, |editor, ctx| {
                        let denylist_value = &AISettings::as_ref(ctx)
                            .autodetection_command_denylist
                            .value()
                            .clone();
                        editor.set_buffer_text(denylist_value, ctx);
                    });
                }
                AISettingsChangedEvent::IsAnyAIEnabled { .. } => {
                    let is_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);
                    let ai_autonomy_settings = UserWorkspaces::as_ref(ctx).ai_autonomy_settings();

                    Self::update_editor_interaction_state(
                        me.autodetection_denylist_editor.clone(),
                        is_enabled,
                        ctx,
                    );
                    Self::update_editor_interaction_state(
                        me.command_execution_allowlist_editor
                            .as_ref(ctx)
                            .editor()
                            .clone(),
                        is_enabled,
                        ctx,
                    );
                    Self::update_editor_interaction_state(
                        me.command_execution_denylist_editor
                            .as_ref(ctx)
                            .editor()
                            .clone(),
                        is_enabled,
                        ctx,
                    );
                    Self::update_editor_interaction_state(
                        me.code_read_allowlist_editor.as_ref(ctx).editor().clone(),
                        is_enabled,
                        ctx,
                    );

                    Self::update_editor_interaction_state(
                        me.directory_allowlist_editor.as_ref(ctx).editor().clone(),
                        is_enabled && !ai_autonomy_settings.has_override_for_read_files_allowlist(),
                        ctx,
                    );

                    Self::update_editor_interaction_state(
                        me.command_denylist_editor.as_ref(ctx).editor().clone(),
                        is_enabled,
                        ctx,
                    );

                    Self::update_editor_interaction_state(
                        me.command_allowlist_editor.as_ref(ctx).editor().clone(),
                        is_enabled
                            && !ai_autonomy_settings.has_override_for_execute_commands_allowlist(),
                        ctx,
                    );

                    me.update_voice_input_dropdown_enablement(ctx);
                    Self::refresh_autonomy_dropdown_menu(&me.autonomy_dropdown_menu, ctx);

                    me.refresh_all_execution_profile_ui(ctx);

                    Self::refresh_code_read_autonomy_dropdown_menu(
                        &me.code_read_autonomy_dropdown_menu,
                        ctx,
                    );
                    Self::refresh_base_model_menu(&me.base_model_dropdown, ctx);
                    Self::refresh_coding_model_menu(&me.coding_model_dropdown, ctx);
                    Self::refresh_mcp_allowlist_dropdown(&me.mcp_allowlist_dropdown, ctx);
                    Self::refresh_mcp_denylist_dropdown(&me.mcp_denylist_dropdown, ctx);
                    me.sync_context_window_editor(ctx, true);
                }
                AISettingsChangedEvent::VoiceInputEnabled { .. } => {
                    me.update_voice_input_dropdown_enablement(ctx);
                }
                AISettingsChangedEvent::AgentModeExecuteReadonlyCommands { .. } => {
                    Self::refresh_autonomy_dropdown_menu(&me.autonomy_dropdown_menu, ctx);
                    Self::refresh_code_read_autonomy_dropdown_menu(
                        &me.code_read_autonomy_dropdown_menu,
                        ctx,
                    );
                }
                AISettingsChangedEvent::AgentModeCodingPermissions { .. } => {
                    Self::refresh_code_read_autonomy_dropdown_menu(
                        &me.code_read_autonomy_dropdown_menu,
                        ctx,
                    );
                }
                AISettingsChangedEvent::VoiceInputToggleKey { .. } => {
                    let current_value = AISettings::as_ref(ctx)
                        .voice_input_toggle_key
                        .value()
                        .display_name();
                    me.voice_input_toggle_key_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_name(current_value, ctx)
                        });
                }
                AISettingsChangedEvent::AgentModeCommandExecutionAllowlist { .. } => {
                    me.command_execution_allowlist_mouse_state_handles = AISettings::as_ref(ctx)
                        .agent_mode_command_execution_allowlist
                        .value()
                        .iter()
                        .map(|_| Default::default())
                        .collect();
                }
                AISettingsChangedEvent::AgentModeCommandExecutionDenylist { .. } => {
                    me.command_execution_denylist_mouse_state_handles = AISettings::as_ref(ctx)
                        .agent_mode_command_execution_denylist
                        .value()
                        .iter()
                        .map(|_| Default::default())
                        .collect();
                }
                AISettingsChangedEvent::AgentModeCodingFileReadAllowlist { .. } => {
                    me.code_read_allowlist_mouse_state_handles = AISettings::as_ref(ctx)
                        .agent_mode_coding_file_read_allowlist
                        .value()
                        .iter()
                        .map(|_| Default::default())
                        .collect();
                }
                AISettingsChangedEvent::CLIAgentToolbarEnabledCommands { .. } => {
                    me.cli_agent_footer_command_mouse_state_handles = AISettings::as_ref(ctx)
                        .cli_agent_footer_enabled_commands
                        .value()
                        .keys()
                        .map(|_| Default::default())
                        .collect();
                    me.cli_agent_footer_command_agent_dropdowns =
                        Self::create_cli_agent_dropdowns(ctx);
                }
                AISettingsChangedEvent::ThinkingDisplayMode { .. } => {
                    let current_mode = *AISettings::as_ref(ctx).thinking_display_mode.value();
                    me.thinking_display_mode_dropdown
                        .update(ctx, |dropdown, ctx| {
                            dropdown.set_selected_by_action(
                                AISettingsPageAction::SetThinkingDisplayMode(current_mode),
                                ctx,
                            );
                        });
                }
                _ => (),
            }
            ctx.notify();
        });

        ctx.subscribe_to_model(&SessionSettings::handle(ctx), |_, _, event, ctx| {
            if let SessionSettingsChangedEvent::ShowModelSelectorsInPrompt { .. } = event {
                ctx.notify();
            }
        });

        ctx.subscribe_to_model(&InputSettings::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });

        let current_permission =
            BlocklistAIPermissions::as_ref(ctx).active_permissions_profile(ctx, None);

        let apply_code_diffs_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);

            dropdown.set_items(
                vec![
                    DropdownItem::new(
                        "Agent decides",
                        AISettingsPageAction::SetApplyCodeDiffs(ActionPermission::AgentDecides),
                    ),
                    DropdownItem::new(
                        "Always allow",
                        AISettingsPageAction::SetApplyCodeDiffs(ActionPermission::AlwaysAllow),
                    ),
                    DropdownItem::new(
                        "Always ask",
                        AISettingsPageAction::SetApplyCodeDiffs(ActionPermission::AlwaysAsk),
                    ),
                ],
                ctx,
            );
            dropdown
        });
        Self::refresh_execution_profile_dropdown_menu(
            &apply_code_diffs_dropdown_menu,
            current_permission.apply_code_diffs,
            !AISettings::as_ref(ctx).is_code_diffs_permissions_editable(ctx),
            ctx,
        );

        let read_files_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_items(
                vec![
                    DropdownItem::new(
                        "Agent decides",
                        AISettingsPageAction::SetReadFiles(ActionPermission::AgentDecides),
                    ),
                    DropdownItem::new(
                        "Always allow",
                        AISettingsPageAction::SetReadFiles(ActionPermission::AlwaysAllow),
                    ),
                    DropdownItem::new(
                        "Always ask",
                        AISettingsPageAction::SetReadFiles(ActionPermission::AlwaysAsk),
                    ),
                ],
                ctx,
            );
            dropdown
        });
        Self::refresh_execution_profile_dropdown_menu(
            &read_files_dropdown_menu,
            current_permission.read_files,
            !AISettings::as_ref(ctx).is_read_files_permissions_editable(ctx),
            ctx,
        );

        let execute_commands_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_items(
                vec![
                    DropdownItem::new(
                        "Agent decides",
                        AISettingsPageAction::SetExecuteCommands(ActionPermission::AgentDecides),
                    ),
                    DropdownItem::new(
                        "Always allow",
                        AISettingsPageAction::SetExecuteCommands(ActionPermission::AlwaysAllow),
                    ),
                    DropdownItem::new(
                        "Always ask",
                        AISettingsPageAction::SetExecuteCommands(ActionPermission::AlwaysAsk),
                    ),
                ],
                ctx,
            );
            dropdown
        });
        Self::refresh_execution_profile_dropdown_menu(
            &execute_commands_dropdown_menu,
            current_permission.execute_commands,
            !AISettings::as_ref(ctx).is_execute_commands_permissions_editable(ctx),
            ctx,
        );

        let write_to_pty_autonomy_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_items(
                vec![
                    DropdownItem::new(
                        "Always allow",
                        AISettingsPageAction::SetWriteToPty(WriteToPtyPermission::AlwaysAllow),
                    ),
                    DropdownItem::new(
                        "Always ask",
                        AISettingsPageAction::SetWriteToPty(WriteToPtyPermission::AlwaysAsk),
                    ),
                    DropdownItem::new(
                        "Ask on first write",
                        AISettingsPageAction::SetWriteToPty(WriteToPtyPermission::AskOnFirstWrite),
                    ),
                ],
                ctx,
            );
            dropdown
        });
        Self::refresh_write_to_pty_dropdown_menu(
            &write_to_pty_autonomy_dropdown_menu,
            current_permission.write_to_pty,
            !AISettings::as_ref(ctx).is_write_to_pty_permissions_editable(ctx),
            ctx,
        );

        let mcp_permissions_dropdown_menu = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_items(
                vec![
                    DropdownItem::new(
                        "Agent decides",
                        AISettingsPageAction::SetMCPPermissions(ActionPermission::AgentDecides),
                    ),
                    DropdownItem::new(
                        "Always allow",
                        AISettingsPageAction::SetMCPPermissions(ActionPermission::AlwaysAllow),
                    ),
                    DropdownItem::new(
                        "Always ask",
                        AISettingsPageAction::SetMCPPermissions(ActionPermission::AlwaysAsk),
                    ),
                ],
                ctx,
            );
            dropdown
        });
        Self::refresh_execution_profile_dropdown_menu(
            &mcp_permissions_dropdown_menu,
            current_permission.mcp_permissions,
            !AISettings::as_ref(ctx).is_mcp_permission_editable(ctx),
            ctx,
        );

        let mcp_allowlist_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = FilterableDropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_header_to_static("Select MCP servers");
            dropdown
        });
        Self::refresh_mcp_allowlist_dropdown(&mcp_allowlist_dropdown, ctx);
        let mcp_allowlist_mouse_state_handles = BlocklistAIPermissions::as_ref(ctx)
            .get_mcp_allowlist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();

        let mcp_denylist_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = FilterableDropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_header_to_static("Select MCP servers");
            dropdown
        });
        Self::refresh_mcp_denylist_dropdown(&mcp_denylist_dropdown, ctx);
        let mcp_denylist_mouse_state_handles = BlocklistAIPermissions::as_ref(ctx)
            .get_mcp_denylist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();

        let command_execution_allowlist_mouse_state_handles = AISettings::as_ref(ctx)
            .agent_mode_command_execution_allowlist
            .value()
            .iter()
            .map(|_| Default::default())
            .collect();

        let command_execution_denylist_mouse_state_handles = AISettings::as_ref(ctx)
            .agent_mode_command_execution_denylist
            .value()
            .iter()
            .map(|_| Default::default())
            .collect();
        let cli_agent_footer_command_mouse_state_handles = AISettings::as_ref(ctx)
            .cli_agent_footer_enabled_commands
            .value()
            .keys()
            .map(|_| Default::default())
            .collect();

        let code_read_allowlist_mouse_state_handles = AISettings::as_ref(ctx)
            .agent_mode_coding_file_read_allowlist
            .value()
            .iter()
            .map(|_| Default::default())
            .collect();

        let directory_allowlist_mouse_state_handles = current_permission
            .directory_allowlist
            .iter()
            .map(|_| Default::default())
            .collect();

        let directory_allowlist_editor = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx).validate_on_submit(|s| {
                let expanded = host_native_absolute_path(s, &None, &None);
                Path::new(&expanded).is_dir()
            });
            input.set_placeholder_text("e.g. ~/code-repos/repo", ctx);
            input
        });

        Self::update_editor_interaction_state(
            directory_allowlist_editor.as_ref(ctx).editor().clone(),
            is_any_ai_enabled,
            ctx,
        );

        ctx.subscribe_to_view(&directory_allowlist_editor, |_, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(s) = event {
                let expanded = host_native_absolute_path(s, &None, &None);
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();

                    model.add_to_directory_allowlist(*profile_id, &PathBuf::from(expanded), ctx);
                });
                ctx.notify();
            }
        });

        let org_denylist = BlocklistAIPermissions::get_org_execute_commands_denylist(ctx);
        let command_denylist_mouse_state_handles = current_permission
            .command_denylist
            .iter()
            .map(|_| Default::default())
            .collect();
        let command_denylist_tooltip_mouse_state_handles: Vec<MouseStateHandle> =
            org_denylist.iter().map(|_| Default::default()).collect();

        let command_denylist_editor = ctx.add_typed_action_view(|ctx| {
            let mut input =
                SubmittableTextInput::new(ctx).validate_on_edit(|s| Regex::new(s).is_ok());
            input.set_placeholder_text("e.g. rm .*", ctx);
            input
        });
        Self::update_editor_interaction_state(
            command_denylist_editor.as_ref(ctx).editor().clone(),
            is_any_ai_enabled && !ai_autonomy_settings.has_override_for_execute_commands_denylist(),
            ctx,
        );

        ctx.subscribe_to_view(&command_denylist_editor, |_, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(s) = event {
                let predicate = match AgentModeCommandExecutionPredicate::new_regex(s) {
                    Ok(regex) => regex,
                    Err(e) => {
                        log::warn!(
                            "Failed to convert string to regex for cmd execution denylist: {e}"
                        );
                        return;
                    }
                };
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.add_to_command_denylist(*profile_id, &predicate, ctx);
                });
                ctx.notify();
            }
        });

        let command_allowlist_mouse_state_handles = current_permission
            .command_allowlist
            .iter()
            .map(|_| Default::default())
            .collect();

        let command_allowlist_editor = ctx.add_typed_action_view(|ctx| {
            let mut input =
                SubmittableTextInput::new(ctx).validate_on_edit(|s| Regex::new(s).is_ok());
            input.set_placeholder_text("e.g. ls .*", ctx);
            input
        });
        Self::update_editor_interaction_state(
            command_allowlist_editor.as_ref(ctx).editor().clone(),
            is_any_ai_enabled
                && !ai_autonomy_settings.has_override_for_execute_commands_allowlist(),
            ctx,
        );

        ctx.subscribe_to_view(&command_allowlist_editor, |_, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(s) = event {
                let predicate = match AgentModeCommandExecutionPredicate::new_regex(s) {
                    Ok(regex) => regex,
                    Err(e) => {
                        log::warn!(
                            "Failed to convert string to regex for cmd execution allowlist: {e}"
                        );
                        return;
                    }
                };
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.add_to_command_allowlist(*profile_id, &predicate, ctx);
                });
                ctx.notify();
            }
        });

        let ai_request_model = AIRequestUsageModel::handle(ctx);
        ctx.subscribe_to_model(&ai_request_model, |me, _, event, ctx| {
            match event {
                AIRequestUsageModelEvent::RequestUsageUpdated => ctx.notify(),
                AIRequestUsageModelEvent::RequestBonusRefunded { .. } => ctx.notify(),
            }
            Self::refresh_base_model_menu(&me.base_model_dropdown, ctx);
            Self::refresh_coding_model_menu(&me.coding_model_dropdown, ctx);
        });

        let profile_views = Self::create_profile_views(ctx);

        let add_profile_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Add Profile", SecondaryTheme)
                .with_icon(Icon::Plus)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AISettingsPageAction::CreateProfile);
                })
        });

        add_profile_button.update(ctx, |button, ctx| {
            button.set_disabled(!is_any_ai_enabled, ctx);
        });
        let agent_toolbar_inline_editor = ctx.add_typed_action_view(|ctx| {
            AgentToolbarInlineEditor::new(AgentToolbarEditorMode::AgentView, ctx)
        });
        let cli_agent_toolbar_inline_editor = ctx.add_typed_action_view(|ctx| {
            AgentToolbarInlineEditor::new(AgentToolbarEditorMode::CLIAgent, ctx)
        });

        #[cfg(feature = "local_fs")]
        let conversation_layout_dropdown = ctx.add_typed_action_view(|ctx| {
            use crate::util::file::external_editor::settings::OpenConversationPreference;

            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);

            let items = vec![
                DropdownItem::new(
                    "New Tab",
                    AISettingsPageAction::SetConversationLayout(OpenConversationPreference::NewTab),
                ),
                DropdownItem::new(
                    "Split Pane",
                    AISettingsPageAction::SetConversationLayout(
                        OpenConversationPreference::SplitPane,
                    ),
                ),
            ];
            dropdown.set_items(items, ctx);

            let current = *crate::util::file::external_editor::EditorSettings::as_ref(ctx)
                .open_conversation_layout_preference;
            match current {
                OpenConversationPreference::NewTab => dropdown.set_selected_by_name("New Tab", ctx),
                OpenConversationPreference::SplitPane => {
                    dropdown.set_selected_by_name("Split Pane", ctx)
                }
            };
            dropdown
        });

        let provider_id_editor = Self::create_provider_id_editor(ctx);
        let provider_display_name_editor = Self::create_provider_display_name_editor(ctx);
        let provider_base_url_editor = Self::create_provider_base_url_editor(ctx);
        let provider_model_editor = Self::create_provider_model_editor(ctx);
        let provider_env_var_editor = Self::create_provider_env_var_editor(ctx);
        let provider_secret_key_editor = Self::create_provider_secret_key_editor(ctx);
        let provider_defaults_temperature_editor = Self::create_provider_defaults_temperature_editor(ctx);
        let provider_defaults_top_p_editor = Self::create_provider_defaults_top_p_editor(ctx);
        let provider_defaults_max_output_tokens_editor = Self::create_provider_defaults_max_output_tokens_editor(ctx);
        let provider_defaults_timeout_ms_editor = Self::create_provider_defaults_timeout_ms_editor(ctx);

        Self {
            page: Self::build_page(None, ctx,
                &provider_id_editor, &provider_display_name_editor, &provider_base_url_editor,
                &provider_model_editor, &provider_env_var_editor, &provider_secret_key_editor,
                &provider_defaults_temperature_editor, &provider_defaults_top_p_editor,
                &provider_defaults_max_output_tokens_editor, &provider_defaults_timeout_ms_editor,
            ),
            active_subpage: None,
            voice_input_toggle_key_dropdown,
            autodetection_denylist_editor,
            local_only_icon_tooltip_states: Default::default(),
            command_execution_allowlist_editor,
            command_execution_denylist_editor,
            command_execution_allowlist_mouse_state_handles,
            command_execution_denylist_mouse_state_handles,
            cli_agent_footer_command_editor,
            cli_agent_footer_command_mouse_state_handles,
            cli_agent_footer_command_agent_dropdowns: Self::create_cli_agent_dropdowns(ctx),
            agent_toolbar_inline_editor,
            cli_agent_toolbar_inline_editor,
            base_model_dropdown,
            coding_model_dropdown,
            context_window_slider_state,
            context_window_editor,
            last_synced_context_window_editor_value,
            autonomy_dropdown_menu,
            code_read_allowlist_editor,
            code_read_autonomy_dropdown_menu,
            code_read_allowlist_mouse_state_handles,
            apply_code_diffs_dropdown_menu,
            read_files_dropdown_menu,
            execute_commands_dropdown_menu,
            write_to_pty_autonomy_dropdown_menu,
            mcp_permissions_dropdown_menu,
            directory_allowlist_mouse_state_handles,
            directory_allowlist_editor,
            command_denylist_mouse_state_handles,
            command_denylist_tooltip_mouse_state_handles,
            command_denylist_editor,
            command_allowlist_mouse_state_handles,
            command_allowlist_editor,
            mcp_allowlist_dropdown,
            mcp_allowlist_mouse_state_handles,
            mcp_denylist_dropdown,
            mcp_denylist_mouse_state_handles,
            thinking_display_mode_dropdown,
            #[cfg(feature = "local_fs")]
            conversation_layout_dropdown,
            profile_views,
            add_profile_button,
            provider_draft: ProviderDraft::default(),
            provider_validation_states: HashMap::new(),
            provider_id_editor,
            provider_display_name_editor,
            provider_base_url_editor,
            provider_model_editor,
            provider_env_var_editor,
            provider_secret_key_editor,
            provider_defaults_temperature_editor,
            provider_defaults_top_p_editor,
            provider_defaults_max_output_tokens_editor,
            provider_defaults_timeout_ms_editor,
            import_dialog_visible: false,
            import_json_text: String::new(),
            import_pending_template: None,
            import_collision: None,
            import_secret_prompt: None,
            import_secret_value: String::new(),
        }
    }

    fn update_voice_input_dropdown_enablement(&mut self, ctx: &mut ViewContext<Self>) {
        let is_voice_enabled = AISettings::as_ref(ctx).is_voice_input_enabled(ctx);
        self.voice_input_toggle_key_dropdown
            .update(ctx, |dropdown, ctx| {
                if is_voice_enabled {
                    dropdown.set_enabled(ctx);
                } else {
                    dropdown.set_disabled(ctx);
                }
            });
        ctx.notify();
    }

    /// Set the active subpage and rebuild the widget list to show only relevant widgets.
    pub fn set_active_subpage(&mut self, subpage: Option<AISubpage>, ctx: &mut ViewContext<Self>) {
        if self.active_subpage != subpage {
            self.active_subpage = subpage;
            self.page = Self::build_page(
                subpage, ctx,
                &self.provider_id_editor, &self.provider_display_name_editor,
                &self.provider_base_url_editor, &self.provider_model_editor,
                &self.provider_env_var_editor, &self.provider_secret_key_editor,
                &self.provider_defaults_temperature_editor, &self.provider_defaults_top_p_editor,
                &self.provider_defaults_max_output_tokens_editor, &self.provider_defaults_timeout_ms_editor,
            );
            ctx.notify();
        }
    }

    fn start_provider_validation(&mut self, provider_id: String, ctx: &mut ViewContext<Self>) {
        let Some(profile) = provider_profile_for_action(&provider_id, ctx) else {
            return;
        };
        let secrets = provider_secret_values(&profile, ApiKeyManager::as_ref(ctx).keys(), ctx);

        self.provider_validation_states
            .insert(provider_id.clone(), ProviderValidationUiState::Validating);
        ctx.notify();

        ctx.spawn(
            async move {
                let validator = ProviderValidator::new(reqwest::Client::new());
                let result = validator
                    .validate(&profile, ValidationLevel::Cheap, |ref_| {
                        provider_secret_value(ref_, &secrets)
                    })
                    .await;
                (provider_id, result)
            },
            |me, (provider_id, result), ctx| {
                me.provider_validation_states.insert(
                    provider_id.clone(),
                    ProviderValidationUiState::ValidationComplete(result.clone()),
                );
                update_provider_registry(ctx, |registry| {
                    registry.apply_validation_result(&provider_id, &result);
                });
                ctx.notify();
            },
        );
    }

    fn start_provider_discovery(&mut self, provider_id: String, ctx: &mut ViewContext<Self>) {
        let Some(profile) = provider_profile_for_action(&provider_id, ctx) else {
            return;
        };
        let secrets = provider_secret_values(&profile, ApiKeyManager::as_ref(ctx).keys(), ctx);

        self.provider_validation_states
            .insert(provider_id.clone(), ProviderValidationUiState::Discovering);
        ctx.notify();

        ctx.spawn(
            async move {
                let validator = ProviderValidator::new(reqwest::Client::new());
                let result = validator
                    .discover_models(&profile, |ref_| provider_secret_value(ref_, &secrets))
                    .await;
                (provider_id, result)
            },
            |me, (provider_id, result), ctx| {
                match result {
                    Ok(models) => {
                        me.provider_validation_states.insert(
                            provider_id.clone(),
                            ProviderValidationUiState::DiscoveryComplete(models.clone()),
                        );
                        update_provider_registry(ctx, |registry| {
                            registry.apply_discovered_models(&provider_id, models);
                        });
                    }
                    Err(error) => {
                        me.provider_validation_states.insert(
                            provider_id,
                            ProviderValidationUiState::DiscoveryFailed(error),
                        );
                    }
                }
                ctx.notify();
            },
        );
    }

    fn process_provider_import_template(
        &mut self,
        template: ProviderExportTemplate,
        collision_resolution: Option<(String, ImportCollisionAction)>,
        ctx: &mut ViewContext<Self>,
    ) {
        let registry = ApiKeyManager::as_ref(ctx).provider_registry_from_legacy_keys(Some(
            AISettings::as_ref(ctx).provider_registry.value().clone(),
        ));
        let mut import_registry = registry.clone();
        let mut imported_profiles = Vec::new();

        for exported in &template.providers {
            let mut exported = exported.clone();
            if let Some((provider_id, action)) = &collision_resolution {
                if &exported.id == provider_id {
                    match action {
                        ImportCollisionAction::Overwrite => {
                            import_registry.providers.remove(provider_id);
                        }
                        ImportCollisionAction::Rename => {
                            exported.id =
                                unique_provider_id(&import_registry.providers, &exported.id);
                            exported.display_name = format!("{} Import", exported.display_name);
                        }
                        ImportCollisionAction::Skip => continue,
                    }
                }
            }

            match import_provider(&exported, &import_registry) {
                Ok(result) => {
                    if result.needs_secret {
                        self.import_pending_template =
                            Some(ProviderExportTemplate::new(vec![exported.clone()]));
                        self.import_collision = None;
                        self.import_secret_prompt = Some(ImportSecretPromptState {
                            provider_id: result.profile.id,
                            secret_hint: result.secret_hint.unwrap_or_default(),
                        });
                        self.import_secret_value.clear();
                        ctx.notify();
                        return;
                    }
                    import_registry.upsert_provider(result.profile.clone());
                    imported_profiles.push(result.profile);
                }
                Err(ImportError::IdCollision(provider_id)) => {
                    self.import_pending_template = Some(template);
                    self.import_collision = Some((provider_id, ImportCollisionAction::Skip));
                    self.import_secret_prompt = None;
                    ctx.notify();
                    return;
                }
            }
        }

        update_provider_registry(ctx, |registry| {
            for profile in imported_profiles {
                registry.upsert_provider(profile);
            }
        });
        self.import_dialog_visible = false;
        self.import_json_text.clear();
        self.import_pending_template = None;
        self.import_collision = None;
        self.import_secret_prompt = None;
        self.import_secret_value.clear();
        ctx.notify();
    }

    fn import_provider_secret(
        &mut self,
        template: &ProviderExportTemplate,
        provider_id: &str,
        secret_value: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(exported) = template
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
        else {
            return;
        };

        let registry = ApiKeyManager::as_ref(ctx).provider_registry_from_legacy_keys(Some(
            AISettings::as_ref(ctx).provider_registry.value().clone(),
        ));
        let Ok(result) = import_provider(exported, &registry) else {
            return;
        };
        let mut profile = result.profile;
        let secret_ref = match &mut profile.auth {
            AuthConfig::ApiKey {
                source: AuthSource::Keychain,
                secret_ref,
                ..
            } => {
                let ref_ = secret_ref
                    .clone()
                    .unwrap_or_else(|| format!("keychain://warp/providers/{provider_id}"));
                *secret_ref = Some(ref_.clone());
                ref_
            }
            AuthConfig::BearerToken {
                source: AuthSource::Keychain,
                secret_ref,
            } => {
                if secret_ref.is_empty() {
                    *secret_ref = format!("keychain://warp/providers/{provider_id}");
                }
                secret_ref.clone()
            }
            AuthConfig::None
            | AuthConfig::ApiKey {
                source: AuthSource::Env,
                ..
            }
            | AuthConfig::BearerToken {
                source: AuthSource::Env,
                ..
            } => return,
        };

        if let Err(error) = ctx.secure_storage().write_value(&secret_ref, secret_value) {
            log::error!("Failed to store imported provider secret: {error:#}");
            return;
        }

        update_provider_registry(ctx, |registry| {
            registry.upsert_provider(profile);
        });
        self.import_dialog_visible = false;
        self.import_json_text.clear();
        self.import_pending_template = None;
        self.import_collision = None;
        self.import_secret_prompt = None;
        self.import_secret_value.clear();
    }

    fn build_page(
        subpage: Option<AISubpage>,
        ctx: &mut ViewContext<Self>,
        provider_id_editor: &ViewHandle<EditorView>,
        provider_display_name_editor: &ViewHandle<EditorView>,
        provider_base_url_editor: &ViewHandle<EditorView>,
        provider_model_editor: &ViewHandle<EditorView>,
        provider_env_var_editor: &ViewHandle<EditorView>,
        provider_secret_key_editor: &ViewHandle<EditorView>,
        provider_defaults_temperature_editor: &ViewHandle<EditorView>,
        provider_defaults_top_p_editor: &ViewHandle<EditorView>,
        provider_defaults_max_output_tokens_editor: &ViewHandle<EditorView>,
        provider_defaults_timeout_ms_editor: &ViewHandle<EditorView>,
    ) -> PageType<Self> {
        let ai_settings = AISettings::as_ref(ctx);

        let mut widgets: Vec<Box<dyn SettingsWidget<View = AISettingsPageView>>> = Vec::new();

        // When viewing a specific subpage, only include its widgets.
        // When subpage is None (legacy/backward-compat), show all widgets.
        match subpage {
            None => {
                // Full page: all widgets (legacy behavior)
                widgets.push(Box::new(GlobalAIWidget::default()));
                if !FeatureFlag::UsageBasedPricing.is_enabled() {
                    widgets.push(Box::new(UsageWidget::default()));
                }
                if ai_settings
                    .intelligent_autosuggestions_enabled_internal
                    .is_supported_on_current_platform()
                    || ai_settings
                        .prompt_suggestions_enabled_internal
                        .is_supported_on_current_platform()
                    || (FeatureFlag::PredictAMQueries.is_enabled()
                        && ai_settings
                            .natural_language_autosuggestions_enabled_internal
                            .is_supported_on_current_platform())
                    || (FeatureFlag::SharedBlockTitleGeneration.is_enabled()
                        && ai_settings
                            .shared_block_title_generation_enabled_internal
                            .is_supported_on_current_platform())
                    || (FeatureFlag::GitOperationsInCodeReview.is_enabled()
                        && ai_settings
                            .git_operations_autogen_enabled_internal
                            .is_supported_on_current_platform())
                {
                    widgets.push(Box::new(ActiveAIWidget::default()));
                }
                widgets.push(Box::new(AgentsWidget::default()));
                widgets.push(Box::new(AIInputWidget::default()));
                if MCPServersWidget::should_show_mcp() {
                    widgets.push(Box::new(MCPServersWidget::default()));
                }
                if FeatureFlag::AIRules.is_enabled() {
                    widgets.push(Box::new(AIFactWidget::default()));
                }
                if cfg!(feature = "voice_input")
                    && ai_settings
                        .voice_input_enabled_internal
                        .is_supported_on_current_platform()
                {
                    widgets.push(Box::new(VoiceWidget::default()));
                }
                widgets.push(Box::new(CLIAgentWidget::default()));
                widgets.push(Box::new(ProvidersWidget::new(
                    ctx,
                    provider_id_editor.clone(),
                    provider_display_name_editor.clone(),
                    provider_base_url_editor.clone(),
                    provider_model_editor.clone(),
                    provider_env_var_editor.clone(),
                    provider_secret_key_editor.clone(),
                    provider_defaults_temperature_editor.clone(),
                    provider_defaults_top_p_editor.clone(),
                    provider_defaults_max_output_tokens_editor.clone(),
                    provider_defaults_timeout_ms_editor.clone(),
                )));
                widgets.push(Box::new(AwsBedrockWidget::new(ctx)));
                widgets.push(Box::new(AgentAttributionWidget::default()));
                widgets.push(Box::new(OtherAIWidget::default()));
                if FeatureFlag::AgentModeComputerUse.is_enabled() {
                    widgets.push(Box::new(CloudAgentComputerUseWidget::default()));
                }
            }
            Some(AISubpage::WarpAgent) => {
                // Oz page: global toggle + Active AI + Input + Other
                widgets.push(Box::new(GlobalAIWidget::default()));
                if ai_settings
                    .intelligent_autosuggestions_enabled_internal
                    .is_supported_on_current_platform()
                    || ai_settings
                        .prompt_suggestions_enabled_internal
                        .is_supported_on_current_platform()
                    || (FeatureFlag::PredictAMQueries.is_enabled()
                        && ai_settings
                            .natural_language_autosuggestions_enabled_internal
                            .is_supported_on_current_platform())
                    || (FeatureFlag::SharedBlockTitleGeneration.is_enabled()
                        && ai_settings
                            .shared_block_title_generation_enabled_internal
                            .is_supported_on_current_platform())
                    || (FeatureFlag::GitOperationsInCodeReview.is_enabled()
                        && ai_settings
                            .git_operations_autogen_enabled_internal
                            .is_supported_on_current_platform())
                {
                    widgets.push(Box::new(ActiveAIWidget::default()));
                }
                widgets.push(Box::new(AIInputWidget::default()));
                let voice_supported = cfg!(feature = "voice_input")
                    && ai_settings
                        .voice_input_enabled_internal
                        .is_supported_on_current_platform();
                if voice_supported {
                    widgets.push(Box::new(VoiceWidget::default()));
                }
                widgets.push(Box::new(ProvidersWidget::new(
                    ctx,
                    provider_id_editor.clone(),
                    provider_display_name_editor.clone(),
                    provider_base_url_editor.clone(),
                    provider_model_editor.clone(),
                    provider_env_var_editor.clone(),
                    provider_secret_key_editor.clone(),
                    provider_defaults_temperature_editor.clone(),
                    provider_defaults_top_p_editor.clone(),
                    provider_defaults_max_output_tokens_editor.clone(),
                    provider_defaults_timeout_ms_editor.clone(),
                )));
                widgets.push(Box::new(AwsBedrockWidget::new(ctx)));
                widgets.push(Box::new(AgentAttributionWidget::default()));
                widgets.push(Box::new(OtherAIWidget::default()));
                if FeatureFlag::AgentModeComputerUse.is_enabled() {
                    widgets.push(Box::new(CloudAgentComputerUseWidget::default()));
                }
            }
            Some(AISubpage::Profiles) => {
                if !FeatureFlag::UsageBasedPricing.is_enabled() {
                    widgets.push(Box::new(UsageWidget::default()));
                }
                widgets.push(Box::new(AgentsWidget::default()));
            }
            Some(AISubpage::Knowledge) => {
                if FeatureFlag::AIRules.is_enabled() {
                    widgets.push(Box::new(AIFactWidget::default()));
                }
            }
            Some(AISubpage::ThirdPartyCLIAgents) => {
                widgets.push(Box::new(CLIAgentWidget::default()));
            }
        }

        // Subpage widgets render their own subheader-sized titles internally,
        // so we don't pass a page-level title to PageType.
        let title: Option<&str> = None;
        PageType::new_uncategorized(widgets, title)
    }

    fn handle_context_window_editor_event(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                if !AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
                    self.sync_context_window_editor(ctx, true);
                    return;
                }
                if let Some(cw) = Self::configurable_context_window(ctx) {
                    let buffer_text = self.context_window_editor.as_ref(ctx).buffer_text(ctx);
                    let cleaned: String = buffer_text
                        .chars()
                        .filter(|c| !c.is_whitespace() && *c != ',')
                        .collect();
                    if let Ok(parsed) = cleaned.parse::<u32>() {
                        let clamped = parsed.clamp(cw.min, cw.max);
                        if Some(clamped) != Self::current_context_window_display_value(ctx) {
                            AIExecutionProfilesModel::handle(ctx).update(
                                ctx,
                                |profiles_model, ctx| {
                                    let profile_id = *profiles_model.active_profile(None, ctx).id();
                                    profiles_model.set_context_window_limit(
                                        profile_id,
                                        Some(clamped),
                                        ctx,
                                    );
                                },
                            );
                        }
                    }
                }
                self.sync_context_window_editor(ctx, true);
                if let EditorEvent::Enter = event {
                    ctx.emit(AISettingsPageEvent::FocusModal);
                }
                ctx.notify();
            }
            EditorEvent::Escape => ctx.emit(AISettingsPageEvent::FocusModal),
            _ => {}
        }
    }

    fn active_profile_data(app: &AppContext) -> AIExecutionProfile {
        AIExecutionProfilesModel::as_ref(app)
            .active_profile(None, app)
            .data()
            .clone()
    }

    fn configurable_context_window(app: &AppContext) -> Option<LLMContextWindow> {
        Self::active_profile_data(app).configurable_context_window(app)
    }

    fn current_context_window_display_value(app: &AppContext) -> Option<u32> {
        Self::active_profile_data(app).context_window_display_value(app)
    }

    fn initial_context_window_value(app: &AppContext) -> u32 {
        Self::current_context_window_display_value(app).unwrap_or_else(|| {
            LLMPreferences::as_ref(app)
                .get_active_base_model(app, None)
                .context_window
                .default_max
        })
    }

    fn sync_context_window_editor(&mut self, ctx: &mut ViewContext<Self>, force: bool) {
        let Some(value) = Self::current_context_window_display_value(ctx) else {
            self.last_synced_context_window_editor_value = None;
            self.context_window_slider_state.reset_offset();
            ctx.notify();
            return;
        };

        let formatted = value.to_string();
        let should_update = if force {
            true
        } else {
            match self.last_synced_context_window_editor_value {
                Some(last_value) => {
                    self.context_window_editor.as_ref(ctx).buffer_text(ctx)
                        == last_value.to_string()
                }
                None => true,
            }
        };

        if should_update {
            self.context_window_editor.update(ctx, |editor, ctx| {
                if editor.buffer_text(ctx) != formatted {
                    editor.system_reset_buffer_text(&formatted, ctx);
                }
            });
            self.last_synced_context_window_editor_value = Some(value);
            self.context_window_slider_state.reset_offset();
            ctx.notify();
        }
    }

    fn handle_detection_denylist_editor_event(
        &mut self,
        event: &EditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            EditorEvent::Blurred | EditorEvent::Enter => {
                let buffer_text = self
                    .autodetection_denylist_editor
                    .as_ref(ctx)
                    .buffer_text(ctx);
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    if let Err(e) = settings
                        .autodetection_command_denylist
                        .set_value(buffer_text, ctx)
                    {
                        log::warn!("Failed to set AI autodetection blacklist commands: {e:?}");
                    }
                })
            }
            EditorEvent::Escape => ctx.emit(AISettingsPageEvent::FocusModal),
            _ => {}
        }
    }

    fn update_editor_interaction_state(
        editor: ViewHandle<EditorView>,
        is_enabled: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        editor.update(ctx, |editor, ctx| {
            let interaction_state = if is_enabled {
                InteractionState::Editable
            } else {
                InteractionState::Disabled
            };
            editor.set_interaction_state(interaction_state, ctx);
            ctx.notify();
        })
    }

    pub fn refresh_base_model_menu(
        menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        menu.update(ctx, |menu, ctx| {
            let disabled_by_ai_toggle = !AISettings::as_ref(ctx).is_any_ai_enabled(ctx);

            if disabled_by_ai_toggle {
                menu.set_disabled(ctx);
            } else {
                menu.set_enabled(ctx);
            }

            let choices = LLMPreferences::as_ref(ctx)
                .get_base_llm_choices_for_agent_mode()
                .collect_vec();

            let items = available_model_menu_items(
                choices,
                |llm| AISettingsPageAction::SetBaseModel(llm.id.clone()).into(),
                None,
                None,
                false,
                false,
                ctx,
            );
            menu.set_rich_items(items, ctx);

            let active = LLMPreferences::as_ref(ctx).get_active_base_model(ctx, None);
            menu.set_selected_by_action(AISettingsPageAction::SetBaseModel(active.id.clone()), ctx);
            ctx.notify();
        });
        ctx.notify();
    }

    pub fn refresh_coding_model_menu(
        menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        menu.update(ctx, |menu, ctx| {
            let disabled_by_ai_toggle = !AISettings::as_ref(ctx).is_any_ai_enabled(ctx);

            if disabled_by_ai_toggle {
                menu.set_disabled(ctx);
            } else {
                menu.set_enabled(ctx);
            }

            let choices = LLMPreferences::as_ref(ctx)
                .get_coding_llm_choices()
                .collect_vec();

            let items = available_model_menu_items(
                choices,
                |llm| AISettingsPageAction::SetCodingModel(llm.id.clone()).into(),
                None,
                None,
                false,
                false,
                ctx,
            );
            menu.set_rich_items(items, ctx);
            let active = LLMPreferences::as_ref(ctx).get_active_coding_model(ctx, None);

            menu.set_selected_by_action(
                AISettingsPageAction::SetCodingModel(active.id.clone()),
                ctx,
            );
            ctx.notify();
        });
        ctx.notify();
    }

    fn refresh_autonomy_dropdown_menu(
        menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        menu.update(ctx, |menu, ctx| {
            if AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
                menu.set_enabled(ctx);
            } else {
                menu.set_disabled(ctx);
            }

            menu.set_items(
                vec![
                    DropdownItem::new(
                        "Read only",
                        AISettingsPageAction::SetAutonomyReadonlyCommandsSetting,
                    ),
                    DropdownItem::new(
                        "Supervised",
                        AISettingsPageAction::SetAutonomySupervisedSetting,
                    ),
                ],
                ctx,
            );
            let active = if *AISettings::as_ref(ctx).agent_mode_execute_read_only_commands {
                0
            } else {
                1
            };
            menu.set_selected_by_index(active, ctx);
            ctx.notify();
        });
        ctx.notify();
    }

    fn refresh_all_execution_profile_ui(&self, ctx: &mut ViewContext<Self>) {
        let permissions = BlocklistAIPermissions::handle(ctx);

        let apply_code_diffs_setting = permissions
            .as_ref(ctx)
            .get_apply_code_diffs_setting(ctx, None);
        Self::refresh_execution_profile_dropdown_menu(
            &self.apply_code_diffs_dropdown_menu,
            apply_code_diffs_setting,
            !AISettings::as_ref(ctx).is_code_diffs_permissions_editable(ctx),
            ctx,
        );

        let read_files_setting = permissions.as_ref(ctx).get_read_files_setting(ctx, None);
        Self::refresh_execution_profile_dropdown_menu(
            &self.read_files_dropdown_menu,
            read_files_setting,
            !AISettings::as_ref(ctx).is_read_files_permissions_editable(ctx),
            ctx,
        );

        let execute_commands_setting: ActionPermission = permissions
            .as_ref(ctx)
            .get_execute_commands_setting(ctx, None);
        Self::refresh_execution_profile_dropdown_menu(
            &self.execute_commands_dropdown_menu,
            execute_commands_setting,
            !AISettings::as_ref(ctx).is_execute_commands_permissions_editable(ctx),
            ctx,
        );

        let write_to_pty_setting: WriteToPtyPermission =
            permissions.as_ref(ctx).get_write_to_pty_setting(ctx, None);
        Self::refresh_write_to_pty_dropdown_menu(
            &self.write_to_pty_autonomy_dropdown_menu,
            write_to_pty_setting,
            !AISettings::as_ref(ctx).is_write_to_pty_permissions_editable(ctx),
            ctx,
        );

        let mcp_permissions_setting = permissions
            .as_ref(ctx)
            .get_mcp_permissions_setting(ctx, None);
        Self::refresh_execution_profile_dropdown_menu(
            &self.mcp_permissions_dropdown_menu,
            mcp_permissions_setting,
            !AISettings::as_ref(ctx).is_mcp_permission_editable(ctx),
            ctx,
        );
        Self::refresh_mcp_allowlist_dropdown(&self.mcp_allowlist_dropdown, ctx);
        Self::refresh_mcp_denylist_dropdown(&self.mcp_denylist_dropdown, ctx);

        let is_any_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);
        self.add_profile_button.update(ctx, |button, ctx| {
            button.set_disabled(!is_any_ai_enabled, ctx);
        });
    }

    fn reset_execution_profile_mouse_state_handles(&mut self, ctx: &mut ViewContext<Self>) {
        let blocklist_permissions = BlocklistAIPermissions::as_ref(ctx);

        self.directory_allowlist_mouse_state_handles = blocklist_permissions
            .get_read_files_allowlist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();

        self.command_denylist_mouse_state_handles = blocklist_permissions
            .get_execute_commands_denylist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();

        let org_denylist = BlocklistAIPermissions::get_org_execute_commands_denylist(ctx);
        self.command_denylist_tooltip_mouse_state_handles =
            org_denylist.iter().map(|_| Default::default()).collect();

        self.command_allowlist_mouse_state_handles = blocklist_permissions
            .get_execute_commands_allowlist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();

        self.mcp_allowlist_mouse_state_handles = blocklist_permissions
            .get_mcp_allowlist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();

        self.mcp_denylist_mouse_state_handles = blocklist_permissions
            .get_mcp_denylist(ctx, None)
            .iter()
            .map(|_| Default::default())
            .collect();
    }

    fn refresh_execution_profile_dropdown_menu(
        menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        current_permission: ActionPermission,
        disabled: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        menu.update(ctx, |menu, ctx| {
            if !disabled {
                menu.set_enabled(ctx);
            } else {
                menu.set_disabled(ctx);
            }

            let active = match current_permission {
                ActionPermission::AgentDecides | ActionPermission::Unknown => 0,
                ActionPermission::AlwaysAllow => 1,
                ActionPermission::AlwaysAsk => 2,
            };

            menu.set_selected_by_index(active, ctx);
            ctx.notify();
        });
        ctx.notify();
    }

    fn refresh_write_to_pty_dropdown_menu(
        menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        current_permission: WriteToPtyPermission,
        disabled: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        menu.update(ctx, |menu, ctx| {
            if !disabled {
                menu.set_enabled(ctx);
            } else {
                menu.set_disabled(ctx);
            }

            let active = match current_permission {
                WriteToPtyPermission::AlwaysAllow => 0,
                WriteToPtyPermission::AlwaysAsk | WriteToPtyPermission::Unknown => 1,
                WriteToPtyPermission::AskOnFirstWrite => 2,
            };

            menu.set_selected_by_index(active, ctx);
            ctx.notify();
        });
        ctx.notify();
    }

    /// Currently, the coding permissions only support "read" access.
    fn refresh_code_read_autonomy_dropdown_menu(
        menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        menu.update(ctx, |menu, ctx| {
            if AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
                menu.set_enabled(ctx);
            } else {
                menu.set_disabled(ctx);
            }

            menu.set_items(
                AgentModeCodingPermissionsType::iter()
                    .map(|t| {
                        let display = match t {
                            AgentModeCodingPermissionsType::AlwaysAskBeforeReading => "Always ask",
                            AgentModeCodingPermissionsType::AlwaysAllowReading => "Always allow",
                            AgentModeCodingPermissionsType::AllowReadingSpecificFiles => {
                                "Allow in specific directories"
                            }
                        };
                        DropdownItem::new(display, AISettingsPageAction::SetCodingPermission(t))
                    })
                    .collect(),
                ctx,
            );
            let ai_settings = AISettings::as_ref(ctx);

            let active = if *ai_settings.agent_mode_execute_read_only_commands {
                menu.set_disabled(ctx);
                AgentModeCodingPermissionsType::AlwaysAllowReading
            } else {
                *ai_settings.agent_mode_coding_permissions
            };
            menu.set_selected_by_action(AISettingsPageAction::SetCodingPermission(active), ctx);
            ctx.notify();
        });
        ctx.notify();
    }

    fn get_non_allowlisted_or_denylisted_mcp_servers(
        ctx: &mut ViewContext<Self>,
    ) -> Vec<(uuid::Uuid, String)> {
        let all_mcp_servers = TemplatableMCPServerManager::get_all_cloud_synced_mcp_servers(ctx);
        let already_allowlisted_mcp_servers =
            BlocklistAIPermissions::as_ref(ctx).get_mcp_allowlist(ctx, None);
        let already_denylisted_mcp_servers =
            BlocklistAIPermissions::as_ref(ctx).get_mcp_denylist(ctx, None);

        all_mcp_servers
            .into_iter()
            .filter(|(uuid, _)| {
                let is_allowlisted = already_allowlisted_mcp_servers.contains(uuid);
                let is_denylisted = already_denylisted_mcp_servers.contains(uuid);
                !is_allowlisted && !is_denylisted
            })
            .collect()
    }

    fn refresh_menu_dropdown<F>(
        menu: &ViewHandle<FilterableDropdown<AISettingsPageAction>>,
        action_fn: F,
        ctx: &mut ViewContext<Self>,
    ) where
        F: Fn(uuid::Uuid) -> AISettingsPageAction,
    {
        let mcps_in_dropdown = Self::get_non_allowlisted_or_denylisted_mcp_servers(ctx);
        menu.update(ctx, |menu, ctx| {
            if AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
                menu.set_enabled(ctx);
            } else {
                menu.set_disabled(ctx);
            }

            let items: Vec<DropdownItem<AISettingsPageAction>> = mcps_in_dropdown
                .iter()
                .map(|(uuid, server_name)| DropdownItem::new(server_name, action_fn(*uuid)))
                .collect();

            menu.set_items(items, ctx);
            ctx.notify()
        });
        ctx.notify();
    }

    fn refresh_mcp_allowlist_dropdown(
        menu: &ViewHandle<FilterableDropdown<AISettingsPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        Self::refresh_menu_dropdown(menu, AISettingsPageAction::AddToMCPAllowlist, ctx);
    }

    fn create_profile_views(ctx: &mut ViewContext<Self>) -> Vec<ViewHandle<ExecutionProfileView>> {
        let profiles_model = AIExecutionProfilesModel::as_ref(ctx);
        let profile_ids = profiles_model.get_all_profile_ids();

        profile_ids
            .iter()
            .map(|&profile_id| {
                let profile_view =
                    ctx.add_typed_action_view(|ctx| ExecutionProfileView::new(profile_id, ctx));

                ctx.subscribe_to_view(&profile_view, move |_me, _, event, ctx| match event {
                    ExecutionProfileViewEvent::EditProfile => {
                        ctx.emit(AISettingsPageEvent::OpenExecutionProfileEditor(profile_id));
                    }
                });

                profile_view
            })
            .collect()
    }

    fn refresh_profile_views(&mut self, ctx: &mut ViewContext<Self>) {
        let new_profile_views = Self::create_profile_views(ctx);
        self.profile_views = new_profile_views;
    }

    fn refresh_mcp_denylist_dropdown(
        menu: &ViewHandle<FilterableDropdown<AISettingsPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        Self::refresh_menu_dropdown(menu, AISettingsPageAction::AddToMCPDenylist, ctx);
    }

    fn create_cli_agent_dropdowns(
        ctx: &mut ViewContext<Self>,
    ) -> Vec<ViewHandle<Dropdown<AISettingsPageAction>>> {
        let entries: Vec<(String, CLIAgent)> = AISettings::as_ref(ctx)
            .cli_agent_footer_enabled_commands
            .value()
            .iter()
            .map(|(pattern, agent_value)| {
                (pattern.clone(), CLIAgent::from_serialized_name(agent_value))
            })
            .collect();

        entries
            .into_iter()
            .map(|(pattern_clone, current_agent)| {
                ctx.add_typed_action_view(move |ctx| {
                    let mut dropdown = Dropdown::new(ctx);
                    dropdown.set_top_bar_max_width(160.);
                    dropdown.set_menu_width(180., ctx);
                    dropdown.set_main_axis_size(MainAxisSize::Min, ctx);

                    let mut items: Vec<MenuItem<DropdownAction<AISettingsPageAction>>> = Vec::new();

                    for agent in all::<CLIAgent>() {
                        if matches!(agent, CLIAgent::Unknown) {
                            continue;
                        }
                        let icon = agent.icon();
                        let mut fields = MenuItemFields::new(agent.display_name())
                            .with_on_select_action(DropdownAction::SelectActionAndClose(
                                AISettingsPageAction::SetCLIAgentForCommand {
                                    pattern: pattern_clone.clone(),
                                    agent: Some(agent),
                                },
                            ));
                        if let Some(icon) = icon {
                            fields = fields.with_icon(icon);
                        }
                        items.push(fields.into_item());
                    }

                    items.push(
                        MenuItemFields::new("Other")
                            .with_on_select_action(DropdownAction::SelectActionAndClose(
                                AISettingsPageAction::SetCLIAgentForCommand {
                                    pattern: pattern_clone.clone(),
                                    agent: None,
                                },
                            ))
                            .into_item(),
                    );

                    dropdown.set_rich_items(items, ctx);

                    dropdown.set_menu_header_text_override(|label| {
                        if label == "Other" {
                            "Select coding agent".to_string()
                        } else {
                            label.to_string()
                        }
                    });

                    let selected_name = if matches!(current_agent, CLIAgent::Unknown) {
                        "Other"
                    } else {
                        current_agent.display_name()
                    };
                    dropdown.set_selected_by_name(selected_name, ctx);

                    dropdown
                })
            })
            .collect()
    }

    // ── Provider form editors ──────────────────────────────────────────────

    fn create_provider_editor(ctx: &mut ViewContext<Self>, placeholder: &'static str, initial: &'static str) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(placeholder, ctx);
            editor.set_buffer_text(initial, ctx);
            editor
        });
        editor
    }

    fn create_provider_id_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "Provider ID", "ollama-local")
    }
    fn create_provider_display_name_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "Display name", "Ollama local")
    }
    fn create_provider_base_url_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "http://127.0.0.1:11434/v1", "http://127.0.0.1:11434/v1")
    }
    fn create_provider_model_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "Model ID", "")
    }
    fn create_provider_env_var_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "Env var name", "")
    }
    fn create_provider_secret_key_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "Secret key", "")
    }
    fn create_provider_defaults_temperature_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "1.0", "")
    }
    fn create_provider_defaults_top_p_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "1.0", "")
    }
    fn create_provider_defaults_max_output_tokens_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "4096", "")
    }
    fn create_provider_defaults_timeout_ms_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        Self::create_provider_editor(ctx, "60000", "")
    }

    fn set_provider_editor(&self, editor: &ViewHandle<EditorView>, text: &str, ctx: &mut ViewContext<Self>) {
        editor.update(ctx, |ed, ctx| ed.set_buffer_text(text, ctx));
    }
}

impl View for AISettingsPageView {
    fn ui_name() -> &'static str {
        "AISettingsPage"
    }

    fn render(&self, app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        self.page.render(self, app)
    }
}

pub enum AISettingsPageEvent {
    FocusModal,
    OpenAIFactCollection,
    OpenMCPServerCollection,
    OpenExecutionProfileEditor(ClientProfileId),
    SignupAnonymousUser,
}

impl Entity for AISettingsPageView {
    type Event = AISettingsPageEvent;
}

#[derive(Debug, Clone, PartialEq)]
pub enum AISettingsPageAction {
    OpenUrl(String),
    SetVoiceInputToggleKey(VoiceInputToggleKey),
    ToggleGlobalAI,
    ToggleActiveAI,
    ToggleIntelligentAutosuggestions,
    TogglePromptSuggestions,
    ToggleCodeSuggestions,
    ToggleNaturalLanguageAutosuggestions,
    ToggleSharedTitleGeneration,
    ToggleGitOperationsAutogen,
    ToggleAIInputAutoDetection,
    ToggleNLDInTerminal,
    ToggleCLIAgentToolbar,
    ToggleUseAgentToolbar,
    ToggleVoiceInput,
    ToggleCanUseWarpCreditsWithByok,
    HyperlinkClick(HyperlinkUrl),
    ToggleCodebaseContext,
    ToggleShowInputHintText,
    ToggleShowAgentTips,
    ToggleShowOzUpdatesInZeroState,
    SetThinkingDisplayMode(ThinkingDisplayMode),
    AttemptLoginGatedUpgrade,
    RemoveCLIAgentToolbarEnabledCommand(String),
    RemoveFromCommandExecutionAllowlist(AgentModeCommandExecutionPredicate),
    RemoveFromCommandExecutionDenylist(AgentModeCommandExecutionPredicate),
    OpenAIFactCollection,
    OpenMCPServerCollection,
    OpenExecutionProfileEditor(ClientProfileId),
    SetBaseModel(LLMId),
    SetCodingModel(LLMId),
    /// Called while the user is actively dragging the context window slider.
    ContextWindowSliderDragged(u32),
    /// Called when the user commits a new context window value (slider drop or
    /// input box commit).
    SetContextWindowSize(u32),
    SetAutonomyReadonlyCommandsSetting,
    SetAutonomySupervisedSetting,
    SetCodingPermission(AgentModeCodingPermissionsType),
    RemoveDirectoryFromCodeReadAllowlist(PathBuf),
    ToggleRules,
    ToggleRuleSuggestions,
    ToggleWarpDriveContext,
    SetApplyCodeDiffs(ActionPermission),
    SetReadFiles(ActionPermission),
    SetExecuteCommands(ActionPermission),
    SetWriteToPty(WriteToPtyPermission),
    SetMCPPermissions(ActionPermission),
    RemoveFromProfileDirectoryAllowlist(PathBuf),
    RemoveFromProfileCommandDenylist(AgentModeCommandExecutionPredicate),
    RemoveFromProfileCommandAllowlist(AgentModeCommandExecutionPredicate),
    ToggleShowBaseModelPickerInPrompt,
    AddToMCPAllowlist(uuid::Uuid),
    RemoveFromMCPAllowlist(uuid::Uuid),
    AddToMCPDenylist(uuid::Uuid),
    RemoveFromMCPDenylist(uuid::Uuid),
    CreateProfile,
    SignupAnonymousUser,
    ToggleAwsBedrockAutoLogin,
    ToggleAwsBedrockCredentialsEnabled,
    RefreshAwsBedrockCredentials,
    ToggleCloudAgentComputerUse,
    ToggleFileBasedMcp,
    ToggleIncludeAgentCommandsInHistory,
    ToggleAgentAttribution,
    #[cfg(feature = "local_fs")]
    SetConversationLayout(crate::util::file::external_editor::settings::OpenConversationPreference),
    ToggleOrchestration,
    ToggleShowConversationHistory,
    ToggleAutoToggleRichInput,
    ToggleAutoOpenRichInputOnCLIAgentStart,
    ToggleAutoDismissRichInputAfterSubmit,
    SetCLIAgentForCommand {
        pattern: String,
        agent: Option<CLIAgent>,
    },
    SetProviderDraftField(ProviderDraftField, String),
    SetProviderDraftKind(ProviderKind),
    SetProviderDraftAuthMode(ProviderDraftAuthMode),
    SetProviderDraftCapability(ProviderDraftCapability, CapabilitySettingDraft),
    SetProviderDraftPolicy(ProviderDraftPolicy, bool),
    SetProviderDraftDefault(ProviderDraftDefaultField, String),
    SaveProviderDraft,
    EditProvider(String),
    CancelProviderEdit,
    AddProvider,
    ToggleProviderEnabled(String),
    DeleteProvider(String),
    DuplicateProvider(String),
    ValidateProvider(String),
    DiscoverProviderModels(String),
    ProviderValidationCompleted {
        provider_id: String,
        result: ValidationResult,
    },
    ProviderDiscoveryCompleted {
        provider_id: String,
        result: Result<Vec<DiscoveredModel>, ValidationError>,
    },
    SetDefaultProvider {
        provider_id: String,
        model_id: String,
    },
    ExportProvider(String),
    ExportAllProviders,
    ShowImportDialog,
    HideImportDialog,
    SetImportJson(String),
    SetImportSecretValue(String),
    ExecuteImport,
    ResolveImportCollision {
        provider_id: String,
        action: ImportCollisionAction,
    },
    SubmitImportSecret {
        provider_id: String,
        secret_value: String,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum ImportCollisionAction {
    Overwrite,
    Rename,
    Skip,
}

impl From<&AISettingsPageAction> for LoginGatedFeature {
    fn from(val: &AISettingsPageAction) -> LoginGatedFeature {
        use AISettingsPageAction::*;
        match val {
            AttemptLoginGatedUpgrade => "Upgrade AI Usage",
            _ => "Unknown reason",
        }
    }
}

impl TypedActionView for AISettingsPageView {
    type Action = AISettingsPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            AISettingsPageAction::OpenUrl(url) => {
                ctx.open_url(url.as_str());
            }
            AISettingsPageAction::SetVoiceInputToggleKey(key) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.voice_input_toggle_key.set_value(*key, ctx));
                    report_if_error!(settings
                        .explicitly_interacted_with_voice
                        .set_value(true, ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleGlobalAI => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.is_any_ai_enabled.toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleGlobalAI {
                                is_ai_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Global AI setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleActiveAI => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .is_active_ai_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleActiveAI {
                                is_active_ai_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Active AI setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleIntelligentAutosuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .intelligent_autosuggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleIntelligentAutosuggestionsSetting {
                                is_intelligent_autosuggestions_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Next Command setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::TogglePromptSuggestions => {
                if !UserWorkspaces::as_ref(ctx).is_prompt_suggestions_toggleable() {
                    return;
                }
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .prompt_suggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::TogglePromptSuggestionsSetting {
                                is_prompt_suggestions_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Prompt Suggestions setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleCodeSuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .code_suggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleCodeSuggestionsSetting {
                                source: ToggleCodeSuggestionsSettingSource::Settings,
                                is_code_suggestions_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Code Suggestions setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleNaturalLanguageAutosuggestions => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .natural_language_autosuggestions_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleNaturalLanguageAutosuggestionsSetting {
                                is_natural_language_autosuggestions_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to set value for Natural Language Autosuggestions setting: {e:?}"
                        );
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleSharedTitleGeneration => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .shared_block_title_generation_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleSharedBlockTitleGenerationSetting {
                                is_shared_block_title_generation_enabled: true,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to set value for Shared Block Title Generation setting: {e:?}"
                        );
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleGitOperationsAutogen => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .git_operations_autogen_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleGitOperationsAutogenSetting {
                                is_git_operations_autogen_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Git Operations Autogen setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleAIInputAutoDetection => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .ai_autodetection_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::AgentModeToggleAutoDetectionSetting {
                                is_autodetection_enabled: new_value,
                                origin: AgentModeAutoDetectionSettingOrigin::SettingsPage
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Input Auto-detection: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleNLDInTerminal => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .nld_in_terminal_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(_new_value) => {}
                    Err(e) => {
                        log::warn!("Failed to set value for NLD in Terminal: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleCLIAgentToolbar => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .should_render_cli_agent_footer
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleCLIAgentToolbarSetting {
                                is_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for CLI Agent Footer setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleAutoToggleRichInput => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.auto_toggle_rich_input.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleAutoOpenRichInputOnCLIAgentStart => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .auto_open_rich_input_on_cli_agent_start
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleAutoDismissRichInputAfterSubmit => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .auto_dismiss_rich_input_after_submit
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleUseAgentToolbar => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .should_render_use_agent_footer_for_user_commands
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleUseAgentToolbarSetting {
                                is_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Use Agent Footer setting: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleCodebaseContext => {
                match CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.codebase_context_enabled.toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleCodebaseContext {
                                is_codebase_context_enabled: new_value
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Codebase Context: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleVoiceInput => {
                match AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings
                        .voice_input_enabled_internal
                        .toggle_and_save_value(ctx)
                }) {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleVoiceInputSetting {
                                is_voice_input_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Voice Input: {e:?}");
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::ToggleCanUseWarpCreditsWithByok => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .can_use_warp_credits_with_byok
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::HyperlinkClick(hyperlink) => {
                ctx.notify();
                ctx.open_url(&hyperlink.url);
            }
            AISettingsPageAction::ToggleShowInputHintText => {
                InputSettings::handle(ctx).update(ctx, |input_settings, ctx| {
                    report_if_error!(input_settings.show_hint_text.toggle_and_save_value(ctx));
                    send_telemetry_from_ctx!(
                        // We purposely keep the FeaturesPageAction event, even though we have moved the setting to AI settings.
                        TelemetryEvent::FeaturesPageAction {
                            action: "ToggleShowInputHintText".to_string(),
                            value: format!("{}", *input_settings.show_hint_text),
                        },
                        ctx
                    );
                });
            }
            AISettingsPageAction::ToggleShowAgentTips => {
                InputSettings::handle(ctx).update(ctx, |input_settings, ctx| match input_settings
                    .show_agent_tips
                    .toggle_and_save_value(ctx)
                {
                    Ok(new_value) => {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::ToggleShowAgentTips {
                                is_enabled: new_value,
                            },
                            ctx
                        );
                    }
                    Err(e) => {
                        log::warn!("Failed to set value for Show Agent Tips setting: {e:?}");
                    }
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleShowOzUpdatesInZeroState => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .should_show_oz_updates_in_zero_state
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::SetThinkingDisplayMode(mode) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.thinking_display_mode.set_value(*mode, ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::AttemptLoginGatedUpgrade => {
                AuthManager::handle(ctx).update(ctx, |auth_manager, ctx| {
                    auth_manager.attempt_login_gated_feature(
                        action.into(),
                        AuthViewVariant::RequireLoginCloseable,
                        ctx,
                    )
                });
            }
            AISettingsPageAction::RemoveCLIAgentToolbarEnabledCommand(command) => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.remove_cli_agent_footer_enabled_command(command, ctx);
                });
            }
            AISettingsPageAction::SetCLIAgentForCommand { pattern, agent } => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    settings.set_cli_agent_for_command(pattern, *agent, ctx);
                });
            }
            AISettingsPageAction::RemoveFromCommandExecutionAllowlist(cmd) => {
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    report_if_error!(model.remove_command_from_autoexecution_allowlist(cmd, ctx));
                })
            }
            AISettingsPageAction::RemoveFromCommandExecutionDenylist(cmd) => {
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    report_if_error!(model.remove_command_from_denylist(cmd, ctx));
                })
            }
            AISettingsPageAction::OpenAIFactCollection => {
                ctx.emit(AISettingsPageEvent::OpenAIFactCollection)
            }
            AISettingsPageAction::OpenMCPServerCollection => {
                ctx.emit(AISettingsPageEvent::OpenMCPServerCollection)
            }
            AISettingsPageAction::OpenExecutionProfileEditor(profile_id) => {
                ctx.emit(AISettingsPageEvent::OpenExecutionProfileEditor(*profile_id))
            }
            AISettingsPageAction::SetBaseModel(id) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |profiles_model, ctx| {
                    let profile_id = *profiles_model.active_profile(None, ctx).id();
                    profiles_model.set_base_model(profile_id, Some(id.clone()), ctx);
                    profiles_model.set_context_window_limit(profile_id, None, ctx);
                });
                self.sync_context_window_editor(ctx, true);
                ctx.notify();
            }
            AISettingsPageAction::SetCodingModel(id) => {
                LLMPreferences::handle(ctx).update(ctx, |prefs, ctx| {
                    prefs.update_preferred_coding_llm(id, None, ctx);
                });
            }
            AISettingsPageAction::ContextWindowSliderDragged(value) => {
                if !AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
                    self.sync_context_window_editor(ctx, true);
                    return;
                }
                if Self::configurable_context_window(ctx).is_some() {
                    let formatted = value.to_string();
                    self.context_window_editor.update(ctx, |editor, ctx| {
                        editor.system_reset_buffer_text(&formatted, ctx);
                    });
                    ctx.notify();
                }
            }
            AISettingsPageAction::SetContextWindowSize(value) => {
                if !AISettings::as_ref(ctx).is_any_ai_enabled(ctx) {
                    self.sync_context_window_editor(ctx, true);
                    return;
                }
                let Some(cw) = Self::configurable_context_window(ctx) else {
                    return;
                };
                let clamped = (*value).clamp(cw.min, cw.max);
                AIExecutionProfilesModel::handle(ctx).update(ctx, |profiles_model, ctx| {
                    let profile_id = *profiles_model.active_profile(None, ctx).id();
                    profiles_model.set_context_window_limit(profile_id, Some(clamped), ctx);
                });
                self.sync_context_window_editor(ctx, true);
                ctx.notify();
            }
            AISettingsPageAction::SetAutonomyReadonlyCommandsSetting
            | AISettingsPageAction::SetAutonomySupervisedSetting => {
                let readonly_cmd_execution_enabled = matches!(
                    action,
                    AISettingsPageAction::SetAutonomyReadonlyCommandsSetting
                );
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    match model.set_should_autoexecute_readonly_commands(
                        readonly_cmd_execution_enabled,
                        ctx,
                    ) {
                        Ok(_) => {
                            send_telemetry_from_ctx!(
                                TelemetryEvent::ToggledAgentModeAutoexecuteReadonlyCommandsSetting {
                                    src: AutonomySettingToggleSource::SettingsPage,
                                    enabled: readonly_cmd_execution_enabled,
                                },
                                ctx);
                        }
                        Err(e) => report_error!(e),
                    }
                });
            }
            AISettingsPageAction::SetCodingPermission(p) => {
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    match model.set_coding_permissions(*p, ctx) {
                        Ok(_) => {
                            send_telemetry_from_ctx!(
                                TelemetryEvent::ChangedAgentModeCodingPermissions {
                                    src: AutonomySettingToggleSource::SettingsPage,
                                    new: *p,
                                },
                                ctx
                            );
                        }
                        Err(e) => report_error!(e),
                    }
                });
            }
            AISettingsPageAction::SetApplyCodeDiffs(permission) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    model.set_apply_code_diffs(*profile.id(), permission, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::SetReadFiles(permission) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    model.set_read_files(*profile.id(), permission, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::SetExecuteCommands(permission) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    model.set_execute_commands(*profile.id(), permission, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::SetWriteToPty(permission) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    model.set_write_to_pty(*profile.id(), permission, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::SetMCPPermissions(permission) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    model.set_mcp_permissions(*profile.id(), permission, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::RemoveDirectoryFromCodeReadAllowlist(dir) => {
                BlocklistAIPermissions::handle(ctx).update(ctx, |model, ctx| {
                    report_if_error!(
                        model.remove_filepath_from_code_read_allowlist(dir.to_owned(), ctx)
                    );
                });
            }
            AISettingsPageAction::ToggleRules => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let _ = settings.memory_enabled.toggle_and_save_value(ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleRuleSuggestions => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let _ = settings
                        .rule_suggestions_enabled_internal
                        .toggle_and_save_value(ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleWarpDriveContext => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let _ = settings
                        .warp_drive_context_enabled
                        .toggle_and_save_value(ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::RemoveFromProfileDirectoryAllowlist(path_buf) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.remove_from_directory_allowlist(
                        *profile_id,
                        &PathBuf::from(path_buf),
                        ctx,
                    );
                });
                ctx.notify();
            }
            AISettingsPageAction::RemoveFromProfileCommandDenylist(cmd) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();

                    model.remove_from_command_denylist(*profile_id, cmd, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::RemoveFromProfileCommandAllowlist(command) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();

                    model.remove_from_command_allowlist(*profile_id, command, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleShowBaseModelPickerInPrompt => {
                SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
                    if let Err(e) = settings
                        .show_model_selectors_in_prompt
                        .toggle_and_save_value(ctx)
                    {
                        log::warn!(
                            "Failed to set value for Show Base Model Picker in Prompt: {e:?}"
                        );
                    }
                });
                ctx.notify();
            }
            AISettingsPageAction::AddToMCPAllowlist(id) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.add_to_mcp_allowlist(*profile_id, id, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::RemoveFromMCPAllowlist(id) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.remove_from_mcp_allowlist(*profile_id, id, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::AddToMCPDenylist(id) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.add_to_mcp_denylist(*profile_id, id, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::RemoveFromMCPDenylist(id) => {
                AIExecutionProfilesModel::handle(ctx).update(ctx, |model, ctx| {
                    let profile = model.default_profile(ctx);
                    let profile_id = profile.id();
                    model.remove_from_mcp_denylist(*profile_id, id, ctx);
                });
                ctx.notify();
            }
            AISettingsPageAction::CreateProfile => {
                let new_profile_id = AIExecutionProfilesModel::handle(ctx)
                    .update(ctx, |model, ctx| model.create_profile(ctx));

                if let Some(profile_id) = new_profile_id {
                    self.profile_views = Self::create_profile_views(ctx);
                    ctx.emit(AISettingsPageEvent::OpenExecutionProfileEditor(profile_id));
                }
                ctx.notify();
            }
            AISettingsPageAction::SignupAnonymousUser => {
                ctx.emit(AISettingsPageEvent::SignupAnonymousUser);
            }
            AISettingsPageAction::ToggleAwsBedrockAutoLogin => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.aws_bedrock_auto_login.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleAwsBedrockCredentialsEnabled => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .aws_bedrock_credentials_enabled
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::RefreshAwsBedrockCredentials => {
                #[cfg(not(target_family = "wasm"))]
                ApiKeyManager::handle(ctx).update(ctx, |manager, ctx| {
                    drop(refresh_aws_credentials(manager, ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleCloudAgentComputerUse => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .cloud_agent_computer_use_enabled
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleFileBasedMcp => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.file_based_mcp_enabled.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleIncludeAgentCommandsInHistory => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .include_agent_commands_in_history
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            #[cfg(feature = "local_fs")]
            AISettingsPageAction::SetConversationLayout(layout) => {
                crate::util::file::external_editor::EditorSettings::handle(ctx).update(
                    ctx,
                    |settings, ctx| {
                        report_if_error!(settings
                            .open_conversation_layout_preference
                            .set_value(*layout, ctx));
                    },
                );
                send_telemetry_from_ctx!(
                    TelemetryEvent::FeaturesPageAction {
                        action: "SetConversationLayout".to_string(),
                        value: format!("{layout:?}")
                    },
                    ctx
                );
                ctx.notify();
            }
            AISettingsPageAction::ToggleOrchestration => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.orchestration_enabled.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleShowConversationHistory => {
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .show_conversation_history
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::ToggleAgentAttribution => {
                // The updated value syncs to warp-server automatically via
                // `CloudPreferencesSyncer` as a `JsonPreference` GSO keyed
                // `Global_AgentAttributionEnabled`; no bespoke server call needed.
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings
                        .agent_attribution_enabled
                        .toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            AISettingsPageAction::SetProviderDraftField(field, value) => {
                match field {
                    ProviderDraftField::Id => self.provider_draft.id = provider_id_from_text(value),
                    ProviderDraftField::DisplayName => {
                        self.provider_draft.display_name = value.trim().to_string();
                    }
                    ProviderDraftField::BaseUrl => {
                        self.provider_draft.base_url = value.trim().to_string();
                    }
                    ProviderDraftField::Model => {
                        self.provider_draft.model = value.trim().to_string();
                    }
                    ProviderDraftField::EnvVar => {
                        self.provider_draft.env_var = value.trim().to_string();
                    }
                    ProviderDraftField::SecretKey => {
                        self.provider_draft.secret_key = value.trim().to_string();
                    }
                    ProviderDraftField::Temperature => {
                        self.provider_draft.temperature = value.trim().to_string();
                    }
                    ProviderDraftField::TopP => {
                        self.provider_draft.top_p = value.trim().to_string();
                    }
                    ProviderDraftField::MaxOutputTokens => {
                        self.provider_draft.max_output_tokens = value.trim().to_string();
                    }
                    ProviderDraftField::TimeoutMs => {
                        self.provider_draft.timeout_ms = value.trim().to_string();
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::SetProviderDraftKind(kind) => {
                self.provider_draft.kind = kind.clone();
                if self.provider_draft.base_url.is_empty() {
                    self.provider_draft.base_url = default_base_url_for_kind(kind).to_string();
                }
                ctx.notify();
            }
            AISettingsPageAction::SetProviderDraftAuthMode(mode) => {
                self.provider_draft.auth_mode = mode.clone();
                ctx.notify();
            }
            AISettingsPageAction::SetProviderDraftCapability(cap, value) => {
                match cap {
                    ProviderDraftCapability::Tools => self.provider_draft.cap_tools = value.clone(),
                    ProviderDraftCapability::Vision => {
                        self.provider_draft.cap_vision = value.clone();
                    }
                    ProviderDraftCapability::StructuredOutputs => {
                        self.provider_draft.cap_structured_outputs = value.clone();
                    }
                    ProviderDraftCapability::PromptCaching => {
                        self.provider_draft.cap_prompt_caching = value.clone();
                    }
                    ProviderDraftCapability::ReasoningControls => {
                        self.provider_draft.cap_reasoning_controls = value.clone();
                    }
                    ProviderDraftCapability::ModelDiscovery => {
                        self.provider_draft.cap_model_discovery = value.clone();
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::SetProviderDraftPolicy(policy, value) => {
                match policy {
                    ProviderDraftPolicy::AllowFallback => {
                        self.provider_draft.allow_fallback = *value;
                    }
                    ProviderDraftPolicy::RedactTelemetry => {
                        self.provider_draft.redact_telemetry = *value;
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::SetProviderDraftDefault(field, value) => {
                match field {
                    ProviderDraftDefaultField::Stream => {
                        self.provider_draft.stream = value == "true";
                    }
                    ProviderDraftDefaultField::Temperature => {
                        self.provider_draft.temperature = value.trim().to_string();
                    }
                    ProviderDraftDefaultField::TopP => {
                        self.provider_draft.top_p = value.trim().to_string();
                    }
                    ProviderDraftDefaultField::MaxOutputTokens => {
                        self.provider_draft.max_output_tokens = value.trim().to_string();
                    }
                    ProviderDraftDefaultField::TimeoutMs => {
                        self.provider_draft.timeout_ms = value.trim().to_string();
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::SaveProviderDraft => {
                // Read live buffer text from editors so we always save what the user sees.
                let mut draft = self.provider_draft.clone();
                draft.id = self.provider_id_editor.as_ref(ctx).buffer_text(ctx);
                draft.display_name = self.provider_display_name_editor.as_ref(ctx).buffer_text(ctx);
                draft.base_url = self.provider_base_url_editor.as_ref(ctx).buffer_text(ctx);
                draft.model = self.provider_model_editor.as_ref(ctx).buffer_text(ctx);
                draft.env_var = self.provider_env_var_editor.as_ref(ctx).buffer_text(ctx);
                draft.secret_key = self.provider_secret_key_editor.as_ref(ctx).buffer_text(ctx);
                draft.temperature = self.provider_defaults_temperature_editor.as_ref(ctx).buffer_text(ctx);
                draft.top_p = self.provider_defaults_top_p_editor.as_ref(ctx).buffer_text(ctx);
                draft.max_output_tokens = self.provider_defaults_max_output_tokens_editor.as_ref(ctx).buffer_text(ctx);
                draft.timeout_ms = self.provider_defaults_timeout_ms_editor.as_ref(ctx).buffer_text(ctx);

                let api_keys = ApiKeyManager::as_ref(ctx).keys().clone();
                let registry = AISettings::as_ref(ctx).provider_registry.value().clone();
                let window_id = ctx.window_id();
                let Some(new_registry) =
                    provider_registry_with_draft(&draft, &api_keys, &registry, ctx)
                else {
                    crate::ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                        toast_stack.add_ephemeral_toast(
                            crate::view_components::DismissibleToast::error(
                                "Could not save provider: check required fields (Provider ID, Base URL)".to_string(),
                            ),
                            window_id,
                            ctx,
                        );
                    });
                    return;
                };
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    if let Err(e) = settings.provider_registry.set_value(new_registry, ctx) {
                        log::error!("Failed to save provider registry: {e:#}");
                        crate::ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                            toast_stack.add_ephemeral_toast(
                                crate::view_components::DismissibleToast::error(format!(
                                    "Failed to save provider: {e}"
                                )),
                                window_id,
                                ctx,
                            );
                        });
                    }
                });
                // Reset to create mode after save
                self.provider_draft = ProviderDraft::default();
                self.set_provider_editor(&self.provider_id_editor, "ollama-local", ctx);
                self.set_provider_editor(&self.provider_display_name_editor, "Ollama local", ctx);
                self.set_provider_editor(&self.provider_base_url_editor, "http://127.0.0.1:11434/v1", ctx);
                self.set_provider_editor(&self.provider_model_editor, "", ctx);
                self.set_provider_editor(&self.provider_env_var_editor, "", ctx);
                self.set_provider_editor(&self.provider_secret_key_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_temperature_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_top_p_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_max_output_tokens_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_timeout_ms_editor, "", ctx);
                crate::ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    toast_stack.add_ephemeral_toast(
                        crate::view_components::DismissibleToast::success(
                            "Provider saved successfully".to_string(),
                        ),
                        window_id,
                        ctx,
                    );
                });
                ctx.notify();
            }
            AISettingsPageAction::EditProvider(provider_id) => {
                let Some(profile) = provider_profile_for_action(provider_id, ctx) else {
                    return;
                };
                self.provider_draft = ProviderDraft::from_profile(&profile);
                self.set_provider_editor(&self.provider_id_editor, &profile.id, ctx);
                self.set_provider_editor(&self.provider_display_name_editor, &profile.display_name, ctx);
                self.set_provider_editor(&self.provider_base_url_editor, &profile.base_url, ctx);
                let model = profile.discovery.fallback_models.first().cloned().unwrap_or_default();
                self.set_provider_editor(&self.provider_model_editor, &model, ctx);
                self.set_provider_editor(&self.provider_env_var_editor, "", ctx);
                self.set_provider_editor(&self.provider_secret_key_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_temperature_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_top_p_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_max_output_tokens_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_timeout_ms_editor, "", ctx);
                ctx.notify();
            }
            AISettingsPageAction::CancelProviderEdit => {
                self.provider_draft = ProviderDraft::default();
                self.set_provider_editor(&self.provider_id_editor, "ollama-local", ctx);
                self.set_provider_editor(&self.provider_display_name_editor, "Ollama local", ctx);
                self.set_provider_editor(&self.provider_base_url_editor, "http://127.0.0.1:11434/v1", ctx);
                self.set_provider_editor(&self.provider_model_editor, "", ctx);
                self.set_provider_editor(&self.provider_env_var_editor, "", ctx);
                self.set_provider_editor(&self.provider_secret_key_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_temperature_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_top_p_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_max_output_tokens_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_timeout_ms_editor, "", ctx);
                ctx.notify();
            }
            AISettingsPageAction::AddProvider => {
                self.provider_draft = ProviderDraft {
                    editing_provider_id: None,
                    ..ProviderDraft::default()
                };
                self.set_provider_editor(&self.provider_id_editor, "ollama-local", ctx);
                self.set_provider_editor(&self.provider_display_name_editor, "Ollama local", ctx);
                self.set_provider_editor(&self.provider_base_url_editor, "http://127.0.0.1:11434/v1", ctx);
                self.set_provider_editor(&self.provider_model_editor, "", ctx);
                self.set_provider_editor(&self.provider_env_var_editor, "", ctx);
                self.set_provider_editor(&self.provider_secret_key_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_temperature_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_top_p_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_max_output_tokens_editor, "", ctx);
                self.set_provider_editor(&self.provider_defaults_timeout_ms_editor, "", ctx);
                ctx.notify();
            }
            AISettingsPageAction::ToggleProviderEnabled(provider_id) => {
                update_provider_registry(ctx, |registry| {
                    if let Some(provider) = registry.providers.get_mut(provider_id) {
                        if provider.id != WARP_HOSTED_PROVIDER_ID {
                            provider.enabled = !provider.enabled;
                        }
                    }
                });
                ctx.notify();
            }
            AISettingsPageAction::DeleteProvider(provider_id) => {
                update_provider_registry(ctx, |registry| {
                    if provider_id != WARP_HOSTED_PROVIDER_ID {
                        registry.providers.remove(provider_id);
                        if registry.defaults.agent_provider.as_deref() == Some(provider_id) {
                            registry.defaults.agent_provider =
                                Some(WARP_HOSTED_PROVIDER_ID.to_string());
                            registry.defaults.agent_model = Some("auto".to_string());
                        }
                    }
                });
                ctx.notify();
            }
            AISettingsPageAction::DuplicateProvider(provider_id) => {
                update_provider_registry(ctx, |registry| {
                    let Some(provider) = registry.providers.get(provider_id).cloned() else {
                        return;
                    };
                    let mut copy = provider.clone();
                    copy.id =
                        unique_provider_id(&registry.providers, &format!("{}-copy", provider.id));
                    copy.display_name = format!("{} Copy", provider.display_name);
                    copy.enabled = false;
                    registry.upsert_provider(copy);
                });
                ctx.notify();
            }
            AISettingsPageAction::ValidateProvider(provider_id) => {
                self.start_provider_validation(provider_id.clone(), ctx);
            }
            AISettingsPageAction::DiscoverProviderModels(provider_id) => {
                self.start_provider_discovery(provider_id.clone(), ctx);
            }
            AISettingsPageAction::ProviderValidationCompleted {
                provider_id,
                result,
            } => {
                self.provider_validation_states.insert(
                    provider_id.clone(),
                    ProviderValidationUiState::ValidationComplete(result.clone()),
                );
                update_provider_registry(ctx, |registry| {
                    registry.apply_validation_result(provider_id, result);
                });
                ctx.notify();
            }
            AISettingsPageAction::ProviderDiscoveryCompleted {
                provider_id,
                result,
            } => {
                match result {
                    Ok(models) => {
                        self.provider_validation_states.insert(
                            provider_id.clone(),
                            ProviderValidationUiState::DiscoveryComplete(models.clone()),
                        );
                        update_provider_registry(ctx, |registry| {
                            registry.apply_discovered_models(provider_id, models.clone());
                        });
                    }
                    Err(error) => {
                        self.provider_validation_states.insert(
                            provider_id.clone(),
                            ProviderValidationUiState::DiscoveryFailed(error.clone()),
                        );
                    }
                }
                ctx.notify();
            }
            AISettingsPageAction::SetDefaultProvider {
                provider_id,
                model_id,
            } => {
                update_provider_registry(ctx, |registry| {
                    registry.defaults = ProviderRegistryDefaults {
                        agent_provider: Some(provider_id.clone()),
                        agent_model: Some(model_id.clone()),
                    };
                });
                ctx.notify();
            }
            AISettingsPageAction::ExportProvider(provider_id) => {
                let Some(provider) = provider_profile_for_action(provider_id, ctx) else {
                    return;
                };
                let template =
                    ProviderExportTemplate::new(vec![ExportedProviderProfile::from(&provider)]);
                if let Ok(json) = template.to_json() {
                    ctx.clipboard().write(ClipboardContent::plain_text(json));
                }
            }
            AISettingsPageAction::ExportAllProviders => {
                let registry = ApiKeyManager::as_ref(ctx).provider_registry_from_legacy_keys(Some(
                    AISettings::as_ref(ctx).provider_registry.value().clone(),
                ));
                let providers = registry
                    .providers
                    .values()
                    .filter(|provider| provider.id != WARP_HOSTED_PROVIDER_ID)
                    .map(ExportedProviderProfile::from)
                    .collect();
                let template = ProviderExportTemplate::new(providers);
                if let Ok(json) = template.to_json() {
                    ctx.clipboard().write(ClipboardContent::plain_text(json));
                }
            }
            AISettingsPageAction::ShowImportDialog => {
                self.import_dialog_visible = true;
                ctx.notify();
            }
            AISettingsPageAction::HideImportDialog => {
                self.import_dialog_visible = false;
                self.import_json_text.clear();
                self.import_pending_template = None;
                self.import_collision = None;
                self.import_secret_prompt = None;
                self.import_secret_value.clear();
                ctx.notify();
            }
            AISettingsPageAction::SetImportJson(json) => {
                self.import_json_text = json.clone();
                ctx.notify();
            }
            AISettingsPageAction::SetImportSecretValue(value) => {
                self.import_secret_value = value.clone();
                ctx.notify();
            }
            AISettingsPageAction::ExecuteImport => {
                let import_json = self.import_json_text.clone();
                let Ok(template) = ProviderExportTemplate::from_json(&import_json) else {
                    return;
                };
                self.process_provider_import_template(template, None, ctx);
            }
            AISettingsPageAction::ResolveImportCollision {
                provider_id,
                action,
            } => {
                let Some(template) = self.import_pending_template.clone() else {
                    return;
                };
                self.process_provider_import_template(
                    template,
                    Some((provider_id.clone(), action.clone())),
                    ctx,
                );
            }
            AISettingsPageAction::SubmitImportSecret {
                provider_id,
                secret_value,
            } => {
                let value = if secret_value.trim().is_empty() {
                    self.import_secret_value.trim().to_string()
                } else {
                    secret_value.trim().to_string()
                };
                if value.is_empty() {
                    return;
                }
                let Some(template) = self.import_pending_template.clone() else {
                    return;
                };
                self.import_provider_secret(&template, provider_id, &value, ctx);
                ctx.notify();
            }
        }
    }
}

impl SettingsPageMeta for AISettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::AI
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        FeatureFlag::AgentMode.is_enabled()
    }

    fn on_page_selected(&mut self, _: bool, ctx: &mut ViewContext<Self>) {
        AIRequestUsageModel::handle(ctx).update(ctx, |ai_request_usage_model, ctx| {
            ai_request_usage_model.refresh_request_usage_async(ctx)
        });
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<AISettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<AISettingsPageView>) -> Self {
        SettingsPageViewHandle::AI(view_handle)
    }
}

fn render_ai_setting_toggle<S: Setting>(
    label: impl Into<String>,
    action: AISettingsPageAction,
    is_setting_enabled: bool,
    is_setting_toggleable: bool,
    switch_state: SwitchStateHandle,
    tooltip_states: &RefCell<HashMap<String, MouseStateHandle>>,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    build_toggle_element(
        render_body_item_label::<AISettingsPageAction>(
            label.into(),
            Some(styles::header_font_color(is_setting_toggleable, app)),
            None,
            LocalOnlyIconState::for_setting(
                S::storage_key(),
                S::sync_to_cloud(),
                &mut tooltip_states.borrow_mut(),
                app,
            ),
            ToggleState::Enabled,
            appearance,
        ),
        render_ai_feature_switch(
            switch_state,
            is_setting_enabled,
            is_setting_toggleable,
            action,
            app,
        ),
        appearance,
        None,
    )
}

fn render_ai_setting_label<S: Setting>(
    label: impl Into<String>,
    is_setting_toggleable: bool,
    tooltip_states: &RefCell<HashMap<String, MouseStateHandle>>,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    Container::new(render_body_item_label::<AISettingsPageAction>(
        label.into(),
        Some(styles::header_font_color(is_setting_toggleable, app)),
        None,
        LocalOnlyIconState::for_setting(
            S::storage_key(),
            S::sync_to_cloud(),
            &mut tooltip_states.borrow_mut(),
            app,
        ),
        ToggleState::Enabled,
        appearance,
    ))
    .with_margin_bottom(HEADER_PADDING)
    .finish()
}

fn render_ai_setting_description(
    description: impl Into<Cow<'static, str>>,
    is_setting_toggleable: bool,
    app: &AppContext,
) -> Box<dyn Element> {
    let default_font_size = Appearance::as_ref(app).ui_font_size();
    render_ai_setting_description_with_font_size(
        description,
        default_font_size,
        is_setting_toggleable,
        app,
    )
}

fn render_ai_setting_description_with_font_size(
    description: impl Into<Cow<'static, str>>,
    font_size: f32,
    is_setting_toggleable: bool,
    app: &AppContext,
) -> Box<dyn Element> {
    let ui_builder = Appearance::as_ref(app).ui_builder();
    ui_builder
        .paragraph(description)
        .with_style(UiComponentStyles {
            font_size: Some(font_size),
            font_color: Some(styles::description_font_color(is_setting_toggleable, app).into()),
            margin: Some(
                Coords::default()
                    .top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                    .bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                    .right(styles::TOGGLE_WIDTH_MARGIN),
            ),
            ..Default::default()
        })
        .build()
        .finish()
}

fn render_toolbar_layout_editor(
    editor: &ViewHandle<AgentToolbarInlineEditor>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let label = Container::new(
        appearance
            .ui_builder()
            .span("Toolbar layout".to_string())
            .with_style(UiComponentStyles {
                font_size: Some(CONTENT_FONT_SIZE),
                ..Default::default()
            })
            .build()
            .finish(),
    )
    .with_margin_bottom(4.)
    .finish();
    let editor = Container::new(ChildView::new(editor).finish())
        .with_margin_bottom(16.)
        .finish();

    Flex::column().with_child(label).with_child(editor).finish()
}

fn render_ai_feature_switch(
    state_handle: SwitchStateHandle,
    is_setting_enabled: bool,
    is_setting_toggleable: bool,
    toggle_action: AISettingsPageAction,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let ui_builder = appearance.ui_builder();
    ui_builder
        .switch(state_handle)
        .check(is_setting_enabled)
        .with_disabled(!is_setting_toggleable)
        .with_disabled_styles(UiComponentStyles {
            background: Some(Fill::Solid(internal_colors::neutral_4(appearance.theme()))),
            foreground: Some(Fill::Solid(internal_colors::neutral_5(appearance.theme()))),
            ..Default::default()
        })
        .build()
        .on_click(move |ctx, _, _| {
            if !is_setting_toggleable {
                return;
            }
            ctx.dispatch_typed_action(toggle_action.clone());
        })
        .finish()
}

fn render_ai_list(
    header: &str,
    description: &str,
    input_list: Box<dyn Element>,
    view: &AISettingsPageView,
    ai_settings: &AISettings,
    app: &AppContext,
) -> Box<dyn Element> {
    let setting_header = render_ai_setting_label::<AgentModeCommandExecutionDenylist>(
        header.to_string(),
        ai_settings.is_any_ai_enabled(app),
        &view.local_only_icon_tooltip_states,
        app,
    );

    let description = render_ai_setting_description(
        description.to_string(),
        ai_settings.is_any_ai_enabled(app),
        app,
    );

    Flex::column()
        .with_child(setting_header)
        .with_child(Container::new(description).with_margin_bottom(-8.).finish())
        .with_child(input_list)
        .finish()
}

#[derive(Default)]
struct GlobalAIWidget {
    switch_state: SwitchStateHandle,
    sign_up_button: MouseStateHandle,
}

impl SettingsWidget for GlobalAIWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "oz warp agent global ai a.i. active next command prompt code diffs suggestion suggested suggestions \
                agent mode natural language detection input hint api keys bring your own byo google anthropic openai"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder();
        let is_ai_disabled_due_to_remote_session_org_policy =
            AISettings::as_ref(app).is_ai_disabled_due_to_remote_session_org_policy(app);

        let is_anonymous = AuthStateProvider::as_ref(app)
            .get()
            .is_anonymous_or_logged_out();

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new_inline(
                    "Warp Agent",
                    appearance.ui_font_family(),
                    PRIMARY_HEADER_FONT_SIZE,
                )
                .with_style(Properties::default().weight(Weight::Bold))
                .with_color(appearance.theme().active_ui_text_color().into())
                .finish(),
            );

        if is_ai_disabled_due_to_remote_session_org_policy {
            row.add_child(
                ConstrainedBox::new(
                    Container::new(
                        Text::new("Your organization disallows AI when the active pane contains content from a remote session", appearance.ui_font_family(), 12.)
                            .with_color(appearance.theme().ui_warning_color())
                            .finish()
                    )
                    .with_padding_left(8.)
                    .with_padding_right(8.)
                    .finish()
                )
                .with_max_width(400.)
                .finish()
            );
        }

        // Show sign-up button for anonymous users, toggle for logged-in users
        if is_anonymous {
            row.add_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Container::new(
                            Text::new_inline(
                                "To use AI features, please create an account.",
                                appearance.ui_font_family(),
                                14.,
                            )
                            .with_color(
                                appearance
                                    .theme()
                                    .sub_text_color(appearance.theme().surface_2())
                                    .into_solid(),
                            )
                            .finish(),
                        )
                        .with_margin_right(16.)
                        .finish(),
                    )
                    .with_child(
                        Container::new(
                            ui_builder
                                .button(ButtonVariant::Accent, self.sign_up_button.clone())
                                .with_style(UiComponentStyles {
                                    font_size: Some(14.),
                                    font_weight: Some(Weight::Semibold),
                                    border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
                                    padding: Some(Coords {
                                        top: 8.,
                                        bottom: 8.,
                                        left: 24.,
                                        right: 24.,
                                    }),
                                    ..Default::default()
                                })
                                .with_text_label("Sign up".to_owned())
                                .build()
                                .on_click(move |ctx, _, _| {
                                    ctx.dispatch_typed_action(
                                        AISettingsPageAction::SignupAnonymousUser,
                                    );
                                })
                                .finish(),
                        )
                        .with_padding_right(TOGGLE_BUTTON_RIGHT_PADDING)
                        .finish(),
                    )
                    .finish(),
            );
        } else {
            row.add_child(
                Container::new(
                    ui_builder
                        .switch(self.switch_state.clone())
                        .check(AISettings::as_ref(app).is_any_ai_enabled(app))
                        .build()
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(AISettingsPageAction::ToggleGlobalAI);
                        })
                        .finish(),
                )
                .with_padding_right(TOGGLE_BUTTON_RIGHT_PADDING)
                .finish(),
            );
        }

        Container::new(row.finish())
            .with_padding_bottom(15.)
            .finish()
    }
}

#[derive(Default)]
struct UsageWidget {
    requests_highlight_index: HighlightedHyperlink,
}

impl UsageWidget {
    fn render_request_usage_count(
        &self,
        used: usize,
        limit: usize,
        is_unlimited: bool,
        workspace_is_delinquent_due_to_payment_issue: bool,
        appearance: &Appearance,
    ) -> Box<dyn warpui::Element> {
        let mut row = Flex::row();
        if used >= limit || workspace_is_delinquent_due_to_payment_issue {
            row.add_child(
                ConstrainedBox::new(
                    Icon::AlertTriangle
                        .to_warpui_icon(appearance.theme().ui_error_color().into())
                        .finish(),
                )
                .with_height(16.)
                .with_width(16.)
                .finish(),
            )
        }

        let request_count_label = if workspace_is_delinquent_due_to_payment_issue {
            "Restricted due to billing issue".to_string()
        } else if is_unlimited {
            "Unlimited".to_string()
        } else {
            format!("{used}/{limit}")
        };

        row.add_child(
            appearance
                .ui_builder()
                .paragraph(request_count_label)
                .with_style(UiComponentStyles {
                    font_color: {
                        if used >= limit {
                            Some(appearance.theme().ui_error_color())
                        } else {
                            Some(blended_colors::text_sub(
                                appearance.theme(),
                                appearance.theme().surface_1(),
                            ))
                        }
                    },
                    font_size: Some(16.),
                    margin: Some(Coords {
                        top: 0.,
                        bottom: 0.,
                        left: 8.,
                        right: 0.,
                    }),
                    ..Default::default()
                })
                .build()
                .finish(),
        );

        row.finish()
    }

    /// Renders a row of what is being limited, along with the current used/limit.
    #[allow(clippy::too_many_arguments)]
    fn render_ai_usage_limit_row(
        &self,
        header: impl Into<Cow<'static, str>>,
        description: impl Into<Cow<'static, str>>,
        used: usize,
        limit: usize,
        is_unlimited: bool,
        workspace_is_delinquent_due_to_payment_issue: bool,
        appearance: &Appearance,
    ) -> Box<dyn warpui::Element> {
        let request_usage_details = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            .with_child(self.render_request_usage_count(
                used,
                limit,
                is_unlimited,
                workspace_is_delinquent_due_to_payment_issue,
                appearance,
            ));

        let request_usage_description = FormattedTextElement::from_str(
            description,
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(blended_colors::text_sub(
            appearance.theme(),
            appearance.theme().surface_1(),
        ));

        Flex::row()
            .with_child(
                Shrinkable::new(
                    2.,
                    Container::new(
                        Flex::column()
                            .with_child(
                                appearance
                                    .ui_builder()
                                    .paragraph(header)
                                    .with_style(UiComponentStyles {
                                        font_color: Some(blended_colors::text_main(
                                            appearance.theme(),
                                            appearance.theme().surface_1(),
                                        )),
                                        margin: Some(Coords {
                                            top: 0.,
                                            bottom: 4.,
                                            left: 0.,
                                            right: 0.,
                                        }),
                                        ..Default::default()
                                    })
                                    .build()
                                    .finish(),
                            )
                            .with_child(request_usage_description.finish())
                            .finish(),
                    )
                    .with_margin_bottom(16.)
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                Shrinkable::new(
                    1.,
                    Container::new(request_usage_details.finish())
                        .with_margin_bottom(16.)
                        .finish(),
                )
                .finish(),
            )
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_main_axis_size(MainAxisSize::Max)
            .finish()
    }
}

impl SettingsWidget for UsageWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "a.i. ai usage limit plan"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_request_usage_model = AIRequestUsageModel::as_ref(app);
        let next_refresh_time = ai_request_usage_model.next_refresh_time();
        let formatted_next_refresh_time = next_refresh_time.format("%b %d").to_string();
        let workspace_is_delinquent_due_to_payment_issue = UserWorkspaces::as_ref(app)
            .current_team()
            .map(|team| team.billing_metadata.is_delinquent_due_to_payment_issue())
            .unwrap_or_default();

        let usage_header = Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    build_sub_header(
                        appearance,
                        "Usage",
                        Some(styles::header_font_color(true, app)),
                    )
                    .finish(),
                )
                .with_child(
                    appearance
                        .ui_builder()
                        .paragraph(format!("Resets {formatted_next_refresh_time}"))
                        .with_style(UiComponentStyles {
                            font_color: Some(blended_colors::text_sub(
                                appearance.theme(),
                                appearance.theme().surface_1(),
                            )),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                )
                .finish(),
        )
        .with_padding_bottom(HEADER_PADDING)
        .finish();

        let request_limit_description = format!(
            "This is the {} limit of AI credits for your account.",
            ai_request_usage_model.refresh_duration_to_string()
        );

        let request_usage_row = self.render_ai_usage_limit_row(
            "Credits",
            request_limit_description,
            ai_request_usage_model.requests_used(),
            ai_request_usage_model.request_limit(),
            ai_request_usage_model.is_unlimited(),
            workspace_is_delinquent_due_to_payment_issue,
            appearance,
        );

        let auth_state = AuthStateProvider::as_ref(app).get();
        let upgrade_cta_text_fragments = if let Some(team) =
            UserWorkspaces::as_ref(app).current_team()
        {
            let current_user_email = auth_state.user_email().unwrap_or_default();
            let has_admin_permissions = team.has_admin_permissions(&current_user_email);
            if team.billing_metadata.can_upgrade_to_higher_tier_plan() {
                let upgrade_url = UserWorkspaces::upgrade_link_for_team(team.uid);
                if has_admin_permissions {
                    vec![
                        FormattedTextFragment::hyperlink("Upgrade", upgrade_url),
                        FormattedTextFragment::plain_text(" to get more AI usage."),
                    ]
                } else {
                    // The /upgrade page says to contact their administrator.
                    vec![
                        FormattedTextFragment::hyperlink("Compare plans", upgrade_url),
                        FormattedTextFragment::plain_text(" for more AI usage."),
                    ]
                }
            } else {
                vec![
                    FormattedTextFragment::hyperlink("Contact support", "mailto:support@warp.dev"),
                    FormattedTextFragment::plain_text(" for more AI usage."),
                ]
            }
        } else {
            let user_id = auth_state.user_id().unwrap_or_default();
            let upgrade_url = UserWorkspaces::upgrade_link(user_id);
            vec![
                FormattedTextFragment::hyperlink("Upgrade", upgrade_url),
                FormattedTextFragment::plain_text(" to get more AI usage."),
            ]
        };

        let mut upgrade_cta = FormattedTextElement::new(
            FormattedText::new([FormattedTextLine::Line(upgrade_cta_text_fragments)]),
            appearance.ui_font_size(),
            appearance.ui_font_family(),
            appearance.ui_font_family(),
            blended_colors::text_sub(appearance.theme(), appearance.theme().surface_1()),
            self.requests_highlight_index.clone(),
        )
        .with_hyperlink_font_color(appearance.theme().accent().into_solid());

        if AuthStateProvider::as_ref(app)
            .get()
            .is_anonymous_or_logged_out()
        {
            upgrade_cta = upgrade_cta.register_default_click_handlers(|_, ctx, _| {
                ctx.dispatch_typed_action(AISettingsPageAction::AttemptLoginGatedUpgrade);
            });
        } else {
            upgrade_cta = upgrade_cta.register_default_click_handlers(|url, ctx, _| {
                ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
            })
        }

        Flex::column()
            .with_children([
                render_separator(appearance),
                usage_header,
                request_usage_row,
                Container::new(upgrade_cta.finish())
                    .with_margin_bottom(16.)
                    .finish(),
            ])
            .finish()
    }
}

#[derive(Default)]
struct ActiveAIWidget {
    active_ai_toggle: SwitchStateHandle,
    intelligent_autosuggestions_toggle: SwitchStateHandle,
    prompt_suggestions_toggle: SwitchStateHandle,
    code_suggestions_toggle: SwitchStateHandle,
    natural_language_autosuggestions_toggle: SwitchStateHandle,
    shared_block_title_generation_toggle: SwitchStateHandle,
    git_operations_autogen_toggle: SwitchStateHandle,
}

impl ActiveAIWidget {
    fn is_next_command_toggleable(&self, app: &AppContext) -> bool {
        UserWorkspaces::as_ref(app).is_next_command_enabled()
            && AISettings::as_ref(app)
                .intelligent_autosuggestions_enabled_internal
                .is_supported_on_current_platform()
    }

    fn is_prompt_suggestions_toggleable(&self, app: &AppContext) -> bool {
        UserWorkspaces::as_ref(app).is_prompt_suggestions_toggleable()
            && AISettings::as_ref(app)
                .prompt_suggestions_enabled_internal
                .is_supported_on_current_platform()
    }

    fn is_suggested_code_banners_toggleable(&self, app: &AppContext) -> bool {
        (self.is_prompt_suggestions_toggleable(app)
            || UserWorkspaces::as_ref(app).is_code_suggestions_toggleable())
            && AISettings::as_ref(app)
                .code_suggestions_enabled_internal
                .is_supported_on_current_platform()
    }

    fn is_natural_language_autosuggestions_toggleable(&self, app: &AppContext) -> bool {
        FeatureFlag::PredictAMQueries.is_enabled()
            && AISettings::as_ref(app)
                .natural_language_autosuggestions_enabled_internal
                .is_supported_on_current_platform()
    }

    // TODO: Check if the user's enterprise billing policy allows toggling this feature.
    fn is_shared_block_title_generation_toggleable(&self, app: &AppContext) -> bool {
        FeatureFlag::SharedBlockTitleGeneration.is_enabled()
            && AISettings::as_ref(app)
                .shared_block_title_generation_enabled_internal
                .is_supported_on_current_platform()
            && (!UserWorkspaces::as_ref(app)
                .current_team()
                .is_some_and(|team| {
                    team.billing_metadata.customer_type == CustomerType::Enterprise
                })
                // Override the enterprise check for dogfood builds, as our dogfood team
                // is an enterprise team.
                || ChannelState::channel().is_dogfood())
    }

    fn is_git_operations_autogen_toggleable(&self, app: &AppContext) -> bool {
        FeatureFlag::GitOperationsInCodeReview.is_enabled()
            && AISettings::as_ref(app)
                .git_operations_autogen_enabled_internal
                .is_supported_on_current_platform()
            && UserWorkspaces::as_ref(app).ai_allowed_for_current_team()
    }

    fn render_next_command_section(
        &self,
        view: &AISettingsPageView,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);

        Flex::column()
            .with_child(
                render_ai_setting_toggle::<IntelligentAutosuggestionsEnabled>(
                    "Next Command",
                    AISettingsPageAction::ToggleIntelligentAutosuggestions,
                    *ai_settings.intelligent_autosuggestions_enabled_internal,
                    is_toggleable,
                    self.intelligent_autosuggestions_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                NEXT_COMMAND_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }

    fn render_prompt_suggestions_section(
        &self,
        view: &AISettingsPageView,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(
                render_ai_setting_toggle::<AgentModeQuerySuggestionsEnabled>(
                    "Prompt Suggestions",
                    AISettingsPageAction::TogglePromptSuggestions,
                    *ai_settings.prompt_suggestions_enabled_internal,
                    is_toggleable,
                    self.prompt_suggestions_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                PROMPT_SUGGESTIONS_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }

    fn render_suggested_code_banners_section(
        &self,
        view: &AISettingsPageView,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(
                render_ai_setting_toggle::<AgentModeQuerySuggestionsEnabled>(
                    "Suggested Code Banners",
                    AISettingsPageAction::ToggleCodeSuggestions,
                    *ai_settings.code_suggestions_enabled_internal,
                    is_toggleable,
                    self.code_suggestions_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                SUGGESTED_CODE_BANNERS_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }

    fn render_natural_language_autosuggestions_section(
        &self,
        view: &AISettingsPageView,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(render_ai_setting_toggle::<
                NaturalLanguageAutosuggestionsEnabled,
            >(
                "Natural Language Autosuggestions",
                AISettingsPageAction::ToggleNaturalLanguageAutosuggestions,
                *ai_settings.natural_language_autosuggestions_enabled_internal,
                is_toggleable,
                self.natural_language_autosuggestions_toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                NATURAL_LANGUAGE_AUTOSUGGESTIONS,
                is_toggleable,
                app,
            ))
            .finish()
    }

    fn render_shared_block_title_generation_section(
        &self,
        view: &AISettingsPageView,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(
                render_ai_setting_toggle::<SharedBlockTitleGenerationEnabled>(
                    "Shared Block Title Generation",
                    AISettingsPageAction::ToggleSharedTitleGeneration,
                    *ai_settings.shared_block_title_generation_enabled_internal,
                    is_toggleable,
                    self.shared_block_title_generation_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
            )
            .with_child(render_ai_setting_description(
                SHARED_BLOCK_TITLE_GENERATION_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }

    fn render_git_operations_autogen_section(
        &self,
        view: &AISettingsPageView,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_active_ai_enabled(app);
        Flex::column()
            .with_child(render_ai_setting_toggle::<GitOperationsAutogenEnabled>(
                "Commit & Pull Request Generation",
                AISettingsPageAction::ToggleGitOperationsAutogen,
                *ai_settings.git_operations_autogen_enabled_internal,
                is_toggleable,
                self.git_operations_autogen_toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                GIT_OPERATIONS_AUTOGEN_DESCRIPTION,
                is_toggleable,
                app,
            ))
            .finish()
    }
}

impl SettingsWidget for ActiveAIWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "active ai a.i. next command prompt suggestions code diffs suggested banners passive unit tests commit pull request pr git code review autogen generate"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        self.is_next_command_toggleable(app)
            || self.is_prompt_suggestions_toggleable(app)
            || self.is_suggested_code_banners_toggleable(app)
            || self.is_natural_language_autosuggestions_toggleable(app)
            || self.is_shared_block_title_generation_toggleable(app)
            || self.is_git_operations_autogen_toggleable(app)
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        let mut column = Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                        .with_child(
                            build_sub_header(
                                appearance,
                                "Active AI",
                                Some(styles::header_font_color(is_any_ai_enabled, app)),
                            )
                            .finish(),
                        )
                        .with_child(
                            Container::new(render_ai_feature_switch(
                                self.active_ai_toggle.clone(),
                                *ai_settings.is_active_ai_enabled_internal,
                                is_any_ai_enabled,
                                AISettingsPageAction::ToggleActiveAI,
                                app,
                            ))
                            .with_padding_right(TOGGLE_BUTTON_RIGHT_PADDING)
                            .finish(),
                        )
                        .finish(),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            );

        if self.is_next_command_toggleable(app) {
            column.add_child(self.render_next_command_section(view, app));
        }

        if self.is_prompt_suggestions_toggleable(app) {
            column.add_child(self.render_prompt_suggestions_section(view, app));
        }

        if self.is_suggested_code_banners_toggleable(app) {
            column.add_child(self.render_suggested_code_banners_section(view, app));
        }

        if self.is_natural_language_autosuggestions_toggleable(app) {
            column.add_child(self.render_natural_language_autosuggestions_section(view, app));
        }

        if self.is_shared_block_title_generation_toggleable(app) {
            column.add_child(self.render_shared_block_title_generation_section(view, app));
        }

        if self.is_git_operations_autogen_toggleable(app) {
            column.add_child(self.render_git_operations_autogen_section(view, app));
        }

        column.finish()
    }
}

#[derive(Default)]
struct AgentsWidget {
    codebase_context_toggle: SwitchStateHandle,
    codebase_context_link_index: HighlightedHyperlink,
    show_in_prompt_checkbox: MouseStateHandle,
}

impl SettingsWidget for AgentsWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        if MCPServersWidget::should_show_mcp() {
            "ai a.i. agent autonomy profiles allowlist denylist autoexecute permissions models llms planning mcp server"
        } else {
            "ai a.i. agent autonomy profiles allowlist denylist autoexecute permissions models llms planning"
        }
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let mut column = Flex::column();

        if FeatureFlag::ProfilesDesignRevamp.is_enabled() {
            column.add_child(
                Container::new(self.render_profiles_section(view, ai_settings, appearance, app))
                    .with_margin_bottom(8.)
                    .finish(),
            );
        } else {
            // Legacy layout: show Agents header + Models + Permissions
            let mut agents_header = Flex::column();
            agents_header.add_child(
                build_sub_header(
                    appearance,
                    "Agents",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            );
            agents_header.add_child(render_ai_setting_description(
                "Set the boundaries for how your Agent operates. Choose what it can access, how much autonomy it has, and when it must ask for your approval. You can also fine-tune behavior around natural language input, codebase awareness, and more.",
                ai_settings.is_any_ai_enabled(app),
                app,
            ));
            let agents_header = agents_header.finish();
            column.add_children([
                render_separator(appearance),
                Container::new(agents_header)
                    .with_margin_bottom(8.)
                    .finish(),
            ]);
            column.add_children([
                Container::new(self.render_models_section(view, ai_settings, appearance, app))
                    .with_margin_bottom(8.)
                    .finish(),
                Container::new(self.render_permissions_section(view, ai_settings, appearance, app))
                    .with_margin_bottom(8.)
                    .finish(),
            ]);
        };

        column.finish()
    }
}

impl AgentsWidget {
    fn render_profiles_section(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let header_and_description = Flex::column()
            .with_child(
                build_sub_header(
                    appearance,
                    "Profiles",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .finish(),
            )
            .with_child(
                Container::new(
                    render_ai_setting_description(
                        "Profiles let you define how your Agent operates — from the actions it can take and when it needs approval, to the models it uses for tasks like coding and planning. You can also scope them to individual projects.",
                        is_any_ai_enabled,
                        app,
                    )
                )
                .with_margin_top(12.)
                .finish()
            )
            .finish();

        let mut profiles_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(Shrinkable::new(1., header_and_description).finish());

        if FeatureFlag::MultiProfile.is_enabled() {
            profiles_row.add_child(
                Container::new(view.add_profile_button.as_ref(app).render(app))
                    .with_margin_left(16.)
                    .finish(),
            );
        }

        let profiles_header = Container::new(profiles_row.finish())
            .with_margin_bottom(12.0)
            .finish();

        let mut profile_elements = vec![profiles_header];

        for profile_view in &view.profile_views {
            profile_elements.push(
                Container::new(ChildView::new(profile_view).finish())
                    .with_margin_bottom(8.)
                    .finish(),
            );
        }

        Flex::column().with_children(profile_elements).finish()
    }

    fn render_models_section(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        let model_subheader = Container::new(render_custom_size_header(
            appearance,
            "Models",
            14.0,
            Some(styles::header_font_color(is_any_ai_enabled, app)),
        ))
        .with_margin_bottom(8.0)
        .finish();

        let base_model_setting =
            Container::new(self.render_base_model_setting(view, ai_settings, appearance, app))
                .with_margin_bottom(8.0)
                .finish();

        let mut children = vec![model_subheader, base_model_setting];
        if let Some(context_window_setting) =
            self.render_context_window_setting(view, ai_settings, appearance, app)
        {
            children.push(
                Container::new(context_window_setting)
                    .with_margin_bottom(8.0)
                    .finish(),
            );
        }

        Flex::column().with_children(children).finish()
    }

    /// Renders the context window slider + numeric input row shown below the
    /// base model dropdown. Returns `None` if the active base model does not
    /// advertise a configurable context window, global AI is disabled, or the
    /// [`FeatureFlag::ConfigurableContextWindow`] flag is disabled.
    fn render_context_window_setting(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Option<Box<dyn Element>> {
        if !FeatureFlag::ConfigurableContextWindow.is_enabled() {
            return None;
        }
        if !ai_settings.is_any_ai_enabled(app) {
            return None;
        }
        let cw = AISettingsPageView::configurable_context_window(app)?;
        let min = cw.min;
        let max = cw.max;

        let label = Container::new(render_body_item_label::<AISettingsPageAction>(
            "Context window (tokens)".to_string(),
            None,
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
        ))
        .with_margin_bottom(4.0)
        .finish();

        let min_label = appearance
            .ui_builder()
            .span(format!("{min}"))
            .with_style(UiComponentStyles {
                font_size: Some(CONTENT_FONT_SIZE),
                ..Default::default()
            })
            .build()
            .finish();

        let max_label = appearance
            .ui_builder()
            .span(format!("{max}"))
            .with_style(UiComponentStyles {
                font_size: Some(CONTENT_FONT_SIZE),
                ..Default::default()
            })
            .build()
            .finish();

        let current_value = AISettingsPageView::current_context_window_display_value(app)
            .unwrap_or(cw.default_max)
            .clamp(min, max);
        let slider = appearance
            .ui_builder()
            .slider(view.context_window_slider_state.clone())
            .with_range(min as f32..max as f32)
            .with_default_value(current_value as f32)
            .with_style(UiComponentStyles {
                width: Some(CONTEXT_WINDOW_SLIDER_WIDTH),
                margin: Some(Coords::default().left(8.).right(8.)),
                ..Default::default()
            })
            .on_drag(|ctx, _, val| {
                ctx.dispatch_typed_action(AISettingsPageAction::ContextWindowSliderDragged(
                    val.round() as u32,
                ));
            })
            .on_change(|ctx, _, val| {
                ctx.dispatch_typed_action(AISettingsPageAction::SetContextWindowSize(
                    val.round() as u32
                ));
            })
            .build()
            .finish();

        let context_window_editor = view.context_window_editor.clone();
        let input_box = Dismiss::new(
            appearance
                .ui_builder()
                .text_input(view.context_window_editor.clone())
                .with_style(UiComponentStyles {
                    width: Some(CONTEXT_WINDOW_INPUT_BOX_WIDTH),
                    padding: Some(Coords {
                        top: 6.,
                        bottom: 6.,
                        left: 10.,
                        right: 10.,
                    }),
                    margin: Some(Coords::default().left(12.)),
                    background: Some(appearance.theme().surface_2().into()),
                    ..Default::default()
                })
                .build()
                .finish(),
        )
        .on_dismiss(move |ctx, app| {
            let buffer_text = context_window_editor.as_ref(app).buffer_text(app);
            let cleaned: String = buffer_text
                .chars()
                .filter(|c| !c.is_whitespace() && *c != ',')
                .collect();
            if let Ok(parsed) = cleaned.parse::<u32>() {
                ctx.dispatch_typed_action(AISettingsPageAction::SetContextWindowSize(parsed));
            }
        })
        .finish();

        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(min_label)
            .with_child(slider)
            .with_child(max_label)
            .with_child(input_box)
            .finish();

        Some(Flex::column().with_child(label).with_child(row).finish())
    }

    fn render_permissions_section(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        let permissions_subheader = Container::new(render_custom_size_header(
            appearance,
            "Permissions",
            14.0,
            Some(styles::header_font_color(is_any_ai_enabled, app)),
        ))
        .with_margin_bottom(4.0)
        .finish();

        let code_diff_setting =
            BlocklistAIPermissions::as_ref(app).get_apply_code_diffs_setting(app, None);
        let code_diffs = self.render_execution_profile_dropdown(
            "Apply code diffs",
            Icon::Code2,
            code_diff_setting.description(),
            &view.apply_code_diffs_dropdown_menu,
            ai_settings,
            appearance,
            app,
        );

        let read_files_setting =
            BlocklistAIPermissions::as_ref(app).get_read_files_setting(app, None);
        let mut read_files_flex = Flex::column().with_main_axis_size(MainAxisSize::Min);
        read_files_flex.add_child(self.render_execution_profile_dropdown(
            "Read files",
            Icon::Notebook,
            read_files_setting.description(),
            &view.read_files_dropdown_menu,
            ai_settings,
            appearance,
            app,
        ));

        if read_files_setting == ActionPermission::AlwaysAsk {
            let directory_allowlist =
                BlocklistAIPermissions::as_ref(app).get_read_files_allowlist(app, None);
            read_files_flex.add_child(
                Container::new(Self::render_directory_allowlist(
                    directory_allowlist,
                    view,
                    ai_settings,
                    appearance,
                    app,
                ))
                .with_margin_bottom(HEADER_PADDING)
                .finish(),
            );
        }
        let read_files = read_files_flex.finish();

        let execute_commands_setting =
            BlocklistAIPermissions::as_ref(app).get_execute_commands_setting(app, None);
        let mut execute_commands_flex = Flex::column().with_main_axis_size(MainAxisSize::Min);
        execute_commands_flex.add_child(self.render_execution_profile_dropdown(
            "Execute commands",
            Icon::Terminal,
            execute_commands_setting.description(),
            &view.execute_commands_dropdown_menu,
            ai_settings,
            appearance,
            app,
        ));

        if execute_commands_setting == ActionPermission::AlwaysAsk
            || execute_commands_setting == ActionPermission::AgentDecides
        {
            let command_allowlist =
                BlocklistAIPermissions::as_ref(app).get_execute_commands_allowlist(app, None);
            execute_commands_flex.add_child(
                Container::new(Self::render_command_allowlist(
                    command_allowlist,
                    view,
                    ai_settings,
                    appearance,
                    app,
                ))
                .with_margin_bottom(8.)
                .finish(),
            );
        }

        if execute_commands_setting != ActionPermission::AlwaysAsk {
            let command_denylist = Container::new(Self::render_command_denylist(
                BlocklistAIPermissions::as_ref(app).get_execute_commands_denylist(app, None),
                view,
                ai_settings,
                appearance,
                app,
            ))
            .with_margin_bottom(8.)
            .finish();
            execute_commands_flex.add_child(command_denylist);
        }
        let execute_commands = execute_commands_flex.finish();

        let mut widget_children = vec![permissions_subheader];

        if UserWorkspaces::as_ref(app)
            .ai_autonomy_settings()
            .has_any_overrides()
        {
            widget_children.push(
                Container::new(render_settings_info_banner(
                    "Some of your permissions are managed by your workspace.",
                    None,
                    appearance,
                ))
                .with_margin_bottom(12.0)
                .finish(),
            );
        }

        widget_children.extend([code_diffs, read_files, execute_commands]);

        let write_to_pty_setting =
            BlocklistAIPermissions::as_ref(app).get_write_to_pty_setting(app, None);
        let write_to_pty = self.render_execution_profile_dropdown(
            "Interact with running commands",
            Icon::Workflow,
            write_to_pty_setting.description(),
            &view.write_to_pty_autonomy_dropdown_menu,
            ai_settings,
            appearance,
            app,
        );
        widget_children.push(write_to_pty);

        if MCPServersWidget::should_show_mcp() {
            let mcp_permissions = self.render_mcp_permissions(view, ai_settings, appearance, app);
            widget_children.push(mcp_permissions);
        }

        if !FeatureFlag::FullSourceCodeEmbedding.is_enabled() {
            let codebase_context = Self::render_codebase_context_outline_generation_setting(
                self.codebase_context_toggle.clone(),
                self.codebase_context_link_index.clone(),
                view,
                ai_settings,
                appearance,
                app,
            );
            widget_children.push(codebase_context);
        }

        Flex::column().with_children(widget_children).finish()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_execution_profile_dropdown(
        &self,
        header_text: &str,
        header_icon: Icon,
        permission_description: &'static str,
        dropdown_menu: &ViewHandle<Dropdown<AISettingsPageAction>>,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn Element> {
        let header = Container::new(render_body_item_label_with_icon::<AISettingsPageAction>(
            header_text.into(),
            header_icon,
            Some(styles::header_font_color(
                ai_settings.is_any_ai_enabled(app),
                app,
            )),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
        ))
        .finish();

        let description_color = appearance.theme().disabled_ui_text_color();
        let alert_icon = Container::new(
            ConstrainedBox::new(
                Icon::AlertCircle
                    .to_warpui_icon(
                        appearance
                            .theme()
                            .sub_text_color(appearance.theme().surface_2()),
                    )
                    .finish(),
            )
            .with_width(14.)
            .with_height(14.)
            .finish(),
        )
        .with_margin_right(4.)
        .finish();
        let text = Text::new(
            permission_description,
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(description_color.into())
        .finish();
        let description = Flex::row()
            .with_children([alert_icon, Shrinkable::new(1.0, text).finish()])
            .finish();

        Container::new(
            Flex::column()
                .with_child(header)
                .with_child(ChildView::new(dropdown_menu).finish())
                .with_child(description)
                .finish(),
        )
        .with_margin_bottom(12.)
        .finish()
    }

    fn render_command_denylist(
        command_denylist: Vec<AgentModeCommandExecutionPredicate>,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_disabled = !ai_settings.is_any_ai_enabled(app);
        let org_denylist = BlocklistAIPermissions::get_org_execute_commands_denylist(app);
        let mut tooltip_idx = 0usize;
        let list = render_input_list(
            None,
            command_denylist
                .into_iter()
                .zip(view.command_denylist_mouse_state_handles.clone())
                .rev()
                .map(|(cmd, mouse_state_handle)| {
                    let is_org = org_denylist.contains(&cmd);
                    let tooltip_mouse_state = if is_org {
                        let handle = view
                            .command_denylist_tooltip_mouse_state_handles
                            .get(tooltip_idx)
                            .cloned();
                        tooltip_idx += 1;
                        handle
                    } else {
                        None
                    };
                    InputListItem {
                        item: cmd.to_string(),
                        mouse_state_handle,
                        on_remove_action: AISettingsPageAction::RemoveFromProfileCommandDenylist(
                            cmd,
                        ),
                        is_disabled: is_org || ai_disabled,
                        tooltip_mouse_state,
                    }
                }),
            Some(&view.command_denylist_editor),
            appearance,
        );
        render_ai_list(
            "Command denylist",
            "Regular expressions to match commands that the Warp Agent should always ask permission to execute.",
            list,
            view,
            ai_settings,
            app,
        )
    }

    fn render_command_allowlist(
        command_allowlist: Vec<AgentModeCommandExecutionPredicate>,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let disabled = !ai_settings.is_command_allowlist_editable(app);
        let list = render_input_list(
            None,
            command_allowlist
                .into_iter()
                .zip(view.command_allowlist_mouse_state_handles.clone())
                .rev()
                .map(move |(cmd, mouse_state_handle)| InputListItem {
                    item: cmd.to_string(),
                    mouse_state_handle,
                    on_remove_action: AISettingsPageAction::RemoveFromProfileCommandAllowlist(cmd),
                    is_disabled: disabled,
                    tooltip_mouse_state: None,
                }),
            Some(&view.command_allowlist_editor),
            appearance,
        );

        render_ai_list(
            "Command allowlist",
            "Regular expressions to match commands that can be automatically executed by the Warp Agent.",
            list,
            view,
            ai_settings,
            app,
        )
    }

    fn render_directory_allowlist(
        directory_allowlist: Vec<PathBuf>,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let disabled = !ai_settings.is_directory_allowlist_editable(app);
        let list = render_input_list(
            None,
            directory_allowlist
                .clone()
                .into_iter()
                .zip(view.directory_allowlist_mouse_state_handles.clone())
                .rev()
                .map(move |(path, mouse_state_handle)| InputListItem {
                    item: path.display().to_string(),
                    mouse_state_handle,
                    on_remove_action: AISettingsPageAction::RemoveFromProfileDirectoryAllowlist(
                        path,
                    ),
                    is_disabled: disabled,
                    tooltip_mouse_state: None,
                }),
            Some(&view.directory_allowlist_editor),
            appearance,
        );

        render_ai_list(
            "Directory allowlist",
            "Give the agent file access to certain directories.",
            list,
            view,
            ai_settings,
            app,
        )
    }

    fn render_base_model_setting(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let show_in_prompt_checkbox = {
            let is_checked = *SessionSettings::as_ref(app).show_model_selectors_in_prompt;

            let mut checkbox = appearance
                .ui_builder()
                .checkbox(self.show_in_prompt_checkbox.clone(), None)
                .check(is_checked);

            if !ai_settings.is_any_ai_enabled(app) {
                checkbox = checkbox.disabled();
            }

            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_children([
                        checkbox
                            .build()
                            .on_click(move |ctx, _, _| {
                                ctx.dispatch_typed_action(
                                    AISettingsPageAction::ToggleShowBaseModelPickerInPrompt,
                                );
                            })
                            .finish(),
                        appearance
                            .ui_builder()
                            .span("Show model picker in prompt".to_string())
                            .with_style(UiComponentStyles {
                                font_color: Some(
                                    theme.sub_text_color(theme.surface_2()).into_solid(),
                                ),
                                font_size: Some(12.0),
                                ..Default::default()
                            })
                            .build()
                            .finish(),
                    ])
                    .finish(),
            )
            .with_margin_top(-6.0)
            .with_margin_left(-4.0)
            .finish()
        };

        render_dropdown_item(
            appearance,
            "Base model",
            Some(
                "This model serves as the primary engine behind the Warp Agent. It powers most interactions and invokes other models for tasks like planning or code generation when necessary. Warp may automatically switch to alternate models based on model availability or for auxiliary tasks such as conversation summarization.",
            ),
            Some(show_in_prompt_checkbox),
            LocalOnlyIconState::Hidden,
            (!ai_settings.is_any_ai_enabled(app))
                .then(|| appearance.theme().disabled_ui_text_color()),
            &view.base_model_dropdown,
        )
    }

    fn render_codebase_context_outline_generation_setting(
        codebase_context_toggle: SwitchStateHandle,
        codebase_context_link_index: HighlightedHyperlink,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);
        let toggle = render_ai_setting_toggle::<CodebaseContextEnabled>(
            "Codebase Context",
            AISettingsPageAction::ToggleCodebaseContext,
            *code_settings.codebase_context_enabled,
            ai_settings.is_any_ai_enabled(app),
            codebase_context_toggle,
            &view.local_only_icon_tooltip_states,
            app,
        );

        let codebase_context_description = vec![
            FormattedTextFragment::plain_text(
                "Allow the Warp Agent to generate an outline of your codebase that can be used for context. No code is ever stored on our servers. ",
            ),
            FormattedTextFragment::hyperlink(
                "Learn more",
                "https://docs.warp.dev/agent-platform/capabilities/codebase-context",
            ),
        ];
        let description = Container::new(
            FormattedTextElement::new(
                FormattedText::new([FormattedTextLine::Line(codebase_context_description)]),
                CONTENT_FONT_SIZE,
                appearance.ui_font_family(),
                appearance.ui_font_family(),
                styles::description_font_color(ai_settings.is_any_ai_enabled(app), app).into(),
                codebase_context_link_index,
            )
            .with_hyperlink_font_color(appearance.theme().accent().into_solid())
            .register_default_click_handlers(|url, ctx, _| {
                ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
            })
            .finish(),
        )
        .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
        .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
        .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
        .finish();

        Flex::column()
            .with_child(toggle)
            .with_child(description)
            .finish()
    }

    fn render_mcp_permissions(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let all_runnable_mcp_servers =
            TemplatableMCPServerManager::get_all_cloud_synced_mcp_servers(app);
        if all_runnable_mcp_servers.is_empty() {
            self.render_mcp_permissions_zero_state(ai_settings, appearance, app)
        } else {
            self.render_mcp_permissions_with_servers(view, ai_settings, appearance, app)
        }
    }

    fn render_mcp_permissions_zero_state(
        &self,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let header = Container::new(render_body_item_label_with_icon::<AISettingsPageAction>(
            "Call MCP servers".into(),
            Icon::Dataflow,
            Some(styles::header_font_color(
                ai_settings.is_any_ai_enabled(app),
                app,
            )),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
        ))
        .with_margin_bottom(4.)
        .finish();

        let subtext = {
            let subtext_fragments = vec![
                FormattedTextFragment::plain_text(
                    "You haven't added any MCP servers yet. Once you do, you'll be able to control how much autonomy the Warp Agent has when interacting with them. ",
                ),
                FormattedTextFragment::hyperlink_action(
                    "Add a server",
                    AISettingsPageAction::OpenMCPServerCollection,
                ),
                FormattedTextFragment::plain_text(" or "),
                FormattedTextFragment::hyperlink(
                    "learn more about MCPs.",
                    "https://docs.warp.dev/agent-platform/capabilities/mcp",
                ),
            ];

            Container::new(
                FormattedTextElement::new(
                    FormattedText::new([FormattedTextLine::Line(subtext_fragments)]),
                    CONTENT_FONT_SIZE,
                    appearance.ui_font_family(),
                    appearance.ui_font_family(),
                    styles::description_font_color(ai_settings.is_any_ai_enabled(app), app).into(),
                    HighlightedHyperlink::default(),
                )
                .with_hyperlink_font_color(appearance.theme().accent().into_solid())
                .register_default_click_handlers_with_action_support(|hyperlink_lens, ctx, _app| {
                    match hyperlink_lens {
                        HyperlinkLens::Url(url) => {
                            ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(
                                HyperlinkUrl {
                                    url: url.to_owned(),
                                },
                            ));
                        }
                        HyperlinkLens::Action(action_ref) => {
                            if let Some(action) =
                                action_ref.as_any().downcast_ref::<AISettingsPageAction>()
                            {
                                ctx.dispatch_typed_action(action.clone());
                            }
                        }
                    }
                })
                .finish(),
            )
            .with_margin_bottom(4.0)
            .finish()
        };

        Flex::column()
            .with_child(header)
            .with_child(subtext)
            .finish()
    }

    fn render_mcp_permissions_with_servers(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut column = Flex::column();

        let current_mcp_setting =
            BlocklistAIPermissions::as_ref(app).get_mcp_permissions_setting(app, None);

        let permission_setting = self.render_execution_profile_dropdown(
            "Call MCP servers",
            Icon::Dataflow,
            current_mcp_setting.description(),
            &view.mcp_permissions_dropdown_menu,
            ai_settings,
            appearance,
            app,
        );
        column.add_child(permission_setting);

        if current_mcp_setting == ActionPermission::AlwaysAsk
            || current_mcp_setting == ActionPermission::AgentDecides
        {
            let allowlist = self.render_mcp_list(
                "MCP allowlist",
                "Allow the Warp Agent to call these MCP servers.",
                &view.mcp_allowlist_dropdown,
                BlocklistAIPermissions::as_ref(app).get_mcp_allowlist(app, None),
                view.mcp_allowlist_mouse_state_handles.clone(),
                AISettingsPageAction::RemoveFromMCPAllowlist,
                ai_settings,
                appearance,
                app,
            );
            column.add_child(allowlist);
        }

        if current_mcp_setting == ActionPermission::AlwaysAllow
            || current_mcp_setting == ActionPermission::AgentDecides
        {
            let denylist = self.render_mcp_list(
                "MCP denylist",
                "The Warp Agent will always ask for permission before calling any MCP servers on this list.",
                &view.mcp_denylist_dropdown,
                BlocklistAIPermissions::as_ref(app).get_mcp_denylist(app, None),
                view.mcp_denylist_mouse_state_handles.clone(),
                AISettingsPageAction::RemoveFromMCPDenylist,
                ai_settings,
                appearance,
                app,
            );
            column.add_child(denylist);
        }

        column.finish()
    }

    // Helper function to render the allow and denylists for mcp servers
    #[allow(clippy::too_many_arguments)]
    fn render_mcp_list(
        &self,
        title: &str,
        description: &str,
        dropdown: &ViewHandle<FilterableDropdown<AISettingsPageAction>>,
        items: Vec<uuid::Uuid>,
        mouse_state_handles: Vec<MouseStateHandle>,
        action: impl Fn(uuid::Uuid) -> AISettingsPageAction,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let selector = Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_children(vec![
                    Shrinkable::new(
                        1.0,
                        Container::new(render_dropdown_item_label(
                            title.to_string(),
                            Some(description.to_string()),
                            LocalOnlyIconState::Hidden,
                            (!ai_settings.is_any_ai_enabled(app))
                                .then(|| appearance.theme().disabled_ui_text_color()),
                            appearance,
                        ))
                        .with_margin_right(4.)
                        .finish(),
                    )
                    .finish(),
                    ChildView::new(dropdown).finish(),
                ])
                .finish(),
        )
        .with_margin_bottom(2.)
        .finish();

        let disabled = !ai_settings.is_any_ai_enabled(app);
        let items = render_input_list(
            None,
            items
                .into_iter()
                .rev()
                .zip(mouse_state_handles.clone())
                .filter_map(move |(uuid, mouse_state_handle)| {
                    let server_name = TemplatableMCPServerManager::get_mcp_name(&uuid, app);
                    server_name.map(|server_name| InputListItem {
                        item: server_name,
                        mouse_state_handle,
                        on_remove_action: action(uuid),
                        is_disabled: disabled,
                        tooltip_mouse_state: None,
                    })
                }),
            None,
            appearance,
        );

        Container::new(Flex::column().with_children(vec![selector, items]).finish())
            .with_margin_bottom(8.)
            .finish()
    }
}

#[derive(Default)]
struct AIInputWidget {
    incorrect_autodetection_highlight_index: HighlightedHyperlink,
    autodetection_toggle: SwitchStateHandle,
    nld_in_terminal_toggle: SwitchStateHandle,
    show_input_hint_toggle: SwitchStateHandle,
    show_agent_tips_toggle: SwitchStateHandle,
    include_agent_commands_in_history_toggle: SwitchStateHandle,
}

impl SettingsWidget for AIInputWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "oz agent ai input natural language detection autodetection prompt terminal command commands history shell executed execution"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let input_header = build_sub_header(
            appearance,
            "Input",
            Some(styles::header_font_color(is_any_ai_enabled, app)),
        )
        .with_padding_bottom(HEADER_PADDING)
        .finish();

        let natural_language_detection_section = Self::render_natural_language_detection_section(
            self.incorrect_autodetection_highlight_index.clone(),
            self.autodetection_toggle.clone(),
            self.nld_in_terminal_toggle.clone(),
            view,
            ai_settings,
            appearance,
            app,
        );

        let show_input_hint_text = render_ai_setting_toggle::<ShowHintText>(
            "Show input hint text",
            AISettingsPageAction::ToggleShowInputHintText,
            *InputSettings::as_ref(app).show_hint_text,
            is_any_ai_enabled,
            self.show_input_hint_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        );

        let mut widget_children = vec![
            render_separator(appearance),
            input_header,
            natural_language_detection_section,
            show_input_hint_text,
        ];

        if FeatureFlag::AgentTips.is_enabled() {
            let agent_tips_toggle = render_ai_setting_toggle::<ShowAgentTips>(
                "Show agent tips",
                AISettingsPageAction::ToggleShowAgentTips,
                *InputSettings::as_ref(app).show_agent_tips,
                is_any_ai_enabled,
                self.show_agent_tips_toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            );
            widget_children.push(agent_tips_toggle);
        }

        widget_children.push(render_ai_setting_toggle::<IncludeAgentCommandsInHistory>(
            "Include agent-executed commands in history",
            AISettingsPageAction::ToggleIncludeAgentCommandsInHistory,
            *ai_settings.include_agent_commands_in_history,
            is_any_ai_enabled,
            self.include_agent_commands_in_history_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        ));

        Flex::column().with_children(widget_children).finish()
    }
}

impl AIInputWidget {
    fn render_natural_language_detection_section(
        incorrect_autodetection_highlight_index: HighlightedHyperlink,
        autodetection_toggle: SwitchStateHandle,
        nld_in_terminal_toggle: SwitchStateHandle,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let is_toggleable = ai_settings.is_any_ai_enabled(app);
        let is_nld_enabled = *ai_settings.ai_autodetection_enabled_internal.value();

        let autodetection_denylist_input_field = appearance
            .ui_builder()
            .text_input(view.autodetection_denylist_editor.clone())
            .with_style(UiComponentStyles {
                width: Some(280.),
                padding: Some(Coords {
                    top: 4.,
                    bottom: 4.,
                    left: 6.,
                    right: 6.,
                }),
                background: Some(appearance.theme().surface_2().into()),
                ..Default::default()
            })
            .build()
            .finish();

        let mut section = Flex::column();

        if FeatureFlag::AgentView.is_enabled() {
            static AUTODETECTION_DESCRIPTION_FRAGMENTS: LazyLock<Vec<FormattedTextFragment>> =
                LazyLock::new(|| {
                    vec![
                        FormattedTextFragment::plain_text("Encountered an incorrect detection? "),
                        FormattedTextFragment::hyperlink(
                            "Let us know",
                            "https://warpdotdev.typeform.com/to/offrTIpq",
                        ),
                    ]
                });

            section.add_children([
                render_ai_setting_toggle::<NLDInTerminalEnabled>(
                    "Autodetect agent prompts in terminal input",
                    AISettingsPageAction::ToggleNLDInTerminal,
                    ai_settings.is_nld_in_terminal_enabled(app),
                    is_toggleable,
                    nld_in_terminal_toggle,
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
                render_ai_setting_toggle::<AIAutoDetectionEnabled>(
                    "Autodetect terminal commands in agent input",
                    AISettingsPageAction::ToggleAIInputAutoDetection,
                    is_nld_enabled,
                    is_toggleable,
                    autodetection_toggle,
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
                Container::new(
                    FormattedTextElement::new(
                        FormattedText::new([FormattedTextLine::Line(
                            (*AUTODETECTION_DESCRIPTION_FRAGMENTS).clone(),
                        )]),
                        CONTENT_FONT_SIZE,
                        appearance.ui_font_family(),
                        appearance.ui_font_family(),
                        styles::description_font_color(is_toggleable, app).into(),
                        incorrect_autodetection_highlight_index,
                    )
                    .with_hyperlink_font_color(appearance.theme().accent().into_solid())
                    .register_default_click_handlers(|url, ctx, _| {
                        ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
                    })
                    .finish(),
                )
                .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                .finish(),
            ])
        } else {
            static NATURAL_LANGUAGE_DETECTION_DESCRIPTION_FRAGMENTS: LazyLock<
                Vec<FormattedTextFragment>,
            > = LazyLock::new(|| {
                vec![
                    FormattedTextFragment::plain_text(
                        "Enabling natural language detection will detect when natural language is written in the terminal input, and then automatically switch to Agent Mode for AI queries.",
                    ),
                    FormattedTextFragment::plain_text(
                        " Encountered an incorrect input detection? ",
                    ),
                    FormattedTextFragment::hyperlink(
                        "Let us know",
                        "https://warpdotdev.typeform.com/to/offrTIpq",
                    ),
                ]
            });

            section.add_children([
                render_ai_setting_toggle::<AIAutoDetectionEnabled>(
                    "Natural language detection",
                    AISettingsPageAction::ToggleAIInputAutoDetection,
                    is_nld_enabled,
                    is_toggleable,
                    autodetection_toggle,
                    &view.local_only_icon_tooltip_states,
                    app,
                ),
                Container::new(
                    FormattedTextElement::new(
                        FormattedText::new([FormattedTextLine::Line(
                            (*NATURAL_LANGUAGE_DETECTION_DESCRIPTION_FRAGMENTS).clone(),
                        )]),
                        CONTENT_FONT_SIZE,
                        appearance.ui_font_family(),
                        appearance.ui_font_family(),
                        styles::description_font_color(is_toggleable, app).into(),
                        incorrect_autodetection_highlight_index,
                    )
                    .with_hyperlink_font_color(appearance.theme().accent().into_solid())
                    .register_default_click_handlers(|url, ctx, _| {
                        ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
                    })
                    .finish(),
                )
                .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                .finish(),
            ]);
        }

        section
            .with_child(render_ai_setting_label::<AICommandDenylist>(
                "Natural language denylist".to_owned(),
                is_toggleable,
                &view.local_only_icon_tooltip_states,
                app,
            ))
            .with_child(render_ai_setting_description(
                "Commands listed here will never trigger natural language detection.",
                is_toggleable,
                app,
            ))
            .with_child(
                Container::new(autodetection_denylist_input_field)
                    .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                    .finish(),
            )
            .finish()
    }
}

#[derive(Default)]
struct MCPServersWidget {
    manage_mcp_servers_button: MouseStateHandle,
    mcp_docs_link_index: HighlightedHyperlink,
    file_based_mcp_toggle: SwitchStateHandle,
    file_based_mcp_docs_link_index: HighlightedHyperlink,
}

impl MCPServersWidget {
    fn should_show_mcp() -> bool {
        FeatureFlag::McpServer.is_enabled() && ContextFlag::ShowMCPServers.is_enabled()
    }
}

impl SettingsWidget for MCPServersWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "oz agent mcp server servers model context protocol file-based file based project claude .mcp.json .claude/.mcp.json .codex config.toml .codex/config.toml"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        MCPServersWidget::should_show_mcp()
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        let ai_settings = AISettings::as_ref(app);

        let header = build_sub_header(
            appearance,
            "MCP Servers",
            Some(styles::header_font_color(is_any_ai_enabled, app)),
        )
        .with_padding_bottom(HEADER_PADDING)
        .finish();

        let mcp_description = vec![
            FormattedTextFragment::plain_text(
                "Add MCP servers to extend the Warp Agent's capabilities. \
            MCP servers expose data sources or tools to agents through a standardized interface, essentially acting like plugins. ",
            ),
            FormattedTextFragment::hyperlink(
                "Learn more",
                "https://docs.warp.dev/agent-platform/capabilities/mcp",
            ),
        ];

        let description = Container::new(
            FormattedTextElement::new(
                FormattedText::new([FormattedTextLine::Line(mcp_description)]),
                CONTENT_FONT_SIZE,
                appearance.ui_font_family(),
                appearance.ui_font_family(),
                styles::description_font_color(is_any_ai_enabled, app).into(),
                self.mcp_docs_link_index.clone(),
            )
            .with_hyperlink_font_color(appearance.theme().accent().into_solid())
            .register_default_click_handlers(|url, ctx, _| {
                ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
            })
            .finish(),
        )
        .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
        .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
        .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
        .finish();

        let file_based_mcp_toggle = if FeatureFlag::FileBasedMcp.is_enabled() {
            Some(
                Flex::column()
                    .with_child(render_ai_setting_toggle::<FileBasedMcpEnabled>(
                        "Auto-spawn servers from third-party agents",
                        AISettingsPageAction::ToggleFileBasedMcp,
                        *ai_settings.file_based_mcp_enabled,
                        is_any_ai_enabled,
                        self.file_based_mcp_toggle.clone(),
                        &view.local_only_icon_tooltip_states,
                        app,
                    ))
                    .with_child({
                        static FILE_BASED_MCP_DESCRIPTION_FRAGMENTS: LazyLock<
                            Vec<FormattedTextFragment>,
                        > = LazyLock::new(|| {
                            vec![
                                FormattedTextFragment::plain_text(
                                    "Automatically detect and spawn MCP servers from globally-scoped third-party AI agent configuration files (e.g. in your home directory). Servers detected inside a repository are never spawned automatically and must be enabled individually from the MCP settings page. ",
                                ),
                                FormattedTextFragment::hyperlink(
                                    "See supported providers.",
                                    "https://docs.warp.dev/agent-platform/capabilities/mcp#file-based-mcp-servers",
                                ),
                            ]
                        });
                        Container::new(
                            FormattedTextElement::new(
                                FormattedText::new([FormattedTextLine::Line(
                                    (*FILE_BASED_MCP_DESCRIPTION_FRAGMENTS).clone(),
                                )]),
                                CONTENT_FONT_SIZE,
                                appearance.ui_font_family(),
                                appearance.ui_font_family(),
                                styles::description_font_color(is_any_ai_enabled, app).into(),
                                self.file_based_mcp_docs_link_index.clone(),
                            )
                            .with_hyperlink_font_color(appearance.theme().accent().into_solid())
                            .register_default_click_handlers(|url, ctx, _| {
                                ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
                            })
                            .finish(),
                        )
                        .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                        .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                        .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                        .finish()
                    })
                    .finish(),
            )
        } else {
            None
        };

        let button = render_full_pane_width_ai_button(
            "Manage MCP servers",
            is_any_ai_enabled,
            self.manage_mcp_servers_button.clone(),
            AISettingsPageAction::OpenMCPServerCollection,
            appearance,
        );

        let mut column = Flex::column()
            .with_child(header)
            .with_child(description)
            .with_child(button);

        if let Some(toggle) = file_based_mcp_toggle {
            column = column.with_child(toggle);
        }
        column.finish()
    }
}

#[derive(Default)]
struct AIFactWidget {
    rules_toggle: SwitchStateHandle,
    rules_link_index: HighlightedHyperlink,
    manage_rules_button: MouseStateHandle,
    rule_suggestions_toggle: SwitchStateHandle,
    warp_drive_context_toggle: SwitchStateHandle,
}

impl AIFactWidget {
    fn render_rules_toggle(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn Element> {
        let toggle = render_ai_setting_toggle::<MemoryEnabled>(
            "Rules",
            AISettingsPageAction::ToggleRules,
            *ai_settings.memory_enabled,
            ai_settings.is_any_ai_enabled(app),
            self.rules_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        );

        let rules_description = vec![
            FormattedTextFragment::plain_text(
                "Rules help the Warp Agent follow your conventions, whether for codebases or specific workflows. ",
            ),
            FormattedTextFragment::hyperlink(
                "Learn more",
                "https://docs.warp.dev/agent-platform/capabilities/rules",
            ),
        ];
        let description = Container::new(
            FormattedTextElement::new(
                FormattedText::new([FormattedTextLine::Line(rules_description)]),
                CONTENT_FONT_SIZE,
                appearance.ui_font_family(),
                appearance.ui_font_family(),
                styles::description_font_color(ai_settings.is_any_ai_enabled(app), app).into(),
                self.rules_link_index.clone(),
            )
            .with_hyperlink_font_color(appearance.theme().accent().into_solid())
            .register_default_click_handlers(|url, ctx, _| {
                ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
            })
            .finish(),
        )
        .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
        .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
        .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
        .finish();

        Flex::column()
            .with_child(toggle)
            .with_child(description)
            .finish()
    }

    fn render_rule_suggestions_toggle(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        app: &warpui::AppContext,
    ) -> Box<dyn Element> {
        let toggle = render_ai_setting_toggle::<RuleSuggestionsEnabled>(
            "Suggested Rules",
            AISettingsPageAction::ToggleRuleSuggestions,
            *ai_settings.rule_suggestions_enabled_internal,
            ai_settings.is_any_ai_enabled(app),
            self.rule_suggestions_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        );

        let description = render_ai_setting_description(
            "Let AI suggest rules to save based on your interactions.",
            ai_settings.is_any_ai_enabled(app),
            app,
        );

        Flex::column()
            .with_child(toggle)
            .with_child(description)
            .finish()
    }

    fn render_warp_drive_context_toggle(
        &self,
        view: &AISettingsPageView,
        ai_settings: &AISettings,
        app: &warpui::AppContext,
    ) -> Box<dyn Element> {
        let toggle = render_ai_setting_toggle::<WarpDriveContextEnabled>(
            "Warp Drive as agent context",
            AISettingsPageAction::ToggleWarpDriveContext,
            *ai_settings.warp_drive_context_enabled,
            ai_settings.is_any_ai_enabled(app),
            self.warp_drive_context_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        );

        let description = render_ai_setting_description(
            "The Warp Agent can leverage your Warp Drive Contents to tailor responses to your personal and team developer workflows and environments. This includes any Workflows, Notebooks, and Environment Variables.",
            ai_settings.is_any_ai_enabled(app),
            app,
        );

        Flex::column()
            .with_child(toggle)
            .with_child(description)
            .finish()
    }
}

impl SettingsWidget for AIFactWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "agent oz ai a.i. knowledge fact memory memories rules warp drive context workflows notebooks environment variables"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        FeatureFlag::AIRules.is_enabled()
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let header = build_sub_header(
            appearance,
            "Knowledge",
            Some(styles::header_font_color(is_any_ai_enabled, app)),
        )
        .with_margin_bottom(HEADER_PADDING)
        .finish();

        let button = render_full_pane_width_ai_button(
            "Manage rules",
            is_any_ai_enabled,
            self.manage_rules_button.clone(),
            AISettingsPageAction::OpenAIFactCollection,
            appearance,
        );

        let mut column = Flex::column()
            .with_child(header)
            .with_child(self.render_rules_toggle(view, ai_settings, appearance, app));

        if FeatureFlag::SuggestedRules.is_enabled() {
            column.add_child(self.render_rule_suggestions_toggle(view, ai_settings, app));
        }

        column
            .with_child(button)
            .with_child(self.render_warp_drive_context_toggle(view, ai_settings, app))
            .finish()
    }
}

#[derive(Default)]
struct VoiceWidget {
    voice_input_toggle: SwitchStateHandle,
    wispr_highlight_index: HighlightedHyperlink,
}

impl VoiceWidget {
    fn render_voice_section(
        &self,
        view: &AISettingsPageView,
        appearance: &Appearance,
        app: &warpui::AppContext,
    ) -> Box<dyn warpui::Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_toggleable = ai_settings.is_any_ai_enabled(app);
        let mut column = Flex::column().with_child(render_ai_setting_toggle::<VoiceInputEnabled>(
            "Voice Input",
            AISettingsPageAction::ToggleVoiceInput,
            *ai_settings.voice_input_enabled_internal,
            is_toggleable,
            self.voice_input_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        ));

        let voice_input_description_text_fragments = vec![
            FormattedTextFragment::plain_text(
                "Voice input allows you to control Warp by speaking directly to your terminal (powered by ",
            ),
            FormattedTextFragment::hyperlink("Wispr Flow", WISPR_FLOW_URL),
            FormattedTextFragment::plain_text(")."),
        ];

        let voice_input_description = FormattedTextElement::new(
            FormattedText::new([FormattedTextLine::Line(
                voice_input_description_text_fragments,
            )]),
            appearance.ui_font_size(),
            appearance.ui_font_family(),
            appearance.ui_font_family(),
            styles::description_font_color(is_toggleable, app).into(),
            self.wispr_highlight_index.clone(),
        )
        .with_hyperlink_font_color(appearance.theme().accent().into_solid())
        .register_default_click_handlers(|url, ctx, _| {
            ctx.dispatch_typed_action(AISettingsPageAction::HyperlinkClick(url));
        });

        column.add_child(
            Container::new(voice_input_description.finish())
                .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                .finish(),
        );

        if ai_settings.is_voice_input_enabled(app) {
            column.add_child(render_dropdown_item(
                appearance,
                "Key for Activating Voice Input",
                Some("Press and hold to activate."),
                None,
                LocalOnlyIconState::for_setting(
                    VoiceInputToggleKey::storage_key(),
                    VoiceInputToggleKey::sync_to_cloud(),
                    &mut view.local_only_icon_tooltip_states.borrow_mut(),
                    app,
                ),
                None,
                &view.voice_input_toggle_key_dropdown,
            ));
        }

        column.finish()
    }
}

impl SettingsWidget for VoiceWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "voice agent oz ai a.i. speech input natural language talk english"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        cfg!(feature = "voice_input") && UserWorkspaces::as_ref(app).is_voice_enabled()
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(
                    appearance,
                    "Voice",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            )
            .with_child(self.render_voice_section(view, appearance, app))
            .finish()
    }
}
#[derive(Default)]
struct OtherAIWidget {
    show_oz_updates_in_zero_state_toggle: SwitchStateHandle,
    use_agent_footer_toggle: SwitchStateHandle,
    show_conversation_history_toggle: SwitchStateHandle,
}

impl OtherAIWidget {
    fn create_thinking_display_mode_dropdown(
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> ViewHandle<Dropdown<AISettingsPageAction>> {
        let items: Vec<DropdownItem<AISettingsPageAction>> = ThinkingDisplayMode::iter()
            .map(|mode| {
                DropdownItem::new(
                    mode.display_name(),
                    AISettingsPageAction::SetThinkingDisplayMode(mode),
                )
            })
            .collect();

        ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.set_menu_max_height(AI_SETTINGS_DROPDOWN_MAX_HEIGHT, ctx);
            dropdown.add_items(items, ctx);
            dropdown
        })
    }
}

impl SettingsWidget for OtherAIWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "other oz updates zero state empty changelog new conversation agent what's new use agent footer toolbar layout chip chips rearrange re-arrange thinking expanded reasoning collapse never show hide conversation history"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        let is_toggleable = is_any_ai_enabled;

        let mut column = Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(
                    appearance,
                    "Other",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            );

        if FeatureFlag::AgentView.is_enabled() {
            let mut agent_view_column = Flex::column()
                .with_child(render_ai_setting_toggle::<ShouldShowOzUpdatesInZeroState>(
                    "Show Oz changelog in new conversation view",
                    AISettingsPageAction::ToggleShowOzUpdatesInZeroState,
                    *ai_settings.should_show_oz_updates_in_zero_state,
                    is_toggleable,
                    self.show_oz_updates_in_zero_state_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ))
                .with_child(render_ai_setting_toggle::<ShouldRenderUseAgentToolbarForUserCommands>(
                    "Show \"Use Agent\" footer",
                    AISettingsPageAction::ToggleUseAgentToolbar,
                    *ai_settings.should_render_use_agent_footer_for_user_commands,
                    is_toggleable,
                    self.use_agent_footer_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ))
                .with_child(render_ai_setting_description(
                    "Shows hint to use the \"Full Terminal Use\"-enabled agent in long running commands.",
                    is_toggleable,
                    app,
                ));

            if is_toggleable && FeatureFlag::AgentToolbarEditor.is_enabled() {
                agent_view_column.add_child(render_toolbar_layout_editor(
                    &view.agent_toolbar_inline_editor,
                    appearance,
                ));
            }

            column.add_child(agent_view_column.finish());
        }

        column.add_child(render_ai_setting_toggle::<ShowConversationHistory>(
            "Show conversation history in tools panel",
            AISettingsPageAction::ToggleShowConversationHistory,
            *ai_settings.show_conversation_history,
            is_toggleable,
            self.show_conversation_history_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        ));

        column.add_child(render_dropdown_item(
            appearance,
            "Agent thinking display",
            Some("Controls how reasoning/thinking traces are displayed."),
            None,
            LocalOnlyIconState::for_setting(
                ThinkingDisplayMode::storage_key(),
                ThinkingDisplayMode::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            (!is_any_ai_enabled).then(|| appearance.theme().disabled_ui_text_color()),
            &view.thinking_display_mode_dropdown,
        ));

        // TODO: OpenConversationLayoutPreference should not depend on local_fs, but it lives under the external editor settings
        // which does require local_fs. It was a mistake to put it there, but now we keep it there for backward compatibility.
        #[cfg(feature = "local_fs")]
        if FeatureFlag::OpenWarpNewSettingsModes.is_enabled() {
            use crate::util::file::external_editor::settings::OpenConversationLayoutPreference;

            column.add_child(render_dropdown_item(
                appearance,
                "Preferred layout when opening existing agent conversations",
                None,
                None,
                LocalOnlyIconState::for_setting(
                    OpenConversationLayoutPreference::storage_key(),
                    OpenConversationLayoutPreference::sync_to_cloud(),
                    &mut view.local_only_icon_tooltip_states.borrow_mut(),
                    app,
                ),
                (!is_any_ai_enabled).then(|| appearance.theme().disabled_ui_text_color()),
                &view.conversation_layout_dropdown,
            ));
        }

        column.finish()
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn cli_agent_settings_widget_id() -> &'static str {
    CLIAgentWidget::static_widget_id()
}

#[derive(Default)]
struct CLIAgentWidget {
    cli_agent_footer_toggle: SwitchStateHandle,
    auto_toggle_rich_input_toggle: SwitchStateHandle,
    auto_toggle_rich_input_info_tooltip: MouseStateHandle,
    auto_open_rich_input_on_cli_agent_start_toggle: SwitchStateHandle,
    auto_dismiss_rich_input_toggle: SwitchStateHandle,
}

impl SettingsWidget for CLIAgentWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "third party cli coding agent claude codex gemini toolbar footer layout chip chips rearrange re-arrange bar command regex auto show rich input dismiss"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);

        // The Coding Agents section is always enabled, independent of the
        // global AI toggle, because these settings control third-party coding
        // agents (Claude Code, Codex, Gemini CLI) rather than Warp's own AI.
        let cli_agent_footer_toggle = render_ai_setting_toggle::<ShouldRenderCLIAgentToolbar>(
            "Show coding agent toolbar",
            AISettingsPageAction::ToggleCLIAgentToolbar,
            *ai_settings.should_render_cli_agent_footer,
            true,
            self.cli_agent_footer_toggle.clone(),
            &view.local_only_icon_tooltip_states,
            app,
        );

        let description_fragments = vec![
            FormattedTextFragment::plain_text(
                "Show a toolbar with quick actions when running coding agents like ",
            ),
            FormattedTextFragment::inline_code("claude"),
            FormattedTextFragment::plain_text(", "),
            FormattedTextFragment::inline_code("codex"),
            FormattedTextFragment::plain_text(", or "),
            FormattedTextFragment::inline_code("gemini"),
            FormattedTextFragment::plain_text("."),
        ];

        let description = FormattedTextElement::new(
            FormattedText::new([FormattedTextLine::Line(description_fragments)]),
            appearance.ui_font_size(),
            appearance.ui_font_family(),
            appearance.monospace_font_family(),
            styles::description_font_color(true, app).into(),
            HighlightedHyperlink::default(),
        );

        let is_footer_enabled = *ai_settings.should_render_cli_agent_footer;

        let mut column = Flex::column()
            .with_child(
                build_sub_header(
                    appearance,
                    "Third party CLI agents",
                    Some(styles::header_font_color(true, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            )
            .with_child(cli_agent_footer_toggle)
            .with_child(
                Container::new(description.finish())
                    .with_margin_top(styles::DESCRIPTION_NEGATIVE_MARGIN_OFFSET)
                    .with_margin_bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                    .with_margin_right(styles::TOGGLE_WIDTH_MARGIN)
                    .finish(),
            );

        if is_footer_enabled {
            use super::settings_page::AdditionalInfo;
            use crate::settings::{
                AutoDismissRichInputAfterSubmit, AutoOpenRichInputOnCLIAgentStart,
                AutoToggleRichInput,
            };

            if FeatureFlag::CLIAgentRichInput.is_enabled() {
                // Setting 1: Auto show/hide rich input based on agent status
                let auto_show_toggle_label = render_body_item_label::<AISettingsPageAction>(
                    "Auto show/hide Rich Input based on agent status".into(),
                    Some(styles::header_font_color(true, app)),
                    Some(AdditionalInfo {
                        mouse_state: self.auto_toggle_rich_input_info_tooltip.clone(),
                        on_click_action: None,
                        secondary_text: None,
                        tooltip_override_text: Some(
                            "Requires the Warp plugin for your coding agent".to_owned(),
                        ),
                    }),
                    LocalOnlyIconState::for_setting(
                        AutoToggleRichInput::storage_key(),
                        AutoToggleRichInput::sync_to_cloud(),
                        &mut view.local_only_icon_tooltip_states.borrow_mut(),
                        app,
                    ),
                    ToggleState::Enabled,
                    appearance,
                );
                column.add_child(build_toggle_element(
                    auto_show_toggle_label,
                    render_ai_feature_switch(
                        self.auto_toggle_rich_input_toggle.clone(),
                        *ai_settings.auto_toggle_rich_input,
                        true,
                        AISettingsPageAction::ToggleAutoToggleRichInput,
                        app,
                    ),
                    appearance,
                    None,
                ));

                column.add_child(
                    render_ai_setting_toggle::<AutoOpenRichInputOnCLIAgentStart>(
                        "Auto open Rich Input when a coding agent session starts",
                        AISettingsPageAction::ToggleAutoOpenRichInputOnCLIAgentStart,
                        *ai_settings.auto_open_rich_input_on_cli_agent_start,
                        true,
                        self.auto_open_rich_input_on_cli_agent_start_toggle.clone(),
                        &view.local_only_icon_tooltip_states,
                        app,
                    ),
                );

                // Setting 2: Auto dismiss rich input after prompt submission
                column.add_child(render_ai_setting_toggle::<AutoDismissRichInputAfterSubmit>(
                    "Auto dismiss Rich Input after prompt submission",
                    AISettingsPageAction::ToggleAutoDismissRichInputAfterSubmit,
                    *ai_settings.auto_dismiss_rich_input_after_submit,
                    true,
                    self.auto_dismiss_rich_input_toggle.clone(),
                    &view.local_only_icon_tooltip_states,
                    app,
                ));
            }

            let command_list = {
                let mut list_column = Flex::column();

                list_column.add_child(
                    appearance
                        .ui_builder()
                        .span("Commands that enable the toolbar".to_string())
                        .with_style(UiComponentStyles {
                            font_size: Some(CONTENT_FONT_SIZE),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                );

                list_column
                    .add_child(ChildView::new(&view.cli_agent_footer_command_editor).finish());

                let background = appearance.theme().surface_1();
                let font_color = appearance.theme().foreground();
                let items: Vec<_> = ai_settings
                    .cli_agent_footer_enabled_commands
                    .value()
                    .keys()
                    .cloned()
                    .collect();
                let len = items.len();
                for (rev_i, pattern) in items.iter().rev().enumerate() {
                    let original_i = len - 1 - rev_i;
                    let remove_action =
                        AISettingsPageAction::RemoveCLIAgentToolbarEnabledCommand(pattern.clone());
                    let mouse_state = view
                        .cli_agent_footer_command_mouse_state_handles
                        .get(original_i)
                        .cloned()
                        .unwrap_or_default();

                    let remove_button = appearance
                        .ui_builder()
                        .close_button(16., mouse_state)
                        .build()
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(remove_action.clone());
                        })
                        .finish();

                    let label = appearance
                        .ui_builder()
                        .wrappable_text(pattern.clone(), true)
                        .with_style(UiComponentStyles {
                            font_color: Some(font_color.into_solid()),
                            font_family_id: Some(appearance.monospace_font_family()),
                            font_size: Some(appearance.ui_font_size()),
                            ..Default::default()
                        })
                        .build()
                        .finish();

                    let mut right_side =
                        Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
                    if let Some(dropdown_handle) = view
                        .cli_agent_footer_command_agent_dropdowns
                        .get(original_i)
                    {
                        right_side.add_child(
                            Container::new(ChildView::new(dropdown_handle).finish())
                                .with_margin_right(8.)
                                .finish(),
                        );
                    }
                    right_side.add_child(remove_button);

                    let row = Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                            .with_children([
                                Shrinkable::new(1., label).finish(),
                                right_side.finish(),
                            ])
                            .finish(),
                    )
                    .with_background(background)
                    .with_horizontal_padding(8.)
                    .with_vertical_padding(4.)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                    .with_margin_bottom(4.)
                    .finish();

                    list_column.add_child(row);
                }

                list_column.finish()
            };
            let command_list_description = appearance
                .ui_builder()
                .paragraph(
                    "Add regex patterns to show the coding agent toolbar for matching commands.",
                )
                .with_style(UiComponentStyles {
                    font_size: Some(appearance.ui_font_size()),
                    font_color: Some(styles::description_font_color(true, app).into()),
                    margin: Some(
                        Coords::default()
                            .top(4.)
                            .bottom(styles::DESCRIPTION_MARGIN_BOTTOM)
                            .right(styles::TOGGLE_WIDTH_MARGIN),
                    ),
                    ..Default::default()
                })
                .build()
                .finish();

            column.add_child(command_list);
            column.add_child(command_list_description);

            if FeatureFlag::AgentToolbarEditor.is_enabled() {
                column.add_child(render_toolbar_layout_editor(
                    &view.cli_agent_toolbar_inline_editor,
                    appearance,
                ));
            }
        }

        column.finish()
    }
}

/// The presentation state of the agent attribution toggle, derived from the
/// org-level [`AdminEnablementSetting`], the user's stored preference, and
/// whether AI is globally enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentAttributionToggleState {
    /// Whether the toggle is rendered in the checked state.
    pub(crate) is_enabled: bool,
    /// Whether the org has forced the value (locking the toggle with a tooltip).
    pub(crate) is_forced_by_org: bool,
    /// Whether the toggle should be rendered as non-interactive overall
    /// (forced by the org, or AI globally disabled).
    pub(crate) is_disabled: bool,
}

/// Derive the toggle state from its three inputs.
pub(crate) fn derive_agent_attribution_toggle_state(
    org_setting: &AdminEnablementSetting,
    user_pref: bool,
    is_any_ai_enabled: bool,
) -> AgentAttributionToggleState {
    let is_forced_by_org = match org_setting {
        AdminEnablementSetting::Enable | AdminEnablementSetting::Disable => true,
        AdminEnablementSetting::RespectUserSetting => false,
    };
    let is_enabled = match org_setting {
        AdminEnablementSetting::Enable => true,
        AdminEnablementSetting::Disable => false,
        AdminEnablementSetting::RespectUserSetting => user_pref,
    };
    AgentAttributionToggleState {
        is_enabled,
        is_forced_by_org,
        is_disabled: is_forced_by_org || !is_any_ai_enabled,
    }
}

#[derive(Default)]
struct AgentAttributionWidget {
    toggle: SwitchStateHandle,
}

impl SettingsWidget for AgentAttributionWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "agent attribution commit pull request co-author author credit oz warp"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);

        let org_setting = UserWorkspaces::as_ref(app).get_agent_attribution_setting();
        let state = derive_agent_attribution_toggle_state(
            &org_setting,
            *ai_settings.agent_attribution_enabled,
            is_any_ai_enabled,
        );

        let ui_builder = appearance.ui_builder();
        let toggle = if state.is_forced_by_org {
            ui_builder
                .switch(self.toggle.clone())
                .check(state.is_enabled)
                .with_tooltip(TooltipConfig {
                    text: "This option is enforced by your organization's settings and cannot be customized.".to_string(),
                    styles: ui_builder.default_tool_tip_styles(),
                })
                .disable()
                .build()
                .finish()
        } else if !is_any_ai_enabled {
            ui_builder
                .switch(self.toggle.clone())
                .check(state.is_enabled)
                .with_disabled(true)
                .build()
                .finish()
        } else {
            ui_builder
                .switch(self.toggle.clone())
                .check(state.is_enabled)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(AISettingsPageAction::ToggleAgentAttribution);
                })
                .finish()
        };

        let toggle_row = build_toggle_element(
            render_body_item_label::<AISettingsPageAction>(
                "Enable agent attribution".to_string(),
                Some(styles::header_font_color(!state.is_disabled, app)),
                None,
                LocalOnlyIconState::Hidden,
                ToggleState::Enabled,
                appearance,
            ),
            toggle,
            appearance,
            None,
        );

        Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(
                    appearance,
                    "Agent Attribution",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            )
            .with_child(toggle_row)
            .with_child(render_ai_setting_description(
                "Oz can add attribution to commit messages and pull requests it creates",
                !state.is_disabled,
                app,
            ))
            .finish()
    }
}

#[cfg(test)]
#[path = "ai_page_tests.rs"]
mod tests;

#[derive(Default)]
struct CloudAgentComputerUseWidget {
    toggle: SwitchStateHandle,
    orchestration_toggle: SwitchStateHandle,
}

impl SettingsWidget for CloudAgentComputerUseWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "oz cloud agent computer use orchestration multi-agent"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        use crate::ai::execution_profiles::{CloudAgentComputerUseState, ComputerUsePermission};

        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);

        // Determine toggle state based on workspace autonomy setting and user preference
        let CloudAgentComputerUseState {
            enabled: is_checked,
            is_forced_by_org,
        } = ComputerUsePermission::resolve_cloud_agent_state(app);

        // Toggle is disabled if forced by org settings OR if AI is globally disabled
        let is_disabled = is_forced_by_org || !is_any_ai_enabled;

        let ui_builder = appearance.ui_builder();
        let toggle = if is_forced_by_org {
            // Disabled by organization setting - show tooltip on hover
            ui_builder
                .switch(self.toggle.clone())
                .check(is_checked)
                .with_tooltip(TooltipConfig {
                    text: "This option is enforced by your organization's settings and cannot be customized.".to_string(),
                    styles: ui_builder.default_tool_tip_styles(),
                })
                .disable()
                .build()
                .finish()
        } else if !is_any_ai_enabled {
            // Disabled because AI is off globally - no tooltip needed
            ui_builder
                .switch(self.toggle.clone())
                .check(is_checked)
                .with_disabled(true)
                .build()
                .finish()
        } else {
            // Enabled - allow toggling
            ui_builder
                .switch(self.toggle.clone())
                .check(is_checked)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(AISettingsPageAction::ToggleCloudAgentComputerUse);
                })
                .finish()
        };

        let toggle_row = build_toggle_element(
            render_body_item_label::<AISettingsPageAction>(
                "Computer use in Cloud Agents".to_string(),
                Some(styles::header_font_color(!is_disabled, app)),
                None,
                LocalOnlyIconState::Hidden,
                ToggleState::Enabled,
                appearance,
            ),
            toggle,
            appearance,
            None,
        );

        let mut column = Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(
                    appearance,
                    "Experimental",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            )
            .with_child(toggle_row)
            .with_child(render_ai_setting_description(
                "Enable computer use in cloud agent conversations started from the Warp app.",
                !is_disabled,
                app,
            ));

        if FeatureFlag::Orchestration.is_enabled() {
            let ai_settings = AISettings::as_ref(app);
            column.add_child(render_ai_setting_toggle::<OrchestrationEnabled>(
                "Orchestration",
                AISettingsPageAction::ToggleOrchestration,
                *ai_settings.orchestration_enabled,
                is_any_ai_enabled,
                self.orchestration_toggle.clone(),
                &view.local_only_icon_tooltip_states,
                app,
            ));
            column.add_child(render_ai_setting_description(
                "Enable multi-agent orchestration, allowing the agent to spawn and coordinate parallel sub-agents.",
                is_any_ai_enabled,
                app,
            ));
        }

        column.finish()
    }
}

struct ProvidersWidget {
    provider_kind_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    auth_mode_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    provider_id_editor: ViewHandle<EditorView>,
    display_name_editor: ViewHandle<EditorView>,
    base_url_editor: ViewHandle<EditorView>,
    model_editor: ViewHandle<EditorView>,
    env_var_editor: ViewHandle<EditorView>,
    secret_key_editor: ViewHandle<EditorView>,
    // Capability dropdowns
    cap_tools_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    cap_vision_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    cap_structured_outputs_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    cap_prompt_caching_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    cap_reasoning_controls_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    cap_model_discovery_dropdown: ViewHandle<Dropdown<AISettingsPageAction>>,
    // Policy togges
    policy_allow_fallback: SwitchStateHandle,
    policy_redact_telemetry: SwitchStateHandle,
    // Defaults
    defaults_stream: SwitchStateHandle,
    defaults_temperature_editor: ViewHandle<EditorView>,
    defaults_top_p_editor: ViewHandle<EditorView>,
    defaults_max_output_tokens_editor: ViewHandle<EditorView>,
    defaults_timeout_ms_editor: ViewHandle<EditorView>,
    import_json_editor: ViewHandle<EditorView>,
    import_secret_editor: ViewHandle<EditorView>,
    save_button: ViewHandle<ActionButton>,
    update_button: ViewHandle<ActionButton>,
    cancel_button: ViewHandle<ActionButton>,
    action_mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
}

impl ProvidersWidget {
    fn new(
        ctx: &mut ViewContext<<Self as SettingsWidget>::View>,
        provider_id_editor: ViewHandle<EditorView>,
        display_name_editor: ViewHandle<EditorView>,
        base_url_editor: ViewHandle<EditorView>,
        model_editor: ViewHandle<EditorView>,
        env_var_editor: ViewHandle<EditorView>,
        secret_key_editor: ViewHandle<EditorView>,
        defaults_temperature_editor: ViewHandle<EditorView>,
        defaults_top_p_editor: ViewHandle<EditorView>,
        defaults_max_output_tokens_editor: ViewHandle<EditorView>,
        defaults_timeout_ms_editor: ViewHandle<EditorView>,
    ) -> Self {
        let provider_kind_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.add_items(
                vec![
                    DropdownItem::new(
                        "OpenAI",
                        AISettingsPageAction::SetProviderDraftKind(ProviderKind::OpenAI),
                    ),
                    DropdownItem::new(
                        "Anthropic",
                        AISettingsPageAction::SetProviderDraftKind(ProviderKind::Anthropic),
                    ),
                    DropdownItem::new(
                        "OpenAI-compatible",
                        AISettingsPageAction::SetProviderDraftKind(ProviderKind::OpenAICompatible),
                    ),
                    DropdownItem::new(
                        "Anthropic-compatible",
                        AISettingsPageAction::SetProviderDraftKind(
                            ProviderKind::AnthropicCompatible,
                        ),
                    ),
                ],
                ctx,
            );
            dropdown.set_selected_by_action(
                AISettingsPageAction::SetProviderDraftKind(ProviderKind::OpenAICompatible),
                ctx,
            );
            dropdown
        });

        let auth_mode_dropdown = ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
            dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
            dropdown.add_items(
                vec![
                    DropdownItem::new(
                        ProviderDraftAuthMode::None.display_name(),
                        AISettingsPageAction::SetProviderDraftAuthMode(ProviderDraftAuthMode::None),
                    ),
                    DropdownItem::new(
                        ProviderDraftAuthMode::EnvApiKey.display_name(),
                        AISettingsPageAction::SetProviderDraftAuthMode(
                            ProviderDraftAuthMode::EnvApiKey,
                        ),
                    ),
                    DropdownItem::new(
                        ProviderDraftAuthMode::KeychainApiKey.display_name(),
                        AISettingsPageAction::SetProviderDraftAuthMode(
                            ProviderDraftAuthMode::KeychainApiKey,
                        ),
                    ),
                    DropdownItem::new(
                        ProviderDraftAuthMode::KeychainBearerToken.display_name(),
                        AISettingsPageAction::SetProviderDraftAuthMode(
                            ProviderDraftAuthMode::KeychainBearerToken,
                        ),
                    ),
                ],
                ctx,
            );
            dropdown.set_selected_by_action(
                AISettingsPageAction::SetProviderDraftAuthMode(ProviderDraftAuthMode::None),
                ctx,
            );
            dropdown
        });

        let save_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Save provider", SecondaryTheme)
                .with_icon(Icon::Plus)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AISettingsPageAction::SaveProviderDraft);
                })
        });

        let update_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Update provider", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AISettingsPageAction::SaveProviderDraft);
                })
        });

        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AISettingsPageAction::CancelProviderEdit);
                })
        });

        // Capability dropdowns
        let cap_dropdown = |ctx: &mut ViewContext<AISettingsPageView>,
                            _label: &'static str,
                            cap: ProviderDraftCapability| {
            ctx.add_typed_action_view(|ctx| {
                let mut dropdown = Dropdown::new(ctx);
                dropdown.set_top_bar_max_width(AI_SETTINGS_DROPDOWN_WIDTH);
                dropdown.set_menu_width(AI_SETTINGS_DROPDOWN_WIDTH, ctx);
                dropdown.add_items(
                    vec![
                        DropdownItem::new(
                            CapabilitySettingDraft::Auto.display_name(),
                            AISettingsPageAction::SetProviderDraftCapability(
                                cap.clone(),
                                CapabilitySettingDraft::Auto,
                            ),
                        ),
                        DropdownItem::new(
                            CapabilitySettingDraft::On.display_name(),
                            AISettingsPageAction::SetProviderDraftCapability(
                                cap.clone(),
                                CapabilitySettingDraft::On,
                            ),
                        ),
                        DropdownItem::new(
                            CapabilitySettingDraft::Off.display_name(),
                            AISettingsPageAction::SetProviderDraftCapability(
                                cap.clone(),
                                CapabilitySettingDraft::Off,
                            ),
                        ),
                    ],
                    ctx,
                );
                dropdown.set_selected_by_action(
                    AISettingsPageAction::SetProviderDraftCapability(
                        cap.clone(),
                        CapabilitySettingDraft::Auto,
                    ),
                    ctx,
                );
                dropdown
            })
        };

        let cap_tools_dropdown = cap_dropdown(ctx, "Tools", ProviderDraftCapability::Tools);
        let cap_vision_dropdown = cap_dropdown(ctx, "Vision", ProviderDraftCapability::Vision);
        let cap_structured_outputs_dropdown = cap_dropdown(
            ctx,
            "Structured outputs",
            ProviderDraftCapability::StructuredOutputs,
        );
        let cap_prompt_caching_dropdown = cap_dropdown(
            ctx,
            "Prompt caching",
            ProviderDraftCapability::PromptCaching,
        );
        let cap_reasoning_controls_dropdown = cap_dropdown(
            ctx,
            "Reasoning controls",
            ProviderDraftCapability::ReasoningControls,
        );
        let cap_model_discovery_dropdown = cap_dropdown(
            ctx,
            "Model discovery",
            ProviderDraftCapability::ModelDiscovery,
        );

        // Policy toggles
        let policy_allow_fallback = SwitchStateHandle::default();
        let policy_redact_telemetry = SwitchStateHandle::default();

        // Defaults
        let defaults_stream = SwitchStateHandle::default();
        let import_json_editor = Self::new_import_editor(ctx);
        let import_secret_editor = Self::new_import_secret_editor(ctx);

        Self {
            provider_kind_dropdown,
            auth_mode_dropdown,
            provider_id_editor,
            display_name_editor,
            base_url_editor,
            model_editor,
            env_var_editor,
            secret_key_editor,
            cap_tools_dropdown,
            cap_vision_dropdown,
            cap_structured_outputs_dropdown,
            cap_prompt_caching_dropdown,
            cap_reasoning_controls_dropdown,
            cap_model_discovery_dropdown,
            policy_allow_fallback,
            policy_redact_telemetry,
            defaults_stream,
            defaults_temperature_editor,
            defaults_top_p_editor,
            defaults_max_output_tokens_editor,
            defaults_timeout_ms_editor,
            import_json_editor,
            import_secret_editor,
            save_button,
            update_button,
            cancel_button,
            action_mouse_states: RefCell::new(HashMap::new()),
        }
    }

    fn new_import_editor(ctx: &mut ViewContext<AISettingsPageView>) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = EditorOptions {
                autogrow: true,
                soft_wrap: true,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::new(options, ctx);
            editor.set_placeholder_text("Paste a redacted provider JSON export", ctx);
            editor
        });
        ctx.subscribe_to_view(&editor, |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Edited(_) | EditorEvent::Blurred) {
                ctx.dispatch_typed_action(&AISettingsPageAction::SetImportJson(
                    editor.as_ref(ctx).buffer_text(ctx),
                ));
            }
        });
        editor
    }

    fn new_import_secret_editor(
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                is_password: true,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("Secret value", ctx);
            editor
        });
        ctx.subscribe_to_view(&editor, |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Edited(_) | EditorEvent::Blurred) {
                ctx.dispatch_typed_action(&AISettingsPageAction::SetImportSecretValue(
                    editor.as_ref(ctx).buffer_text(ctx),
                ));
            }
        });
        editor
    }

    fn render_input(
        label: &'static str,
        editor: ViewHandle<EditorView>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let label = Text::new_inline(label, appearance.ui_font_family(), CONTENT_FONT_SIZE)
            .with_color(styles::header_font_color(true, app).into())
            .finish();
        let input = appearance
            .ui_builder()
            .text_input(editor)
            .with_style(UiComponentStyles {
                padding: Some(Coords {
                    top: 10.,
                    bottom: 10.,
                    left: 12.,
                    right: 12.,
                }),
                background: Some(appearance.theme().surface_2().into()),
                ..Default::default()
            })
            .build()
            .finish();
        Flex::column()
            .with_spacing(6.)
            .with_child(label)
            .with_child(input)
            .finish()
    }

    fn render_text_button(
        &self,
        key: String,
        label: &'static str,
        action: AISettingsPageAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mouse_state = self
            .action_mouse_states
            .borrow_mut()
            .entry(key)
            .or_default()
            .clone();
        appearance
            .ui_builder()
            .button(ButtonVariant::Text, mouse_state)
            .with_text_label(label.to_string())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(action.clone());
            })
            .finish()
    }

    fn render_capability_dropdown(
        label: &'static str,
        dropdown: &ViewHandle<Dropdown<AISettingsPageAction>>,
        _current: CapabilitySettingDraft,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let label_el = Text::new_inline(label, appearance.ui_font_family(), CONTENT_FONT_SIZE)
            .with_color(styles::header_font_color(true, app).into())
            .finish();
        let dropdown_el = Container::new(ChildView::new(dropdown).finish())
            .with_margin_top(4.)
            .finish();
        Flex::column()
            .with_spacing(4.)
            .with_child(label_el)
            .with_child(dropdown_el)
            .finish()
    }

    fn render_policy_toggle(
        label: &'static str,
        is_enabled: bool,
        action: AISettingsPageAction,
        state: SwitchStateHandle,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new_inline(label, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                    .with_color(styles::header_font_color(true, app).into())
                    .finish(),
            )
            .with_child(
                appearance
                    .ui_builder()
                    .switch(state)
                    .check(is_enabled)
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(action.clone());
                    })
                    .finish(),
            )
            .finish()
    }

    fn render_defaults_toggle(
        label: &'static str,
        is_enabled: bool,
        state: SwitchStateHandle,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        Flex::row()
            .with_spacing(8.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new_inline(label, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                    .with_color(styles::header_font_color(true, app).into())
                    .finish(),
            )
            .with_child(
                appearance
                    .ui_builder()
                    .switch(state)
                    .check(is_enabled)
                    .build()
                    .finish(),
            )
            .finish()
    }

    fn render_provider_row(
        &self,
        provider: &::ai::provider_registry::ProviderProfile,
        registry: &ProviderRegistry,
        validation_state: Option<&ProviderValidationUiState>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_default = registry.defaults.agent_provider.as_deref() == Some(provider.id.as_str());
        let status = if provider.enabled {
            "Enabled"
        } else {
            "Disabled"
        };
        let local = if provider.policy.local_only == Some(true) {
            " local"
        } else {
            ""
        };
        let model_ids = provider.model_ids();
        let default_model = provider
            .defaults
            .model
            .clone()
            .or_else(|| model_ids.first().cloned())
            .unwrap_or_else(|| "auto".to_string());
        let model_summary = if model_ids.is_empty() {
            "No models configured".to_string()
        } else {
            format!("Models: {}", model_ids.join(", "))
        };
        let kind = provider_kind_label(&provider.kind);
        let connection_hint = if provider.kind.is_direct() {
            if provider.policy.local_only == Some(true) {
                format!("Direct API ({kind}, local) — sends requests directly to {kind}")
            } else {
                format!("Direct API — sends requests directly to {kind}")
            }
        } else {
            "Warp-hosted — requests routed through Warp".to_string()
        };
        let title = format!("{} ({})", provider.display_name, provider.id);
        let detail = format!("{connection_hint} - {status}{local} - {model_summary}");

        let mut actions = Flex::row()
            .with_spacing(8.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        if provider.id != WARP_HOSTED_PROVIDER_ID {
            actions.add_child(self.render_text_button(
                format!("{}:edit", provider.id),
                "Edit",
                AISettingsPageAction::EditProvider(provider.id.clone()),
                appearance,
            ));
            actions.add_child(self.render_text_button(
                format!("{}:validate", provider.id),
                "Validate",
                AISettingsPageAction::ValidateProvider(provider.id.clone()),
                appearance,
            ));
            actions.add_child(self.render_text_button(
                format!("{}:discover", provider.id),
                "Discover models",
                AISettingsPageAction::DiscoverProviderModels(provider.id.clone()),
                appearance,
            ));
            actions.add_child(self.render_text_button(
                format!("{}:toggle", provider.id),
                if provider.enabled {
                    "Disable"
                } else {
                    "Enable"
                },
                AISettingsPageAction::ToggleProviderEnabled(provider.id.clone()),
                appearance,
            ));
            actions.add_child(self.render_text_button(
                format!("{}:duplicate", provider.id),
                "Duplicate",
                AISettingsPageAction::DuplicateProvider(provider.id.clone()),
                appearance,
            ));
            actions.add_child(self.render_text_button(
                format!("{}:export", provider.id),
                "Export",
                AISettingsPageAction::ExportProvider(provider.id.clone()),
                appearance,
            ));
            if !is_legacy_provider_id(&provider.id) {
                actions.add_child(self.render_text_button(
                    format!("{}:delete", provider.id),
                    "Delete",
                    AISettingsPageAction::DeleteProvider(provider.id.clone()),
                    appearance,
                ));
            }
        }
        if !is_default && provider.enabled {
            actions.add_child(self.render_text_button(
                format!("{}:default", provider.id),
                "Set default",
                AISettingsPageAction::SetDefaultProvider {
                    provider_id: provider.id.clone(),
                    model_id: default_model,
                },
                appearance,
            ));
        }

        let default_label = if is_default {
            Some(
                Text::new_inline("Default", appearance.ui_font_family(), CONTENT_FONT_SIZE)
                    .with_color(appearance.theme().accent().into())
                    .finish(),
            )
        } else {
            None
        };

        let mut header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Expanded::new(
                    1.,
                    Text::new_inline(title, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(styles::header_font_color(provider.enabled, app).into())
                        .finish(),
                )
                .finish(),
            );
        if let Some(default_label) = default_label {
            header.add_child(default_label);
        }

        let mut content = Flex::column()
            .with_spacing(8.)
            .with_child(header.finish())
            .with_child(
                Text::new(detail, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                    .with_color(styles::description_font_color(provider.enabled, app).into())
                    .soft_wrap(true)
                    .finish(),
            );
        if let Some(validation_state) = validation_state {
            content.add_child(render_provider_validation_state(
                validation_state,
                provider.kind.is_compatible(),
                appearance,
                app,
            ));
        }
        content.add_child(actions.finish());

        Container::new(content.finish())
            .with_uniform_padding(12.)
            .with_background(appearance.theme().surface_2())
            .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish()
    }

    fn render_import_panel(
        &self,
        view: &AISettingsPageView,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Option<Box<dyn Element>> {
        if !view.import_dialog_visible {
            return None;
        }

        let mut panel = Flex::column().with_spacing(10.);
        panel.add_child(
            Text::new_inline(
                "Import providers",
                appearance.ui_font_family(),
                CONTENT_FONT_SIZE,
            )
            .with_style(Properties::default().weight(Weight::Semibold))
            .with_color(appearance.theme().active_ui_text_color().into())
            .finish(),
        );
        panel.add_child(
            Text::new(
                "Paste a redacted provider export. Keychain secrets are not imported until you enter a replacement secret value.",
                appearance.ui_font_family(),
                CONTENT_FONT_SIZE,
            )
            .with_color(styles::description_font_color(true, app).into())
            .soft_wrap(true)
            .finish(),
        );

        if let Some((provider_id, _)) = &view.import_collision {
            panel.add_child(self.render_import_notice(
                format!("Provider ID '{provider_id}' already exists."),
                "Choose how to handle this imported provider.".to_string(),
                appearance,
                app,
            ));
            panel.add_child(
                Flex::row()
                    .with_spacing(8.)
                    .with_child(self.render_text_button(
                        format!("import:{provider_id}:overwrite"),
                        "Overwrite",
                        AISettingsPageAction::ResolveImportCollision {
                            provider_id: provider_id.clone(),
                            action: ImportCollisionAction::Overwrite,
                        },
                        appearance,
                    ))
                    .with_child(self.render_text_button(
                        format!("import:{provider_id}:rename"),
                        "Rename",
                        AISettingsPageAction::ResolveImportCollision {
                            provider_id: provider_id.clone(),
                            action: ImportCollisionAction::Rename,
                        },
                        appearance,
                    ))
                    .with_child(self.render_text_button(
                        format!("import:{provider_id}:skip"),
                        "Skip",
                        AISettingsPageAction::ResolveImportCollision {
                            provider_id: provider_id.clone(),
                            action: ImportCollisionAction::Skip,
                        },
                        appearance,
                    ))
                    .finish(),
            );
        } else if let Some(secret_prompt) = &view.import_secret_prompt {
            panel.add_child(self.render_import_notice(
                format!("Secret required for '{}'.", secret_prompt.provider_id),
                secret_prompt.secret_hint.clone(),
                appearance,
                app,
            ));
            panel.add_child(Self::render_input(
                "Secret value",
                self.import_secret_editor.clone(),
                appearance,
                app,
            ));
            panel.add_child(
                Flex::row()
                    .with_spacing(8.)
                    .with_child(self.render_text_button(
                        format!("import:{}:submit-secret", secret_prompt.provider_id),
                        "Store secret and import",
                        AISettingsPageAction::SubmitImportSecret {
                            provider_id: secret_prompt.provider_id.clone(),
                            secret_value: view.import_secret_value.clone(),
                        },
                        appearance,
                    ))
                    .finish(),
            );
        } else {
            panel.add_child(Self::render_input(
                "Provider export JSON",
                self.import_json_editor.clone(),
                appearance,
                app,
            ));
            panel.add_child(
                Flex::row()
                    .with_spacing(8.)
                    .with_child(self.render_text_button(
                        "import:execute".to_string(),
                        "Import",
                        AISettingsPageAction::ExecuteImport,
                        appearance,
                    ))
                    .with_child(self.render_text_button(
                        "import:cancel".to_string(),
                        "Cancel",
                        AISettingsPageAction::HideImportDialog,
                        appearance,
                    ))
                    .finish(),
            );
        }

        Some(
            Container::new(panel.finish())
                .with_uniform_padding(12.)
                .with_background(appearance.theme().surface_1())
                .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                .finish(),
        )
    }

    fn render_import_notice(
        &self,
        title: String,
        detail: String,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        Container::new(
            Flex::column()
                .with_spacing(4.)
                .with_child(
                    Text::new_inline(title, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(appearance.theme().active_ui_text_color().into())
                        .finish(),
                )
                .with_child(
                    Text::new(detail, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_color(styles::description_font_color(true, app).into())
                        .soft_wrap(true)
                        .finish(),
                )
                .finish(),
        )
        .with_uniform_padding(10.)
        .with_background(appearance.theme().surface_2())
        .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish()
    }
}

impl SettingsWidget for ProvidersWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "providers byo api key local models openai anthropic ollama lm studio compatible endpoint env var"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let registry = ApiKeyManager::as_ref(app).provider_registry_from_legacy_keys(Some(
            AISettings::as_ref(app).provider_registry.value().clone(),
        ));
        let mut providers = registry.providers.values().collect_vec();
        providers.sort_by_key(|provider| {
            (
                provider.id == WARP_HOSTED_PROVIDER_ID,
                provider.display_name.clone(),
            )
        });

        let mut list = Flex::column().with_spacing(8.);
        for provider in providers {
            list.add_child(self.render_provider_row(
                provider,
                &registry,
                view.provider_validation_states.get(&provider.id),
                appearance,
                app,
            ));
        }
        let provider_actions = Flex::row()
            .with_spacing(8.)
            .with_child(self.render_text_button(
                "providers:add".to_string(),
                "Add provider",
                AISettingsPageAction::AddProvider,
                appearance,
            ))
            .with_child(self.render_text_button(
                "providers:import".to_string(),
                "Import providers",
                AISettingsPageAction::ShowImportDialog,
                appearance,
            ))
            .with_child(self.render_text_button(
                "providers:export-all".to_string(),
                "Export all",
                AISettingsPageAction::ExportAllProviders,
                appearance,
            ))
            .finish();

        // Update save button label based on editing mode
        let is_editing = view.provider_draft.is_editing();

        // Build form children
        let mut form = Flex::column().with_spacing(12.);

        // Provider kind + auth mode dropdowns
        form.add_child(
            Flex::row()
                .with_spacing(12.)
                .with_child(
                    Expanded::new(1., ChildView::new(&self.provider_kind_dropdown).finish())
                        .finish(),
                )
                .with_child(
                    Expanded::new(1., ChildView::new(&self.auth_mode_dropdown).finish()).finish(),
                )
                .finish(),
        );

        // Provider ID (only shown when creating, not editing)
        if !is_editing {
            form.add_child(
                Flex::row()
                    .with_spacing(12.)
                    .with_child(
                        Expanded::new(
                            1.,
                            Self::render_input(
                                "Provider ID",
                                self.provider_id_editor.clone(),
                                appearance,
                                app,
                            ),
                        )
                        .finish(),
                    )
                    .with_child(
                        Expanded::new(
                            1.,
                            Self::render_input(
                                "Display name",
                                self.display_name_editor.clone(),
                                appearance,
                                app,
                            ),
                        )
                        .finish(),
                    )
                    .finish(),
            );
        } else {
            form.add_child(
                Flex::row()
                    .with_spacing(12.)
                    .with_child(
                        Expanded::new(
                            1.,
                            Self::render_input(
                                "Display name",
                                self.display_name_editor.clone(),
                                appearance,
                                app,
                            ),
                        )
                        .finish(),
                    )
                    // Show read-only ID label when editing
                    .with_child(
                        Expanded::new(
                            1.,
                            Flex::column()
                                .with_spacing(6.)
                                .with_child(
                                    Text::new_inline(
                                        "Provider ID",
                                        appearance.ui_font_family(),
                                        CONTENT_FONT_SIZE,
                                    )
                                    .with_color(styles::header_font_color(true, app).into())
                                    .finish(),
                                )
                                .with_child(
                                    Text::new_inline(
                                        view.provider_draft.id.clone(),
                                        appearance.ui_font_family(),
                                        CONTENT_FONT_SIZE,
                                    )
                                    .with_color(appearance.theme().disabled_ui_text_color().into())
                                    .finish(),
                                )
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            );
        }

        form.add_child(Self::render_input(
            "Base URL",
            self.base_url_editor.clone(),
            appearance,
            app,
        ));
        form.add_child(
            Flex::row()
                .with_spacing(12.)
                .with_child(
                    Expanded::new(
                        1.,
                        Self::render_input(
                            "Manual model ID",
                            self.model_editor.clone(),
                            appearance,
                            app,
                        ),
                    )
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        match view.provider_draft.auth_mode {
                            ProviderDraftAuthMode::KeychainApiKey
                            | ProviderDraftAuthMode::KeychainBearerToken => Self::render_input(
                                if is_editing {
                                    "Replace secret key"
                                } else {
                                    "Secret key"
                                },
                                self.secret_key_editor.clone(),
                                appearance,
                                app,
                            ),
                            ProviderDraftAuthMode::EnvApiKey => Self::render_input(
                                "API key env var",
                                self.env_var_editor.clone(),
                                appearance,
                                app,
                            ),
                            ProviderDraftAuthMode::None => Flex::column().finish(),
                        },
                    )
                    .finish(),
                )
                .finish(),
        );

        // Capabilities section
        form.add_child(
            Flex::column()
                .with_spacing(8.)
                .with_child(
                    Text::new_inline(
                        "Capabilities",
                        appearance.ui_font_family(),
                        CONTENT_FONT_SIZE,
                    )
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .with_color(appearance.theme().active_ui_text_color().into())
                    .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_spacing(12.)
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_capability_dropdown(
                                    "Tools",
                                    &self.cap_tools_dropdown,
                                    view.provider_draft.cap_tools.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_capability_dropdown(
                                    "Vision",
                                    &self.cap_vision_dropdown,
                                    view.provider_draft.cap_vision.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_capability_dropdown(
                                    "Structured outputs",
                                    &self.cap_structured_outputs_dropdown,
                                    view.provider_draft.cap_structured_outputs.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_spacing(12.)
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_capability_dropdown(
                                    "Prompt caching",
                                    &self.cap_prompt_caching_dropdown,
                                    view.provider_draft.cap_prompt_caching.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_capability_dropdown(
                                    "Reasoning controls",
                                    &self.cap_reasoning_controls_dropdown,
                                    view.provider_draft.cap_reasoning_controls.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_capability_dropdown(
                                    "Model discovery",
                                    &self.cap_model_discovery_dropdown,
                                    view.provider_draft.cap_model_discovery.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .finish(),
        );

        // Policy section
        form.add_child(
            Flex::column()
                .with_spacing(8.)
                .with_child(
                    Text::new_inline("Policy", appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(appearance.theme().active_ui_text_color().into())
                        .finish(),
                )
                .with_child(Self::render_policy_toggle(
                    "Fallback to Warp hosted",
                    view.provider_draft.allow_fallback,
                    AISettingsPageAction::SetProviderDraftPolicy(
                        ProviderDraftPolicy::AllowFallback,
                        true,
                    ),
                    self.policy_allow_fallback.clone(),
                    appearance,
                    app,
                ))
                .with_child(Self::render_policy_toggle(
                    "Redact telemetry",
                    view.provider_draft.redact_telemetry,
                    AISettingsPageAction::SetProviderDraftPolicy(
                        ProviderDraftPolicy::RedactTelemetry,
                        true,
                    ),
                    self.policy_redact_telemetry.clone(),
                    appearance,
                    app,
                ))
                .finish(),
        );

        // Defaults section
        form.add_child(
            Flex::column()
                .with_spacing(8.)
                .with_child(
                    Text::new_inline("Defaults", appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(appearance.theme().active_ui_text_color().into())
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_spacing(12.)
                        .with_child(Self::render_defaults_toggle(
                            "Stream",
                            view.provider_draft.stream,
                            self.defaults_stream.clone(),
                            appearance,
                            app,
                        ))
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_input(
                                    "Temperature",
                                    self.defaults_temperature_editor.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_input(
                                    "Top P",
                                    self.defaults_top_p_editor.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_spacing(12.)
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_input(
                                    "Max output tokens",
                                    self.defaults_max_output_tokens_editor.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .with_child(
                            Expanded::new(
                                1.,
                                Self::render_input(
                                    "Timeout (ms)",
                                    self.defaults_timeout_ms_editor.clone(),
                                    appearance,
                                    app,
                                ),
                            )
                            .finish(),
                        )
                        .finish(),
                )
                .finish(),
        );

        // Action buttons: Cancel (editing only) + Save/Update
        if is_editing {
            form.add_child(
                Flex::row()
                    .with_spacing(8.)
                    .with_child(ChildView::new(&self.cancel_button).finish())
                    .with_child(ChildView::new(&self.update_button).finish())
                    .finish(),
            );
        } else {
            form.add_child(ChildView::new(&self.save_button).finish());
        }
        let form = form.finish();

        let mut providers_section = Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(appearance, "Providers", None)
                    .with_margin_bottom(8.)
                    .finish(),
            )
            .with_child(render_ai_setting_description(
                "API providers connect directly to external AI services via HTTP. They are different from CLI agents, which run as separate processes on your machine. Secrets are stored in your OS keychain; this form supports no-auth local endpoints, API keys from environment variables, and API keys or bearer tokens from secure storage.",
                true,
                app,
            ))
            .with_child(Container::new(provider_actions).with_margin_bottom(8.).finish());
        if let Some(import_panel) = self.render_import_panel(view, appearance, app) {
            providers_section
                .add_child(Container::new(import_panel).with_margin_bottom(8.).finish());
        }
        providers_section
            .with_child(list.finish())
            .with_child(
                Container::new(form)
                    .with_margin_top(8.)
                    .with_uniform_padding(12.)
                    .with_background(appearance.theme().surface_1())
                    .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                    .finish(),
            )
            .finish()
    }
}

fn render_provider_validation_state(
    state: &ProviderValidationUiState,
    is_compatible_provider: bool,
    appearance: &Appearance,
    app: &AppContext,
) -> Box<dyn Element> {
    let color = if state.is_success() {
        appearance.theme().accent()
    } else {
        styles::description_font_color(true, app)
    };
    let mut detail = state.detail();
    if matches!(state, ProviderValidationUiState::DiscoveryFailed(_)) && is_compatible_provider {
        detail.push_str(" You can keep using a compatible endpoint with manual model IDs.");
    }

    Container::new(
        Flex::column()
            .with_spacing(4.)
            .with_child(
                Text::new_inline(
                    state.title(),
                    appearance.ui_font_family(),
                    CONTENT_FONT_SIZE,
                )
                .with_style(Properties::default().weight(Weight::Semibold))
                .with_color(color.into())
                .finish(),
            )
            .with_child(
                Text::new(detail, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                    .with_color(styles::description_font_color(true, app).into())
                    .soft_wrap(true)
                    .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(10.)
    .with_background(appearance.theme().surface_1())
    .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .finish()
}

fn provider_kind_label(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::WarpHosted => "Warp-hosted",
        ProviderKind::OpenAI => "OpenAI",
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::OpenAICompatible => "OpenAI-compatible",
        ProviderKind::AnthropicCompatible => "Anthropic-compatible",
    }
}

fn default_base_url_for_kind(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::WarpHosted => "",
        ProviderKind::OpenAI => "https://api.openai.com/v1",
        ProviderKind::Anthropic => "https://api.anthropic.com/v1",
        ProviderKind::OpenAICompatible | ProviderKind::AnthropicCompatible => "",
    }
}

fn provider_id_from_text(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn unique_provider_id(
    providers: &BTreeMap<String, ::ai::provider_registry::ProviderProfile>,
    base: &str,
) -> String {
    let base = provider_id_from_text(base);
    let base = if base.is_empty() {
        "provider".to_string()
    } else {
        base
    };
    if !providers.contains_key(&base) {
        return base;
    }
    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if !providers.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded provider id suffix search should always return")
}

fn is_legacy_provider_id(provider_id: &str) -> bool {
    matches!(
        provider_id,
        LEGACY_OPENAI_PROVIDER_ID | LEGACY_ANTHROPIC_PROVIDER_ID
    )
}

fn provider_registry_with_draft(
    draft: &ProviderDraft,
    api_keys: &ApiKeys,
    provider_registry: &ProviderRegistry,
    ctx: &mut ViewContext<AISettingsPageView>,
) -> Option<ProviderRegistry> {
    let id = provider_id_from_text(&draft.id);
    if id.is_empty() {
        return None;
    }
    let display_name = if draft.display_name.is_empty() {
        id.clone()
    } else {
        draft.display_name.clone()
    };
    let base_url = if draft.base_url.is_empty() {
        default_base_url_for_kind(&draft.kind).to_string()
    } else {
        draft.base_url.clone()
    };

    // When editing, preserve the existing secret reference if the user didn't re-enter it.
    let existing_secret_ref = draft.editing_provider_id.as_ref().and_then(|edit_id| {
        provider_registry
            .providers
            .get(edit_id)
            .and_then(|p| match &p.auth {
                AuthConfig::ApiKey {
                    source: AuthSource::Keychain,
                    secret_ref: Some(ref_),
                    ..
                }
                | AuthConfig::BearerToken {
                    source: AuthSource::Keychain,
                    secret_ref: ref_,
                } => Some(ref_.clone()),
                _ => None,
            })
    });

    let auth = match draft.auth_mode {
        ProviderDraftAuthMode::None => AuthConfig::None,
        ProviderDraftAuthMode::EnvApiKey => {
            let env_var = draft.env_var.trim();
            if env_var.is_empty() {
                return None;
            }
            AuthConfig::ApiKey {
                source: AuthSource::Env,
                secret_ref: None,
                env_var: Some(env_var.to_string()),
            }
        }
        ProviderDraftAuthMode::KeychainApiKey => {
            if draft.secret_key.trim().is_empty() {
                // When editing, keep the existing secret reference.
                if let Some(ref_) = &existing_secret_ref {
                    AuthConfig::ApiKey {
                        source: AuthSource::Keychain,
                        secret_ref: Some(ref_.clone()),
                        env_var: None,
                    }
                } else {
                    return None;
                }
            } else {
                let secret_ref = format!("keychain://warp/providers/{id}");
                if let Err(e) = ctx
                    .secure_storage()
                    .write_value(&secret_ref, draft.secret_key.trim())
                {
                    log::error!("Failed to write provider API key to secure storage: {e:#}");
                    return None;
                }
                AuthConfig::ApiKey {
                    source: AuthSource::Keychain,
                    secret_ref: Some(secret_ref),
                    env_var: None,
                }
            }
        }
        ProviderDraftAuthMode::KeychainBearerToken => {
            if draft.secret_key.trim().is_empty() {
                // When editing, keep the existing secret reference.
                if let Some(ref_) = &existing_secret_ref {
                    AuthConfig::BearerToken {
                        source: AuthSource::Keychain,
                        secret_ref: ref_.clone(),
                    }
                } else {
                    return None;
                }
            } else {
                let secret_ref = format!("keychain://warp/providers/{id}");
                if let Err(e) = ctx
                    .secure_storage()
                    .write_value(&secret_ref, draft.secret_key.trim())
                {
                    log::error!("Failed to write bearer token to secure storage: {e:#}");
                    return None;
                }
                AuthConfig::BearerToken {
                    source: AuthSource::Keychain,
                    secret_ref,
                }
            }
        }
    };

    let mut registry = ProviderRegistry::from_existing(Some(provider_registry.clone()), api_keys);
    let mut profile = ::ai::provider_registry::ProviderProfile::direct(
        id.clone(),
        draft.kind.clone(),
        display_name,
        Some(base_url),
        auth,
    );
    profile.capabilities = draft.capabilities();
    profile.policy = draft.policy();
    profile.defaults = draft.defaults();
    if !draft.model.trim().is_empty() {
        profile.discovery = ProviderDiscovery {
            strategy: DiscoveryStrategy::ManualOnly,
            fallback_models: vec![draft.model.trim().to_string()],
        };
    }
    registry.upsert_provider(profile);
    Some(registry)
}

fn provider_profile_for_action(provider_id: &str, ctx: &AppContext) -> Option<ProviderProfile> {
    ApiKeyManager::as_ref(ctx)
        .provider_registry_from_legacy_keys(Some(
            AISettings::as_ref(ctx).provider_registry.value().clone(),
        ))
        .providers
        .get(provider_id)
        .cloned()
}

fn provider_secret_values(
    profile: &ProviderProfile,
    legacy_keys: &ApiKeys,
    ctx: &AppContext,
) -> HashMap<String, String> {
    let mut values = HashMap::new();
    if let Some(openai) = &legacy_keys.openai {
        values.insert("legacy://AiApiKeys/openai".to_string(), openai.clone());
    }
    if let Some(anthropic) = &legacy_keys.anthropic {
        values.insert(
            "legacy://AiApiKeys/anthropic".to_string(),
            anthropic.clone(),
        );
    }

    match &profile.auth {
        AuthConfig::ApiKey {
            source: AuthSource::Keychain,
            secret_ref: Some(secret_ref),
            ..
        }
        | AuthConfig::BearerToken { secret_ref, .. } => {
            if !secret_ref.starts_with("legacy://") {
                if let Ok(value) = ctx.secure_storage().read_value(secret_ref) {
                    values.insert(secret_ref.clone(), value);
                }
            }
        }
        AuthConfig::None
        | AuthConfig::ApiKey {
            source: AuthSource::Env,
            ..
        }
        | AuthConfig::ApiKey {
            source: AuthSource::Keychain,
            secret_ref: None,
            ..
        } => {}
    }

    values
}

fn provider_secret_value(ref_: &str, values: &HashMap<String, String>) -> Option<String> {
    values
        .get(ref_)
        .cloned()
        .or_else(|| std::env::var(ref_).ok())
}

fn update_provider_registry(
    ctx: &mut ViewContext<AISettingsPageView>,
    update: impl FnOnce(&mut ProviderRegistry),
) {
    let mut registry = ApiKeyManager::as_ref(ctx).provider_registry_from_legacy_keys(Some(
        AISettings::as_ref(ctx).provider_registry.value().clone(),
    ));
    update(&mut registry);
    AISettings::handle(ctx).update(ctx, |settings, ctx| {
        report_if_error!(settings.provider_registry.set_value(registry, ctx));
    });
}

struct AwsBedrockWidget {
    aws_auth_refresh_command_editor: ViewHandle<EditorView>,
    aws_auth_refresh_profile_editor: ViewHandle<EditorView>,
    credentials_enabled_toggle: SwitchStateHandle,
    auto_login_toggle: SwitchStateHandle,
    refresh_credentials_button: ViewHandle<ActionButton>,
}

impl AwsBedrockWidget {
    fn new(ctx: &mut ViewContext<<Self as SettingsWidget>::View>) -> Self {
        let ai_settings = AISettings::as_ref(ctx);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(ctx);

        let aws_auth_refresh_command = ai_settings.aws_bedrock_auth_refresh_command.value().clone();
        let aws_auth_refresh_profile = ai_settings.aws_bedrock_profile.value().clone();
        let is_usage_enabled = is_any_ai_enabled
            && UserWorkspaces::as_ref(ctx).is_aws_bedrock_credentials_enabled(ctx);

        let aws_auth_refresh_command_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                is_password: false,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("aws login", ctx);
            editor.set_buffer_text(&aws_auth_refresh_command, ctx);
            editor
        });
        AISettingsPageView::update_editor_interaction_state(
            aws_auth_refresh_command_editor.clone(),
            is_usage_enabled,
            ctx,
        );
        ctx.subscribe_to_view(&aws_auth_refresh_command_editor, |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                let should_reset = buffer_text.trim().is_empty();
                let value = if should_reset {
                    "aws login".to_string()
                } else {
                    buffer_text
                };
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let _ = settings
                        .aws_bedrock_auth_refresh_command
                        .set_value(value, ctx);
                });
                if should_reset {
                    editor.update(ctx, |editor, ctx| {
                        editor.set_buffer_text("aws login", ctx);
                    });
                }
            }
        });

        let aws_auth_refresh_profile_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                is_password: false,
                text: TextOptions {
                    font_size_override: Some(appearance.ui_font_size()),
                    font_family_override: Some(appearance.monospace_font_family()),
                    text_colors_override: Some(TextColors {
                        default_color: appearance.theme().active_ui_text_color(),
                        disabled_color: appearance.theme().disabled_ui_text_color(),
                        hint_color: appearance.theme().disabled_ui_text_color(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("default", ctx);
            editor.set_buffer_text(&aws_auth_refresh_profile, ctx);
            editor
        });
        AISettingsPageView::update_editor_interaction_state(
            aws_auth_refresh_profile_editor.clone(),
            is_usage_enabled,
            ctx,
        );
        ctx.subscribe_to_view(&aws_auth_refresh_profile_editor, |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                let should_reset = buffer_text.trim().is_empty();
                let value = if should_reset {
                    "default".to_string()
                } else {
                    buffer_text
                };
                AISettings::handle(ctx).update(ctx, |settings, ctx| {
                    let _ = settings.aws_bedrock_profile.set_value(value, ctx);
                });
                if should_reset {
                    editor.update(ctx, |editor, ctx| {
                        editor.set_buffer_text("default", ctx);
                    });
                }
            }
        });

        let refresh_credentials_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Refresh", SecondaryTheme)
                .with_icon(Icon::RefreshCw04)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AISettingsPageAction::RefreshAwsBedrockCredentials);
                })
        });
        refresh_credentials_button.update(ctx, |button, ctx| {
            button.set_disabled(!is_usage_enabled, ctx);
        });

        // Keep enablement in sync with the Global AI toggle.
        let aws_auth_refresh_command_editor_clone = aws_auth_refresh_command_editor.clone();
        let aws_auth_refresh_profile_editor_clone = aws_auth_refresh_profile_editor.clone();
        let refresh_credentials_button_clone = refresh_credentials_button.clone();
        ctx.subscribe_to_model(&AISettings::handle(ctx), move |_, _, event, ctx| {
            if matches!(
                event,
                AISettingsChangedEvent::IsAnyAIEnabled { .. }
                    | AISettingsChangedEvent::AwsBedrockCredentialsEnabled { .. }
            ) {
                let is_any_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);
                let is_usage_enabled = is_any_ai_enabled
                    && UserWorkspaces::as_ref(ctx).is_aws_bedrock_credentials_enabled(ctx);

                AISettingsPageView::update_editor_interaction_state(
                    aws_auth_refresh_command_editor_clone.clone(),
                    is_usage_enabled,
                    ctx,
                );
                AISettingsPageView::update_editor_interaction_state(
                    aws_auth_refresh_profile_editor_clone.clone(),
                    is_usage_enabled,
                    ctx,
                );
                refresh_credentials_button_clone.update(ctx, |button, ctx| {
                    button.set_disabled(!is_usage_enabled, ctx);
                });

                ctx.notify();
            }
        });

        let aws_auth_refresh_command_editor_clone = aws_auth_refresh_command_editor.clone();
        let aws_auth_refresh_profile_editor_clone = aws_auth_refresh_profile_editor.clone();
        let refresh_credentials_button_clone = refresh_credentials_button.clone();
        ctx.subscribe_to_model(
            &UserWorkspaces::handle(ctx),
            move |_, workspace, event, ctx| {
                if let UserWorkspacesEvent::TeamsChanged = event {
                    let is_any_ai_enabled = AISettings::as_ref(ctx).is_any_ai_enabled(ctx);
                    let is_usage_enabled = is_any_ai_enabled
                        && workspace
                            .as_ref(ctx)
                            .is_aws_bedrock_credentials_enabled(ctx);

                    AISettingsPageView::update_editor_interaction_state(
                        aws_auth_refresh_command_editor_clone.clone(),
                        is_usage_enabled,
                        ctx,
                    );
                    AISettingsPageView::update_editor_interaction_state(
                        aws_auth_refresh_profile_editor_clone.clone(),
                        is_usage_enabled,
                        ctx,
                    );
                    refresh_credentials_button_clone.update(ctx, |button, ctx| {
                        button.set_disabled(!is_usage_enabled, ctx);
                    });

                    ctx.notify();
                }
            },
        );

        Self {
            aws_auth_refresh_command_editor,
            aws_auth_refresh_profile_editor,
            credentials_enabled_toggle: SwitchStateHandle::default(),
            auto_login_toggle: SwitchStateHandle::default(),
            refresh_credentials_button,
        }
    }

    fn render_aws_bedrock_section(
        &self,
        appearance: &Appearance,
        app: &AppContext,
        is_bedrock_available: bool,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let user_workspaces = UserWorkspaces::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        let is_section_enabled = is_any_ai_enabled && is_bedrock_available;
        let is_admin_enforced = matches!(
            user_workspaces.aws_bedrock_host_enablement_setting(),
            crate::workspaces::workspace::HostEnablementSetting::Enforce
        );
        let is_toggleable =
            is_section_enabled && user_workspaces.is_aws_bedrock_credentials_toggleable();
        let are_credentials_enabled = user_workspaces.is_aws_bedrock_credentials_enabled(app);
        let is_usage_enabled = is_section_enabled && are_credentials_enabled;
        let toggle_description = if is_admin_enforced {
            "Warp loads and sends local AWS CLI credentials for Bedrock-supported models. This setting is managed by your organization.".to_string()
        } else {
            "Warp loads and sends local AWS CLI credentials for Bedrock-supported models."
                .to_string()
        };

        let mut column = Flex::column().with_spacing(16.).with_child(
            Flex::column()
                .with_child(render_ai_setting_toggle::<AwsBedrockCredentialsEnabled>(
                    "Use AWS Bedrock credentials",
                    AISettingsPageAction::ToggleAwsBedrockCredentialsEnabled,
                    are_credentials_enabled,
                    is_toggleable,
                    self.credentials_enabled_toggle.clone(),
                    &RefCell::new(HashMap::new()),
                    app,
                ))
                .with_child(render_ai_setting_description(
                    toggle_description,
                    is_section_enabled,
                    app,
                ))
                .finish(),
        );

        /// Helper function to render the UI for an input field.
        fn render_input(
            appearance: &Appearance,
            label: &'static str,
            editor: ViewHandle<EditorView>,
            is_enabled: bool,
            app: &AppContext,
        ) -> Box<dyn Element> {
            let padding = Some(Coords {
                top: 10.,
                bottom: 10.,
                left: 16.,
                right: 16.,
            });
            let editor_style = UiComponentStyles {
                padding,
                background: Some(appearance.theme().surface_2().into()),
                ..Default::default()
            };

            let label = Text::new_inline(label, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                .with_color(styles::header_font_color(is_enabled, app).into())
                .finish();

            let input = appearance
                .ui_builder()
                .text_input(editor)
                .with_style(editor_style)
                .build()
                .finish();

            Flex::column()
                .with_spacing(8.)
                .with_child(label)
                .with_child(input)
                .finish()
        }

        fn render_credential_status_card(
            refresh_button: &ViewHandle<ActionButton>,
            appearance: &Appearance,
            are_credentials_enabled: bool,
            app: &AppContext,
        ) -> Box<dyn Element> {
            let (title_color, detail_color) = (
                styles::header_font_color(are_credentials_enabled, app),
                styles::description_font_color(are_credentials_enabled, app),
            );
            let (title_text, detail_text, icon) = ApiKeyManager::as_ref(app)
                .aws_credentials_state()
                .user_facing_components();

            let icon = Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(title_color).finish())
                    .with_width(16.)
                    .with_height(16.)
                    .finish(),
            )
            .with_horizontal_padding(4.)
            .finish();

            let text_column = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(4.)
                .with_child(
                    Text::new_inline(title_text, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .with_color(title_color.into())
                        .finish(),
                )
                .with_child(
                    Text::new(detail_text, appearance.ui_font_family(), CONTENT_FONT_SIZE)
                        .with_color(detail_color.into())
                        .soft_wrap(true)
                        .finish(),
                );

            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(12.)
                    .with_child(
                        Expanded::new(
                            1.,
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(12.)
                                .with_child(icon)
                                .with_child(Expanded::new(1., text_column.finish()).finish())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(ChildView::new(refresh_button).finish())
                    .finish(),
            )
            .with_uniform_padding(12.)
            .with_background(appearance.theme().surface_2())
            .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish()
        }

        column.add_child(
            Container::new(render_credential_status_card(
                &self.refresh_credentials_button,
                appearance,
                are_credentials_enabled,
                app,
            ))
            .with_margin_top(-styles::DESCRIPTION_MARGIN_BOTTOM)
            .finish(),
        );
        column.add_child(render_input(
            appearance,
            "Login Command",
            self.aws_auth_refresh_command_editor.clone(),
            is_usage_enabled,
            app,
        ));
        column.add_child(render_input(
            appearance,
            "AWS Profile",
            self.aws_auth_refresh_profile_editor.clone(),
            is_usage_enabled,
            app,
        ));

        let auto_login_enabled = *AISettings::as_ref(app).aws_bedrock_auto_login.value();

        let toggle = render_ai_setting_toggle::<AwsBedrockAutoLogin>(
            "Automatically run login command",
            AISettingsPageAction::ToggleAwsBedrockAutoLogin,
            auto_login_enabled,
            is_usage_enabled,
            self.auto_login_toggle.clone(),
            &RefCell::new(HashMap::new()),
            app,
        );
        let description = render_ai_setting_description(
            "When enabled, the login command will run automatically when AWS Bedrock credentials expire.",
            is_usage_enabled,
            app,
        );
        column.add_child(
            Flex::column()
                .with_child(toggle)
                .with_child(description)
                .finish(),
        );

        column.finish()
    }
}

impl SettingsWidget for AwsBedrockWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "aws bedrock amazon credentials login profile"
    }

    fn should_render(&self, app: &AppContext) -> bool {
        // Only show if admin has enabled AWS Bedrock for the workspace
        UserWorkspaces::as_ref(app).is_aws_bedrock_available_from_workspace()
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ai_settings = AISettings::as_ref(app);
        let is_any_ai_enabled = ai_settings.is_any_ai_enabled(app);
        let is_bedrock_available =
            UserWorkspaces::as_ref(app).is_aws_bedrock_available_from_workspace();

        let column = Flex::column()
            .with_child(render_separator(appearance))
            .with_child(
                build_sub_header(
                    appearance,
                    "AWS Bedrock",
                    Some(styles::header_font_color(is_any_ai_enabled, app)),
                )
                .with_padding_bottom(HEADER_PADDING)
                .finish(),
            )
            .with_child(self.render_aws_bedrock_section(appearance, app, is_bedrock_available));

        Container::new(column.finish())
            .with_margin_bottom(HEADER_PADDING)
            .finish()
    }
}

mod styles {
    use warp_core::ui::{appearance::Appearance, theme::Fill};
    use warpui::{AppContext, SingletonEntity};

    // Apply a negative margin to the description text so it appears closer to the main
    // settings option text.
    pub const DESCRIPTION_NEGATIVE_MARGIN_OFFSET: f32 = -12.;

    /// The space between a description and the next toggle.
    pub const DESCRIPTION_MARGIN_BOTTOM: f32 = 12.;

    /// Margin to leave for switch toggle to the right of the description subtext.
    pub const TOGGLE_WIDTH_MARGIN: f32 = 48.;

    pub fn header_font_color(is_enabled_setting: bool, app: &AppContext) -> Fill {
        let appearance = Appearance::as_ref(app);
        if is_enabled_setting {
            appearance
                .theme()
                .main_text_color(appearance.theme().surface_2())
        } else {
            appearance.theme().disabled_ui_text_color()
        }
    }

    pub fn description_font_color(is_enabled_setting: bool, app: &AppContext) -> Fill {
        let appearance = Appearance::as_ref(app);
        if is_enabled_setting {
            appearance
                .theme()
                .sub_text_color(appearance.theme().surface_1())
        } else {
            appearance.theme().disabled_ui_text_color()
        }
    }
}
