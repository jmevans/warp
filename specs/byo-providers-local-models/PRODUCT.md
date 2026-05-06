# BYO Providers and Local Models for Warp Agent

## Summary

Warp Agent should let users run the built-in agent against their own API credentials, enterprise proxies, and local OpenAI-compatible or Anthropic-compatible runtimes without requiring Warp-hosted inference. Warp-hosted models remain available as a separate provider choice and continue to follow existing Warp plan, credit, and server-side routing rules.

## Problem

Warp already has a BYO API key surface, but that surface is limited to a fixed set of providers, is tied to plan or workspace policy checks, and still sends built-in agent requests through Warp's server-mediated agent path. Users who build Warp locally, run local models, or need enterprise-controlled egress need a first-class direct-provider path for Warp Agent.

## Goals

- Let any user configure self-provided inference for Warp Agent when the provider credentials or endpoint are controlled by that user.
- Support direct OpenAI and Anthropic API providers plus custom OpenAI-compatible and Anthropic-compatible HTTP endpoints.
- Support local runtimes and enterprise proxies through the same provider UX as hosted API providers.
- Keep provider provenance visible anywhere the user chooses a model.
- Keep secrets out of plain-text configuration by default.
- Preserve Warp-hosted models as a separate provider mode.

## Non-goals

- Do not add support for consumer chat subscriptions such as ChatGPT Plus or Claude Pro as account entitlements.
- Do not replace bring-your-own CLI agent integrations; this spec is for the built-in Warp Agent inference path.
- Do not promise full tool, vision, or structured-output support on every compatibility server.
- Do not require a custom endpoint to expose model discovery if the user can manually enter a model id.
- Do not require all AI surfaces to move to direct-provider mode in the first implementation. Warp Agent is the required first surface.

## Figma

Figma: none provided.

## Behavior

1. Warp exposes a first-class Providers surface for Warp Agent configuration. A provider can be one of `warp-hosted`, `openai`, `anthropic`, `openai-compatible`, or `anthropic-compatible`.

2. `warp-hosted` providers represent existing Warp-routed models. They remain subject to existing Warp account, plan, credit, workspace, and server-side orchestration behavior.

3. `openai` providers represent direct calls to OpenAI using the user's own API credentials.

4. `anthropic` providers represent direct calls to Anthropic using the user's own API credentials.

5. `openai-compatible` providers represent custom endpoints that implement the OpenAI HTTP API shape closely enough for Warp to send chat-style requests. This includes local runtimes and enterprise proxies.

6. `anthropic-compatible` providers represent custom endpoints that implement the Anthropic Messages API shape closely enough for Warp to send message-style requests. This includes local runtimes and enterprise proxies.

7. A user can configure a direct or compatible provider without a Warp paid plan when Warp is not brokering inference through Warp-hosted infrastructure.

8. A direct or compatible provider requires a stable provider id, display name, provider kind, enabled state, base URL, authentication mode, optional headers, defaults, discovery settings, and capability settings.

9. Provider ids are user-visible in config and automation surfaces. They must be stable after creation unless the user explicitly renames or duplicates the provider.

10. The Providers settings UI supports adding, validating, editing, duplicating, disabling, deleting, and setting defaults for provider profiles.

11. The Add Provider flow asks for provider kind, display name, base URL, authentication mode, optional headers, validation, model discovery or manual model entry, and default assignment.

12. Authentication modes available to users are no auth, API key from secure storage, API key from environment variable, and bearer token from secure storage. Inline secret entry may be accepted in import flows only long enough to move the value into secure storage or reject it.

13. Secrets are not stored in plain-text settings by default. Config files and redacted exports contain secret references or environment variable names, not raw credential values.

14. When secure storage is unavailable, Warp clearly marks the provider as using a degraded secret-storage mode before accepting credentials.

15. A user can configure a local endpoint with no auth, such as an OpenAI-compatible endpoint at `http://127.0.0.1:11434/v1`.

16. A user can configure an enterprise proxy endpoint with custom headers and an API key source.

17. Warp validates providers explicitly. Saving a provider without successful validation is allowed only after the user acknowledges manual or degraded mode.

18. Cheap validation checks reachability, auth, and response-family compatibility without performing an inference request when the provider has a non-inference endpoint that can prove those properties.

19. Deep validation is optional and may send a minimal inference request. The user must be able to tell when validation could spend provider quota or incur cost.

20. Model discovery runs automatically for providers that advertise or plausibly support a model-list endpoint.

21. If model discovery succeeds, Warp stores discovered model ids and displays them under that provider.

22. If model discovery fails because the endpoint lacks a model-list endpoint, Warp lets the user save the provider in manual-model mode and enter one or more model ids.

23. If validation succeeds but model discovery fails, Warp offers retry discovery, save with manual model id, or mark the provider as manual models only.

24. Provider capability settings cover at least tools, vision, structured outputs, prompt caching, reasoning controls, and model discovery. Each capability can be automatic, enabled, or disabled where that distinction is meaningful.

25. Warp disables or warns on UI affordances that require unsupported provider capabilities. For example, a provider marked as not supporting tools cannot silently be selected for a tool-dependent agent profile without a visible limitation.

26. Model pickers show provider and model as distinct information. The user can tell whether a model is Warp-hosted, direct, local, proxy-backed, discovered, manually entered, or capability-limited before selecting it.

27. Model picker groups include Warp-hosted, OpenAI direct, Anthropic direct, local OpenAI-compatible, local Anthropic-compatible, and enterprise or custom providers when such providers exist.

28. Provider-level defaults can be assigned for Warp Agent. Additional AI surfaces such as command suggestions or inline completion may use the same default model only after that surface has been explicitly wired to the provider registry.

29. Conversation-level and execution-profile-level model choices can override provider defaults.

30. Selecting a direct or compatible provider for a turn sends the prompt, attachments, tool definitions, and provider credentials only to the selected provider endpoint and the local Warp client components needed to execute the turn.

31. Direct-provider prompts, attachments, responses, tool arguments, and credentials are not sent to Warp inference services.

32. Warp never silently switches a direct or compatible provider request to Warp-hosted inference.

33. Fallback from a direct or compatible provider to Warp-hosted inference is off by default and can be enabled only by an explicit provider-level policy setting.

34. When fallback is enabled, Warp visibly marks fallback behavior in the provider UI and model picker so the user knows Warp-hosted inference may be used.

35. If fallback is disabled and the selected provider fails, Warp surfaces a provider error and leaves the conversation in a recoverable failed state.

36. Provider errors are actionable. Auth errors, unreachable endpoint errors, invalid response shape errors, missing model errors, unsupported capability errors, timeout errors, and interrupted stream errors must be distinguishable to the user.

37. Redacted reliability telemetry is allowed for direct-provider mode, but it must not include prompt bodies, response bodies, file contents, tool arguments containing user data, raw headers, raw secrets, resolved secret values, or full local file paths by default.

38. Allowed direct-provider telemetry is limited to operational metadata such as provider kind, local-vs-remote classification, redacted or hashed endpoint origin, redacted or hashed model id, capability probe result, success or failure class, HTTP status family, latency, stream duration, and token usage when provided by the provider.

39. Provider configuration can be edited from Settings. Any command-palette or CLI actions added for provider management must use the same provider model and validations as Settings.

40. Warp's persisted local settings remain TOML-backed. Import and export may support JSON or YAML provider templates, but the local settings file is still the source of non-secret persisted provider metadata.

41. Redacted export never includes raw secrets. If the user explicitly requests an unredacted export, Warp must warn before including any secret and should prefer secret references whenever possible.

42. Existing OpenAI and Anthropic BYO API keys are migrated into provider profiles without requiring the user to re-enter secrets.

43. Migration preserves existing Warp-hosted model behavior and existing Warp credit fallback preference.

44. After migration, users with existing BYO keys see equivalent direct-provider profiles in the Providers UI.

45. Managed teams can disable custom compatible endpoints or restrict them to allowlisted endpoint origins.

46. Managed teams can allow first-party direct providers while disallowing arbitrary custom endpoints.

47. Managed-team policy restrictions are visible in Settings when a user attempts to add or use a disallowed provider.

48. When a `warp-oss` or self-built client is used, direct and compatible providers work for user-supplied inference without requiring access to paid Warp-hosted model entitlements.

49. If the user is signed out, Warp may require a local identity record for preferences, but it must not require cloud plan entitlement for self-provided inference.

50. Local-only providers are labeled as local when their endpoint resolves to localhost or another configured local-only origin.

51. A local-only provider must not use Warp-hosted fallback unless the user explicitly disables the local-only policy or changes the provider's fallback policy.

52. Provider validation and model discovery can be retried independently without deleting or recreating the provider.

53. Disabling a provider hides it from new model-picker selections but does not delete its metadata, model cache, or secret reference.

54. Deleting a provider removes its metadata and prompts the user before deleting any associated secret from secure storage.

55. Duplicating a provider copies non-secret metadata and creates a new provider id. It must not duplicate the raw secret value unless the user explicitly chooses to reuse the same secret reference.

56. Provider changes take effect for new agent turns. An in-flight direct-provider request continues using the provider settings resolved at the start of that request.

57. If a provider is deleted while a conversation still references it, the conversation shows the historical provider label and asks the user to choose a replacement before sending a new turn.

58. If a discovered model disappears from a provider, existing conversations can still display that model historically, but new turns require a currently valid or manually confirmed model id.

59. Warp's UI language distinguishes API providers from CLI agent integrations so users do not confuse direct HTTP provider support with consumer-subscription or external CLI support.

60. The feature is successful when an open-source contributor can build Warp, add an OpenAI or Anthropic API key, and use Warp Agent without a Warp paid subscription; a local-model user can point Warp Agent at a localhost compatible endpoint; an enterprise user can configure an approved proxy; and no user is surprised by implicit Warp credit usage.

## Open questions

1. Should the first implementation support direct-provider requests while fully signed out, or should it require a local/offline identity record?

2. Should custom endpoints be allowed for managed teams by default until an admin policy exists, or should managed teams require explicit admin enablement from day one?

3. Should discovered model metadata use a fixed TTL, an indefinite cache with manual refresh, or both?
