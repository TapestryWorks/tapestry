# Tasks

## 1. Organize model invocation

- [x] 1.1 Convert the current model client module into `model/` with a client submodule and relocate the genai integration under `model/genai/`; preserve feature gating and verify with `cargo test -p tapestry-core --features genai`.
- [x] 1.2 Update model and genai re-exports/imports while retaining existing public paths; verify default builds with `cargo check -p tapestry-core`.

## 2. Organize model-input formatting

- [x] 2.1 Move history projection and prompt rendering into `formatter/history.rs` and `formatter/prompt.rs` without changing behavior; verify existing core tests pass with `cargo test -p tapestry-core`.
- [x] 2.2 Retain the current `projection` and `prompt` public paths and crate-root exports as compatibility re-exports; verify compilation with `cargo check --workspace`.

## 3. Document and validate the module layout

- [x] 3.1 Update `docs/architecture.md` to describe the `model` and `formatter` module responsibilities and their dependency direction; verify the documentation matches the implemented public layout.
- [x] 3.2 Run `cargo fmt --all -- --check` and `cargo test --workspace` to verify formatting and unchanged workspace behavior.
