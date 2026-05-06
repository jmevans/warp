# BYO Providers and Local Models for Warp Agent

## Status: Complete

Implemented on branch `byo-providers-local-models`. PR #1 open.

## Summary

Users can add their own LLM providers (OpenAI, Anthropic, OpenAI-compatible) with their own API keys, select models from those providers in the agent model picker, and set them as the default model for their execution profile — bypassing Warp-hosted inference when desired.

## What was implemented

- **Provider Registry** — CRUD for local provider profiles with API key storage, endpoint configuration, and model discovery
- **Direct Provider Request Path** — SSE-based streaming to local provider endpoints without routing through Warp servers
- **Managed Team Policy** — Enforced `allow_in_managed_teams` policy to block BYO providers on managed teams
- **Settings UI** — Add/edit/remove provider profiles with API key management
- **Model Picker Integration** — Local provider models appear alongside Warp-hosted models
- **Profile Persistence** — "Set as default" persists local provider model to cloud-synced execution profile
- **BYO Model Resolution Fix** — Cache-based fallback ensures local provider models resolve correctly at startup even before the catalog loads

## Technical details

See `TECH.md` for architecture details and `PRODUCT.md` for user-visible behavior.

## Build status

- Compiles with 0 errors, 0 warnings
- 3777+ tests pass (warp crate)
- 997 AI-related tests pass
- CI blocked on pre-existing Node/env issue in `command-signatures-v2`
