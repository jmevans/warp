pub(crate) mod convert_conversation;
mod convert_from;
mod convert_to;
mod direct_provider;
mod r#impl;

use std::collections::HashMap;

pub use ai::agent::convert::ConvertToAPITypeError;
use ai::api_keys::ApiKeyManager;
use ai::provider_registry::{ProviderResolutionError, ResolvedProvider};
pub use convert_from::{
    user_inputs_from_messages, ConversionParams, ConvertAPIMessageToClientOutputMessage,
    MaybeAIAgentOutputMessage, MessageToAIAgentOutputMessageError,
};
pub use direct_provider::{AgentExchangeSnapshot, SerializedImage};

pub use r#impl::generate_multi_agent_output;

use futures_lite::Stream;
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use settings::Setting as _;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use warp_core::channel::ChannelState;
use warp_core::execution_mode::AppExecutionMode;
use warp_core::features::FeatureFlag;

use crate::ai::agent::conversation::AIConversationId;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::{
    ai::{blocklist::SessionContext, llms::LLMId},
    server::server_api::AIApiError,
};

use super::{AIAgentInput, MCPContext, MCPServer, RequestMetadata, Suggestions};
use warpui_extras::secure_storage::AppContextExt;

use crate::ai::blocklist::{BlocklistAIPermissions, RequestInput};
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::templatable_manager::TemplatableMCPServerInfo;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::settings::AISettings;
use crate::terminal::safe_mode_settings::get_secret_obfuscation_mode;
use crate::workspaces::user_workspaces::UserWorkspaces;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{AppContext, EntityId, SingletonEntity as _};

/// Unique, server-generated conversation-scoped token to be roundtripped to the API when sending
/// requests that follow-up within a given conversation.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerConversationToken(String);

impl ServerConversationToken {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn debug_link(&self) -> String {
        format!(
            "{}/debug/maa/{}",
            ChannelState::server_root_url(),
            self.as_str()
        )
    }

    pub fn conversation_link(&self) -> String {
        format!(
            "{}/conversation/{}",
            ChannelState::server_root_url(),
            self.as_str()
        )
    }
}

impl From<ServerConversationToken> for String {
    fn from(value: ServerConversationToken) -> Self {
        value.0
    }
}

// Conversions between AI ServerConversationToken and protocol ServerConversationToken
impl From<session_sharing_protocol::common::ServerConversationToken> for ServerConversationToken {
    fn from(token: session_sharing_protocol::common::ServerConversationToken) -> Self {
        Self(token.to_string())
    }
}

impl TryFrom<ServerConversationToken>
    for session_sharing_protocol::common::ServerConversationToken
{
    type Error = uuid::Error;

    fn try_from(token: ServerConversationToken) -> Result<Self, Self::Error> {
        token.as_str().parse()
    }
}

#[allow(missing_debug_implementations, clippy::type_complexity)]
#[derive(Clone)]
pub struct RequestParams {
    pub input: Vec<AIAgentInput>,
    pub conversation_token: Option<ServerConversationToken>,
    pub forked_from_conversation_token: Option<ServerConversationToken>,
    pub ambient_agent_task_id: Option<AmbientAgentTaskId>,
    pub tasks: Vec<warp_multi_agent_api::Task>,
    pub existing_suggestions: Option<Suggestions>,
    pub metadata: Option<RequestMetadata>,
    pub session_context: SessionContext,
    pub model: LLMId,
    #[allow(unused)]
    pub coding_model: LLMId,
    pub cli_agent_model: LLMId,
    pub computer_use_model: LLMId,
    pub is_memory_enabled: bool,
    pub warp_drive_context_enabled: bool,
    pub context_window_limit: Option<u32>,
    pub mcp_context: Option<MCPContext>,
    pub planning_enabled: bool,
    should_redact_secrets: bool,
    pub resolved_provider: Result<ResolvedProvider, ProviderResolutionError>,

    /// The provider ID from the original request's model ID, preserved even if
    /// provider resolution fails (e.g. provider was deleted mid-conversation).
    pub original_provider_id: Option<String>,

    /// User-provided API keys for AI providers (BYO API Key).
    pub api_keys: Option<warp_multi_agent_api::request::settings::ApiKeys>,
    pub legacy_api_keys: ai::api_keys::ApiKeys,
    pub allow_use_of_warp_credits_with_byok: bool,
    pub autonomy_level: warp_multi_agent_api::AutonomyLevel,
    pub isolation_level: warp_multi_agent_api::IsolationLevel,
    pub web_search_enabled: bool,
    pub computer_use_enabled: bool,
    pub ask_user_question_enabled: bool,
    pub research_agent_enabled: bool,
    pub orchestration_enabled: bool,
    pub supported_tools_override: Option<Vec<warp_multi_agent_api::ToolType>>,
    /// The conversation ID of the parent agent that spawned this child agent, if any.
    pub parent_agent_id: Option<String>,
    /// The display name for this agent (e.g. "Agent 1"), assigned by the orchestrator.
    pub agent_name: Option<String>,
    /// Resolves a secret reference (e.g. keychain path or env var name) to its value.
    /// Populated at construction time from secure storage + env, so the stream can
    /// resolve secrets without holding an `AppContext`.
    pub resolve_secret: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,

    /// Previous exchanges in the conversation, serialized for direct-provider message history.
    pub conversation_history: Vec<AgentExchangeSnapshot>,

    /// Executes MCP tool calls in direct-provider mode.
    pub mcp_tool_executor: Option<Arc<dyn McpToolExecutor>>,
}

pub type Event = Result<warp_multi_agent_api::ResponseEvent, Arc<AIApiError>>;

#[cfg(not(target_family = "wasm"))]
pub type ResponseStream = Pin<Box<dyn Stream<Item = Event> + Send + 'static>>;

// The WASM version of this type has no bound on `Send`, which is an unnecessary bound when
// targeting wasm because the browser is single-threaded (and we don't leverage WebWorkers for async
// execution in WoW).
#[cfg(target_family = "wasm")]
pub type ResponseStream = Pin<Box<dyn Stream<Item = Event>>>;

#[derive(Debug, Clone)]
pub struct ConversationData {
    pub id: AIConversationId,
    pub tasks: Vec<warp_multi_agent_api::Task>,
    pub server_conversation_token: Option<ServerConversationToken>,
    pub forked_from_conversation_token: Option<ServerConversationToken>,
    pub ambient_agent_task_id: Option<AmbientAgentTaskId>,
    pub existing_suggestions: Option<Suggestions>,
}

impl RequestParams {
    pub fn new(
        terminal_view_id: Option<EntityId>,
        session_context: SessionContext,
        request_input: &RequestInput,
        conversation: ConversationData,
        metadata: Option<RequestMetadata>,
        app: &AppContext,
    ) -> Self {
        let ai_settings = AISettings::as_ref(app);
        let is_memory_enabled = ai_settings.is_memory_enabled(app);
        let warp_drive_context_enabled = ai_settings.is_warp_drive_context_enabled(app);

        // Build MCP context - either grouped by server or flat lists based on feature flag
        let mcp_context = if FeatureFlag::MCPGroupedServerContext.is_enabled() {
            // Group MCP tools and resources by server
            let templatable_manager = TemplatableMCPServerManager::as_ref(app);

            let mut active_servers: Vec<&TemplatableMCPServerInfo> = templatable_manager
                .get_active_templatable_servers()
                .values()
                .copied()
                .collect();

            // If file-based MCP servers are enabled, add active servers in scope of
            // the user's current working directory
            if let Some(cwd) = session_context.current_working_directory() {
                active_servers.extend(
                    templatable_manager
                        .get_active_file_based_servers(Path::new(cwd), app)
                        .values(),
                );
            }

            // Include any ephemeral MCP servers started via the Oz CLI.
            active_servers.extend(
                templatable_manager
                    .get_active_cli_spawned_servers()
                    .values(),
            );

            let servers: Vec<MCPServer> = active_servers
                .into_iter()
                .map(|server| MCPServer {
                    name: server.name().to_string(),
                    description: server.description().unwrap_or_default().to_string(),
                    id: server.installation_id().to_string(),
                    resources: server.resources().to_vec(),
                    tools: server.tools().to_vec(),
                })
                .collect();

            if servers.is_empty() {
                None
            } else {
                #[allow(deprecated)]
                Some(MCPContext {
                    resources: vec![],
                    tools: vec![],
                    servers,
                })
            }
        } else {
            // Flat lists of resources and tools
            let templatable_mcp_manager = TemplatableMCPServerManager::as_ref(app);
            let resources = templatable_mcp_manager
                .resources()
                .cloned()
                .collect::<Vec<_>>();
            let tools = templatable_mcp_manager.tools().cloned().collect::<Vec<_>>();

            #[allow(deprecated)]
            (!resources.is_empty() || !tools.is_empty()).then_some(MCPContext {
                resources,
                tools,
                servers: vec![],
            })
        };

        let should_redact_secrets = get_secret_obfuscation_mode(app).should_redact_secret();

        let user_workspaces = UserWorkspaces::as_ref(app);
        let api_key_manager = ApiKeyManager::as_ref(app);
        let legacy_api_keys = api_key_manager.keys().clone();
        let provider_registry = api_key_manager.provider_registry_from_legacy_keys(Some(
            ai_settings.provider_registry.value().clone(),
        ));

        let original_provider_id =
            ai::provider_registry::ProviderQualifiedModelId::parse(request_input.model_id.as_str())
                .map(|parsed| parsed.provider_id);

        let resolved_provider =
            provider_registry.resolve_agent_provider(Some(request_input.model_id.as_str()));
        let resolved_provider = resolved_provider.and_then(|resolved| {
            let is_direct_allowed = user_workspaces.is_direct_provider_allowed();
            let is_compatible_allowed = user_workspaces.is_compatible_endpoint_allowed();
            let is_endpoint_origin_allowed =
                user_workspaces.is_endpoint_origin_allowed(&resolved.base_url);
            let is_managed_team = user_workspaces.is_on_managed_team();
            resolved
                .validate_team_policy(
                    is_direct_allowed,
                    is_compatible_allowed,
                    is_endpoint_origin_allowed,
                    is_managed_team,
                )
                .map(|_| resolved)
        });
        let api_keys = api_key_manager.api_keys_for_request(
            user_workspaces.is_byo_api_key_enabled(),
            user_workspaces.is_aws_bedrock_credentials_enabled(app),
        );
        let allow_use_of_warp_credits_with_byok =
            *AISettings::as_ref(app).can_use_warp_credits_with_byok;

        let app_execution_mode = AppExecutionMode::as_ref(app);
        let autonomy_level = if app_execution_mode.is_autonomous() {
            warp_multi_agent_api::AutonomyLevel::Unsupervised
        } else {
            warp_multi_agent_api::AutonomyLevel::Supervised
        };

        let isolation_level = if app_execution_mode.is_sandboxed() {
            warp_multi_agent_api::IsolationLevel::Sandbox
        } else {
            warp_multi_agent_api::IsolationLevel::None
        };

        let web_search_enabled =
            BlocklistAIPermissions::as_ref(app).get_web_search_enabled(app, terminal_view_id);
        let research_agent_enabled = app
            .private_user_preferences()
            .read_value("ResearchAgentEnabled")
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let is_ambient_agent = conversation.ambient_agent_task_id.is_some();
        let computer_use_enabled = FeatureFlag::AgentModeComputerUse.is_enabled()
            && BlocklistAIPermissions::as_ref(app)
                .get_computer_use_setting(app, terminal_view_id)
                .is_enabled()
            && computer_use::is_supported_on_current_platform()
            && (FeatureFlag::LocalComputerUse.is_enabled() || is_ambient_agent);
        let ask_user_question_enabled = BlocklistAIPermissions::as_ref(app)
            .get_ask_user_question_setting(app, terminal_view_id)
            != crate::ai::execution_profiles::AskUserQuestionPermission::Never;

        let orchestration_enabled = ai_settings.is_orchestration_enabled(app)
            && session_context
                .session_type()
                .as_ref()
                .is_none_or(|t| matches!(t, crate::terminal::model::session::SessionType::Local));

        // Reconcile the persisted override against the active base model's
        // current `LLMContextWindow` instead of trusting whatever was stored
        // last. If the active model isn't configurable or has been removed
        // server-side, drop the override; otherwise clamp it to the model's
        // current `[min, max]` range. This closes the window between an
        // in-flight model metadata refresh and the next request.
        let context_window_limit = {
            let profile_data = AIExecutionProfilesModel::as_ref(app)
                .active_profile(terminal_view_id, app)
                .data()
                .clone();
            profile_data
                .configurable_context_window(app)
                .and_then(|cw| {
                    profile_data
                        .context_window_limit
                        .map(|v| v.clamp(cw.min, cw.max))
                })
        };

        // Build a secret resolver: snapshot keychain values now (SecureStorage isn't Send),
        // and resolve env vars at request time.
        let secure_storage = app.secure_storage();
        let keychain_cache: HashMap<String, String> = resolved_provider
            .as_ref()
            .ok()
            .and_then(|p| resolve_keychain_refs(secure_storage, &p.auth))
            .unwrap_or_default();
        let resolve_secret = Arc::new(move |ref_: &str| -> Option<String> {
            if let Some(value) = keychain_cache.get(ref_) {
                return Some(value.clone());
            }
            if ref_.starts_with("legacy://") {
                return None;
            }
            std::env::var(ref_).ok()
        });

        Self {
            input: request_input.all_inputs().cloned().collect(),
            conversation_token: conversation.server_conversation_token,
            forked_from_conversation_token: conversation.forked_from_conversation_token,
            ambient_agent_task_id: conversation.ambient_agent_task_id,
            tasks: conversation.tasks,
            existing_suggestions: conversation.existing_suggestions,
            context_window_limit,
            metadata,
            session_context,
            model: request_input.model_id.clone(),
            coding_model: request_input.coding_model_id.clone(),
            cli_agent_model: request_input.cli_agent_model_id.clone(),
            computer_use_model: request_input.computer_use_model_id.clone(),
            is_memory_enabled,
            warp_drive_context_enabled,
            mcp_context,
            planning_enabled: true,
            should_redact_secrets,
            resolved_provider,
            original_provider_id,
            api_keys,
            legacy_api_keys,
            allow_use_of_warp_credits_with_byok,
            autonomy_level,
            isolation_level,
            web_search_enabled,
            computer_use_enabled,
            ask_user_question_enabled,
            research_agent_enabled,
            orchestration_enabled,
            supported_tools_override: request_input.supported_tools_override.clone(),
            parent_agent_id: None,
            agent_name: None,
            resolve_secret,
            conversation_history: Vec::new(),
            mcp_tool_executor: None,
        }
    }
}

/// Extract keychain-referenced secrets from secure storage at construction time
/// so the resolver closure can be `Send + Sync`.
fn resolve_keychain_refs(
    secure_storage: &dyn warpui_extras::secure_storage::SecureStorage,
    auth: &ai::provider_registry::ResolvedAuth,
) -> Option<HashMap<String, String>> {
    let (ref_, key) = match auth {
        ai::provider_registry::ResolvedAuth::KeychainBearerToken { secret_ref } => {
            let key = secure_storage.read_value(secret_ref).ok()?;
            (secret_ref.clone(), key)
        }
        ai::provider_registry::ResolvedAuth::KeychainApiKey { secret_ref } => {
            let ref_ = secret_ref.as_ref()?;
            if !ref_.starts_with("keychain://") {
                return None;
            }
            let key = secure_storage.read_value(ref_).ok()?;
            (ref_.clone(), key)
        }
        ai::provider_registry::ResolvedAuth::EnvApiKey { .. }
        | ai::provider_registry::ResolvedAuth::None => return None,
    };
    let mut map = HashMap::new();
    map.insert(ref_, key);
    Some(map)
}

/// Async executor for MCP tool calls in direct-provider mode.
///
/// Implemented by capturing the `TemplatableMCPServerManager` at `RequestParams`
/// construction time, so the stream can execute tools without holding an `AppContext`.
pub trait McpToolExecutor: Send + Sync {
    fn execute(
        &self,
        tool_name: String,
        arguments: serde_json::Map<String, Value>,
    ) -> BoxFuture<'static, Result<String, String>>;
}
