# Design

## Context

See proposal.md for the motivation. Today `tapestry-core` exposes model-client behavior in `model.rs`, the genai integration in a sibling `genai/` module, and model-input preparation across `projection.rs` and `prompt.rs`. `tapestry-protocol` owns the model-independent request, response, history, and event data contracts.

The refactor must preserve the observable event/history semantics, feature gating for genai, and existing public paths where practical.

## Goals / Non-Goals

**Goals:**
- Group model invocation behavior and provider integrations under `tapestry_core::model`.
- Group event-to-history projection and prompt rendering under `tapestry_core::formatter`.
- Keep model-facing data types in `tapestry-protocol`.
- Preserve existing root exports and legacy module paths as re-exports.

**Non-Goals:**
- Change event projection, incomplete-turn repair, prompt rendering, provider mapping, streaming, or runtime behavior.
- Add providers, dependencies, or new public model protocol types.
- Extract either module into a separate crate.

## Decisions

### `model` owns invocation behavior and provider adapters

Convert `model.rs` into `model/mod.rs` with the generic client and stream implementation in `model/client.rs`. Keep concrete clients in separate sibling modules: `MockModelClient` in `model/mock.rs` and the genai integration in `model/genai/`. The `model` module re-exports `ModelClient`, `ModelStream`, `ModelStreamSource`, and `MockModelClient`; its genai submodule remains behind the existing `genai` Cargo feature.

This keeps the `ModelClient` boundary provider-neutral while colocating provider-specific configuration, service, and request/response mappings. Keeping provider mapping separate from the generic client was considered, but nesting the existing integration avoids a sibling module that looks like a separate core responsibility.

### `formatter` owns event-history projection and prompt rendering

Create `formatter/history.rs` for `HistoryProjection` and its fold/repair behavior, and `formatter/prompt.rs` for existing prompt rendering helpers. Provider mapping will use these renderers as it does today. The formatter module represents preparation of model input, not a second event protocol or provider-specific wire format.

Keeping projection at `core::projection` was considered, but it leaves model-input preparation split between unrelated root modules. The new directory groups the related preparation responsibilities without changing their APIs or semantics.

### Preserve compatibility through re-exports

Retain `tapestry_core::model::{ModelClient, ...}` by re-exporting the client API from `model/mod.rs`; retain `tapestry_core::genai`, `tapestry_core::projection`, and `tapestry_core::prompt` as root re-exports of the new nested modules. Keep current crate-root exports unchanged.

Compatibility aliases avoid forcing downstream code to migrate merely because implementation files moved. A later breaking release can remove aliases separately if desired.

## Risks / Trade-offs

- [Moving feature-gated modules can accidentally alter public paths or feature behavior] → Keep compatibility re-exports and run workspace checks/tests both with default features and the genai feature enabled.
- [Import rewrites may introduce dependency cycles between model and formatter] → Formatter depends on protocol event/model types; provider mapping may depend on formatter, while formatter must not depend on provider modules.
- [Behavioral drift while moving code] → Move existing tests with their implementation and run them unchanged; do not combine this refactor with semantic edits.

## Migration Plan

Move implementation files into the new directories, update internal module declarations/imports, preserve old public module paths with re-exports, and update the architecture documentation. Verify formatting, workspace tests, and the genai-feature build. Rollback is a revert of this source-only refactor.
