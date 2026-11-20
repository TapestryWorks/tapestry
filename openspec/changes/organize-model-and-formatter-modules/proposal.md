# Proposal

## Why

`tapestry-core` currently spreads model-provider integration, model-client behavior, prompt rendering, and event-to-history projection across sibling modules, even though these responsibilities form two coherent areas: calling models and preparing model history. Grouping them into `model` and `formatter` directories will make those boundaries easier to understand and extend without changing runtime behavior.

## What Changes

- Organize model clients and provider integrations under `tapestry-core::model`, including the existing genai integration.
- Organize prompt rendering and event-to-history projection under `tapestry-core::formatter`.
- Keep event-log projection semantics, model/protocol data types, and provider mapping behavior unchanged.
- Preserve existing public exports where practical so this remains a source-compatible structural refactor.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This is a module organization refactor with no intended behavior or protocol changes.

## Impact

- Affected: `crates/tapestry-core/src/model.rs`, `projection.rs`, `prompt.rs`, `genai/`, their imports/re-exports, related tests, and `docs/architecture.md`.
- No dependency or wire-format changes are intended.
- Set `skip_specs: true`; existing behavior requirements remain unchanged.
