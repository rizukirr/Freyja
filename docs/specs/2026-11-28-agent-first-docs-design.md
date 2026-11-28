---
title: agent-first-docs
date: 2026-11-28
status: approved
---

# agent-first-docs: Design

## Problem

The documentation now lives in `~/Projects/freyja-web`, a Docusaurus site that still syncs a copy from `freyja/docs/`. The two copies have drifted, and the freyja copy is the stale one. Across the site, the crate docs and the examples, the first thing a reader sees is often `client.generate`, which is the low-level call. `Agent` is the intended entry point, so readers meet two styles and do not know which to use.

## Goals

1. The website is the only copy. `freyja` has no tracked file under `docs/` (the gitignored `docs/specs` and `docs/plans` stay), and every README link that pointed into `docs/` points to `https://rizukirr.github.io/freyja-web/docs/...`. Criterion: `git ls-files docs` in freyja prints nothing, and `grep -n "](docs/" README.md` finds nothing.
2. The website no longer syncs from freyja. `scripts/sync-docs.mjs`, `test/sync-docs.test.mjs` and the `sync-docs` npm script are gone, the README's documentation source section says the docs are edited here, and the docs `editUrl` points to `https://github.com/rizukirr/freyja-web/edit/main/`. Criterion: `npm test` passes and `grep -rn "sync-docs\|rizukirr/freyja/edit" package.json README.md docusaurus.config.js` finds nothing.
3. The website's `extending.md` carries the `Recorder` section the repo copy has. Criterion: `grep -c "ScriptedModel::replay" docs/extending.md` is at least 1 in freyja-web.
4. A new guide `docs/calling-the-model-directly.md` documents the low-level calls: `Client::generate`, `Client::generate_as`, `Client::stream`, `Client::check`, and the manual tool loop. It sits after Building an agent in the sidebar and in the docs index table. Criterion: the file exists, `sidebars.js` and `docs/README.md` name it, and `npm run build` passes.
5. Outside that guide, `docs/reference/client.md` and `docs/reference/wire/`, no website code example calls `client.generate`, `client.generate_as` or `client.stream`. Pages that need the low-level form link to the guide. Criterion: `grep -rln "client\.generate\|client\.stream" docs --include=*.md` in freyja-web lists only `docs/calling-the-model-directly.md` and `docs/reference/client.md`, plus any file under `docs/reference/wire/`, `docs/specs/` or `docs/plans/`.
6. The crate docs in `src/lib.rs` lead with `Agent` and mention `Client::generate` only as the low-level layer. Criterion: `grep -c "client.generate\|\.generate(&" src/lib.rs` is 0 and `cargo --offline test --doc` passes.
7. `examples/simple.rs`, `custom_endpoint.rs`, `images.rs` and `async_tools.rs` use `Agent`. `tool_loop.rs`, `retry.rs`, `pairing_probe.rs`, `portable.rs`, `structured_output.rs` and `streaming.rs` keep the low-level calls and say in their opening doc comment that they are low-level examples. Criterion: `grep -l "\.generate(\|\.generate_as\|\.stream(" examples/*.rs` lists only the six low-level files, each of their first doc paragraphs contains "low-level", and `cargo --offline build --examples --all-features` passes.
8. Nothing else in freyja breaks. Criterion: `cargo --offline test --all-features`, `cargo --offline clippy --all-features --all-targets -- -D warnings` and `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` pass. Source comments that point at `docs/...` files name the website page instead.

## Non-goals

- Deploying the website or turning on GitHub Pages.
- Changing what any example does beyond how it calls the model.
- A typed-output method on `Agent`.
- Rewriting the blog.

## Constraints

- Rust 2024, MSRV 1.88. No new dependency in either repo.
- No live LLM calls. Examples are built, never run.
- Writing follows the plain rules: no em dashes, no semicolons, one line per paragraph.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: no simpler framing. Removing the repo copy without fixing the sync would break the site's sync, and making examples consistent needs the low-level page to exist first.

The website changes run on a branch in a worktree of freyja-web, the crate changes on a branch in a worktree of freyja. The new guide is written first so other pages can link to it. Pages keep their structure, and only the examples and the sentences around them change.

## Alternatives considered

- Keeping both copies and the sync. The user moved the docs, and two copies already drifted.
- Putting the low-level examples on the Client reference page. The user picked a guide page.

## Testing

freyja: the cargo checks in goal 8 plus `cargo --offline build --examples --all-features`. freyja-web: `npm test` and `npm run build`, which fails on broken links.

## Open questions

N/A: the user answered scope, page placement and URL.
