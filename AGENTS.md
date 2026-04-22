# AGENTS.md

## Scope
This file applies to the entire repository.

## Purpose
This repository is used by both human contributors and AI coding agents. Treat this file as the default collaboration contract for planning, editing, validating, and handing off work.

## Project Overview
- `termua` is a cross-platform terminal application built with Rust and GPUI.
- The repository is a Rust workspace with the main desktop application in `termua/` and supporting crates in `crates/`.
- Major functional areas include terminal backends, SSH, serial sessions, SFTP, terminal sharing, cast playback/recording, themes, settings, and AI assistant integration.
- The codebase is platform-sensitive. Linux, macOS, and Windows behavior all matter.

## Repository Map
- `termua/`: main desktop application, windowing, session flows, settings, UI composition, and app bootstrap.
- `crates/alacritty_terminal/`: terminal engine integration and terminal behavior tests.
- `crates/gpui_*` and `crates/menubar/`: reusable UI and interaction components.
- `crates/termua_relay/`: relay server and websocket protocol logic.
- `crates/termua_zeroclaw/`: AI assistant integration.
- `crates/wezterm-ssh/`: SSH-related functionality and tests.
- `assets/`: icons, logos, screenshots, shell-related assets.
- `locales/`: localization resources.
- `packaging/`: Linux, macOS, and Windows packaging scripts.
- `.github/workflows/`: CI expectations for formatting, linting, tests, coverage, and packaging.

## Toolchain And Build Expectations
- The repository includes a local `nightly` Rust toolchain configuration.
- CI currently runs core checks on `stable` for formatting, linting, tests, and packaging.
- Do not introduce changes that depend on nightly-only behavior unless the repository is explicitly being migrated and the CI configuration is updated with it.
- Keep the workspace buildable in the same spirit as CI, not just in one local environment.

## Core Working Principles
- Prefer small, targeted changes over broad refactors.
- Fix root causes instead of layering workarounds when reasonably possible.
- Preserve cross-platform behavior. If a change is platform-specific, avoid accidental regressions on other platforms.
- Follow existing module boundaries and naming conventions.
- Preserve modularity and extensibility. Avoid designs that introduce unnecessary coupling or make future expansion harder.
- Keep changes easy to review. Separate unrelated edits.
- Do not opportunistically rewrite adjacent code unless it is necessary for correctness, safety, or maintainability of the requested task.

## Rules For Human Contributors
- Read the relevant module before editing it.
- If touching multiple crates, make sure the dependency direction still makes architectural sense.
- Avoid introducing new dependencies unless they are clearly justified.
- Keep public API changes intentional and documented in the handoff summary.
- If behavior changes, update user-facing docs or examples when appropriate.

## Rules For AI Agents
- For non-trivial work, present a short plan before editing.
- Read the relevant files and nearby call sites before making changes.
- Do not modify unrelated files just because they are convenient to clean up.
- Do not claim success without running verification commands and reporting what was run.
- Prefer surgical edits that match the existing style of the touched module.
- Keep solutions modular and extensible. Prefer designs that reduce coupling and leave room for future growth.
- If a task is ambiguous, narrow the change to the safest interpretation rather than inventing large new behavior.
- When you cannot fully verify something locally, say so explicitly and name the remaining risk.

## Editing Guidance By Area

### `termua/`
- Treat `termua/` as the product entry point and integration layer.
- Be careful with startup flow, settings loading, logging, app bootstrap, and session lifecycle code.
- UI changes should preserve existing interaction patterns unless the task explicitly asks for UX changes.
- Changes in settings, theme behavior, localization, or session management should be checked for downstream effects across windows and sidebars.

### `crates/alacritty_terminal/`
- This area is behavior-sensitive and heavily test-oriented.
- Prefer minimal changes with strong regression coverage.
- Do not update reference fixtures casually; only do so when behavior intentionally changes and the new output is understood.

### `crates/termua_relay/`
- Preserve protocol compatibility unless the task explicitly includes a protocol change.
- Changes to connection flow, control gating, or websocket state must be validated with tests.

### `crates/wezterm-ssh/`
- Treat SSH and SFTP behavior as correctness-sensitive.
- Avoid changing authentication, proxying, session handling, or file transfer behavior without targeted test coverage.

### `crates/gpui_*` and `crates/menubar/`
- These crates are shared building blocks. Avoid app-specific assumptions leaking into reusable components.
- Favor API additions that are small and composable over one-off special cases.

### `assets/`, `locales/`, and `packaging/`
- Only modify these when the task clearly requires it.
- Keep asset naming and packaging conventions consistent with the existing structure.
- Localization updates should keep keys organized and avoid silent divergence between locales.

## Testing And Verification Policy
This repository uses a max-verification mindset. Before claiming a task is complete, run the broadest reasonable validation set for the affected area. Unless the task is purely editorial, prefer the following baseline:

1. `cargo fmt --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo nextest run --no-fail-fast --all-features`

If `cargo-nextest` is unavailable locally, use:
- `cargo test --workspace --all-features`

When feasible, also run:
- `cargo build --workspace --all-features`

Additional expectations:
- For changes in a specific crate, run targeted tests first, then the broad workspace checks.
- For protocol, relay, terminal, SSH, SFTP, settings, or session-lifecycle changes, prefer full workspace verification instead of narrow-only checks.
- For packaging-related changes, validate the relevant packaging scripts or build steps when practical.
- For documentation-only changes, testing is optional, but note that no code verification was needed.

## CI Alignment
Before concluding that work is done, align with the intent of CI:
- Formatting must pass.
- Clippy must pass with warnings denied.
- Tests should pass broadly, not only for a hand-picked happy path.
- Do not rely on “works on my machine” reasoning if CI-facing commands were not run.

## Change Boundaries
- Do not edit generated, build output, or cache-like directories such as `target/`.
- Do not modify lockfiles, packaging metadata, screenshots, or assets unless the task actually requires it.
- Do not change workflow files just to make local work easier unless CI behavior is part of the requested task.
- Do not introduce broad formatting churn in untouched files.

## Handoff Expectations
When handing work off to another person or to the user, include:
- what changed
- which files were touched
- what verification was run
- any limitations, assumptions, or follow-up work

## Preferred Change Style
- Keep patches reviewable.
- Favor explicitness over cleverness.
- Reuse existing patterns already present in the workspace.
- Prefer modular, composable changes over tightly coupled one-off implementations.
- Add tests near the affected code when the repository already has a local testing pattern for that area.
- Put unit tests at the bottom of the file when adding or updating inline tests.
- Preserve consistency with existing naming, error handling, and module organization.

## When In Doubt
- choose the smaller change
- verify more, not less
- preserve platform compatibility
- document assumptions clearly
