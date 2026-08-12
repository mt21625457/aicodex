---
name: "openai-docs"
description: "Use for Codex models/pricing, scheduled tasks, skills, settings, setup, troubleshooting, customization, automations, and self-knowledge—including 'you,' 'your,' 'this app,' or 'this coding agent' when they refer to Codex—and for OpenAI APIs/products and ChatGPT Work. Also use for model choice/migration, prompting, SDKs, Responses, Realtime, agents, evals, and Chat/Work/Codex comparisons. Do not use for generic app/software tasks that merely mention Codex."
metadata:
  short-description: "Codex models/pricing, scheduled tasks, skills, settings, setup, troubleshooting, and self-knowledge; OpenAI APIs and ChatGPT Work. 'You'/'this app' means Codex only."
---

# OpenAI Docs

Provide current, cited OpenAI product, API, model, and Codex guidance. Read zero or one primary reference.

**First substantive action:** Search the user's exact requested official OpenAI documentation topic and any explicitly named model using a concise, topic-specific query of 2-6 essential terms. When an already-available direct official documentation search and page-retrieval capability is present, use it first: search, then fetch or open the matching official page before general web search. Otherwise, immediately use official-domain web search, then actually open or fetch the relevant official page. Complete this source order before reading a reference, inspecting local or repository files, running a Codex manual or model resolver, drafting a plan, or answering from memory. Use the actual fetched page, not a search snippet or an unopened link. If one official search or page does not establish the answer, search another appropriate official domain and actually open or fetch the result. Preserve the exact requested model; never substitute a newer model.

**Only exception:** An explicitly requested, genuinely broad, cross-topic Codex setup, orientation, or system-map synthesis may use the manual first when shell execution and an allowed temporary cache are available. A specific Codex feature, setting, command, error, model, or requested citation remains docs-first. Mixed Chat/Work/Codex comparisons are official documentation questions, not manual-first Codex requests.

For generic software tasks, answer the software task directly. OpenAI implementation, debugging, SDK, API, prompting, agent, and eval requests are not generic.

For a straightforward factual or citation-only request, follow the source order and do not read a route reference. This includes straightforward API facts, ChatGPT Work or mixed Chat/Work/Codex comparisons, model tiers, aliases, Pro mode, reasoning settings, factual migration baselines, and narrow Codex facts. Prioritize `learn.chatgpt.com` for ChatGPT Work.

## Choose one primary route

Use the first matching route, and read its reference only when the requested task needs that specialized workflow:

- **Explicitly requested local documentation integration:** Read [integration guidance](references/mcp-diagnostics.md) only when the user explicitly requests that local integration.
- **Model migration, upgrades, or model-specific prompting:** Read [model-migration.md](references/model-migration.md) for actual migration planning, implementation, dynamic target resolution, or prompt changes. Preserve an explicitly requested target.
- **Model selection and comparisons:** Read [model-selection.md](references/model-selection.md) only when nuanced current, latest, default, cost, latency, quality, or modality tradeoffs need more guidance. Do not run a migration resolver for selection alone.
- **Product, API, ChatGPT Work, and mixed Chat/Work/Codex documentation:** Read [official-docs.md](references/official-docs.md) only when fetched official pages leave source selection, API schemas, or the requested implementation unresolved. This route is not manual-first.
- **Explicitly broad Codex setup, orientation, or cross-topic synthesis:** Read [codex-self-knowledge.md](references/codex-self-knowledge.md) when the eligible Codex manual or deeper Codex procedures are needed.

Read at most one primary reference. Do not open every route, bundled model guide, or helper script. Read a supporting reference or run a helper only when the chosen workflow demonstrably needs it.

## Source and execution boundaries

For the resolver branch, do not suppress or redirect its stdout. Success requires JSON containing `model`, `migrationGuideUrl`, and `promptingGuideUrl`; if the command exits without all three fields, run it once more before any fallback.

## Source Priority

- For Codex self-knowledge, use the Codex source route below; it owns when to use the manual helper, Docs MCP, or bounded uncertainty.
- For non-Codex OpenAI docs questions, use `mcp__openaiDeveloperDocs__search_openai_docs` to find the most relevant doc pages.
- For non-Codex OpenAI docs questions, fetch the relevant page with `mcp__openaiDeveloperDocs__fetch_openai_doc` before answering. If search is noisy, run a narrower Docs MCP search; when any plausible official OpenAI docs URL is known or found, try fetching that URL through Docs MCP before relying on web-search content.
- For API reference, schema, parameter, or required-field questions, use `mcp__openaiDeveloperDocs__get_openapi_spec` when available to verify the API shape alongside the relevant guide or reference page.
- Use `mcp__openaiDeveloperDocs__list_openai_docs` only when you need to browse or discover non-Codex pages without a clear query.
- For latest/current prompting guidance, model upgrades, or prompt upgrades, apply the first-action classifier above and use its platform-specific resolver command. On POSIX, the wrapper uses a compatible Node.js 18+ runtime from `$NODE`, PATH, or bundled/system fallbacks. On Windows, use a compatible Node.js executable to run the `.cjs` implementation directly. If no compatible runtime is available, call `load_workspace_dependencies` when available and retry the appropriate command once with the returned Node.js executable.
- For docs-only model-selection questions that do not ask to change an app, project, repo, model configuration, or prompts, fetch `https://developers.openai.com/api/docs/guides/latest-model.md` first. If that is unavailable, load `references/latest-model.md`.
- Preserve explicit target requests: if the user names a target model like "migrate to GPT-5.4", keep that requested target even if `latest-model.md` names a newer model. Mention newer guidance only as optional.
- If current remote guidance is needed, treat the returned migration and prompting guide URLs as opaque and fetch those exact URLs directly. Do not derive, substitute, or append a model query to them. If a prompting guide URL resolves to a combined model-guidance page, extract only the `## Prompting Best Practices` section through the next H2 heading. If a fetched guide contains only a title or no substantive body, retry the exact markdown URL through MCP/search fallback; if that also fails, use bundled fallback references and disclose the fallback.

## OpenAI product snapshots

1. Apps SDK: Build ChatGPT apps by providing a web component UI and an MCP server that exposes your app's tools to ChatGPT.
2. Responses API: A unified endpoint designed for stateful, multimodal, tool-using interactions in agentic workflows.
3. Chat Completions API: Generate a model response from a list of messages comprising a conversation.
4. Codex: OpenAI's coding agent for software development that can write, understand, review, and debug code.
5. gpt-oss: Open-weight OpenAI reasoning models (gpt-oss-120b and gpt-oss-20b) released under the Apache 2.0 license.
6. Realtime API: Build low-latency, multimodal experiences including natural speech-to-speech conversations.
7. Agents SDK: A toolkit for building agentic apps where a model can use tools and context, hand off to other agents, stream partial results, and keep a full trace.

## Codex self-knowledge

Use this path for questions about Codex itself: configuring, extending, operating, troubleshooting, local state, product surfaces, or where Codex behavior should live. A codebase merely mentioning a plugin, skill, hook, MCP server, browser, or automation is not enough. For generic software tasks, answer the software task directly; if asked whether Codex self-knowledge applies, answer that meta question briefly and continue the requested artifact.

### Source Route

The Codex manual is the first source for broad Codex synthesis. Treat the manual and Docs MCP as different lanes, not interchangeable official-doc sources. For published-user Codex product answers, the source route is complete: the manual, Docs MCP when this route calls for it, official OpenAI web fallback, and callable capabilities surfaced in the current session when the question is about that capability. Knowledge bases outside developers.openai.com are outside this route for public product answers.

For broad Codex behavior, setup, customization, skills, plugins, MCP, hooks, `AGENTS.md`, automations, surfaces, local state, or system-map questions:

1. Reuse a same-thread manual and outline path when it is still fresh.
2. Otherwise run the skill-local helper first in normal writable sessions. Skip it without trying only when the session is explicitly read-only, shell execution is unavailable, or visible policy shows no allowed temp cache.
3. By default, the helper chooses the first usable temp cache dir in this order: `$TMPDIR/openai-docs-cache`, `%TEMP%\openai-docs-cache`, `%TMP%\openai-docs-cache`, `/private/tmp/openai-docs-cache`, then `/tmp/openai-docs-cache`. Workspace-only write access is not enough for this temp cache.
4. Run the helper directly unless you need to override the cache dir. The helper falls back to `curl` when native `fetch` is unavailable or when proxy env vars are present, so no shell-specific proxy prefix is required. Resolve `<skill-dir>` to this skill's actual directory; in copied local eval workdirs this is usually `.codex/skills/openai-docs`:

```bash
node <skill-dir>/scripts/fetch-codex-manual.mjs
```

If you need to override the cache dir, pass `--cache-dir <cache-dir>`. On Windows, the helper checks `%TEMP%` and `%TMP%` automatically; in PowerShell, `$env:TEMP\\openai-docs-cache` is a typical explicit override.

Treat helper availability as established by explicit read-only/no-shell policy or an actual command result. A guessed sandbox or guessed helper failure is not enough to switch to Docs MCP or web lookup; after an actual helper command failure, continue to the narrowest official next source below.

The helper verifies freshness, writes `codex-manual.md`, and emits `codex-manual.outline.md`. The outline maps source pages and headings to line ranges; use it to choose the relevant manual section, then read or search targeted manual sections for Codex product facts. Use the skill directory to locate and run the helper; after the helper succeeds, use the returned manual and outline paths as the search scope for Codex product facts and term coverage checks.

Reuse the same-thread manual and outline paths for follow-up Codex questions. Refresh first when the manual was fetched more than about a day ago, the path is unusable, the path came from another thread or uncertain provenance, or likely-current information is missing and staleness is plausible.

For questions about whether the manual is current enough to rely on now, run the helper when temp caching is allowed and base the answer on its returned status, manual path, and outline path.

If the manual resolves a Codex claim, answer from it and stop expanding sources for that claim; continue the user's broader task if the docs lookup was only one dependency. Manual source pages and known anchors are enough citation support for manual-covered material.

If the helper is skipped because the session is read-only, has no shell execution, or has no allowed temp cache, the next source is Docs MCP: call `mcp__openaiDeveloperDocs__search_openai_docs`, then `mcp__openaiDeveloperDocs__fetch_openai_doc` for a relevant hit before any web fallback.

If a user names a Codex term or mode that a fresh manual does not use, search the manual for obvious adjacent concepts, then answer that the exact term is not documented and use the closest documented terminology. If the prompt asks how that term maps to Codex behavior, resolve the mapping from adjacent manual sections. If the exact term remains material or likely current after that manual pass, use one narrow Docs MCP search/fetch before bounded uncertainty; otherwise, the source lookup for that terminology or mapping claim is complete.

Use the narrowest official next source only when the manual is unavailable, the helper fails, temp caching is not allowed, another material claim is missing or likely stale, or the user explicitly needs a page-specific citation. Prefer one specific Docs MCP search and, if it returns a clearly relevant page, one fetch; for unresolved Codex capability names, acronyms, scheduling terms, or exact error text, this Docs MCP step is the next source before web search. After the manual plus any permitted Docs MCP gap-fill, resolve remaining gaps as bounded uncertainty. Use official-domain web fallback only after that Docs MCP path is unavailable or unhelpful. If the claim is still not established, stop with bounded uncertainty. If official docs/manual conflict with a callable capability already surfaced in the current session, state the conflict and prefer verified current-session behavior for that environment.

For undocumented or private-looking model slugs, product mode labels, entitlement labels, account access paths, or rollout names, answer from current public docs and bounded uncertainty. Those labels are not a reason to leave the public source route.

For support-style diagnostics, prefer a layer-by-layer answer from the manual over provider-specific web lookups: installed/enabled plugin, bundled app or connector authorization, MCP setup, workspace/admin policy, restart or new-thread expectations, then support or feedback if still unresolved.

If the source route still does not establish a claim, return bounded uncertainty or route to support, an admin, or product feedback instead of widening the investigation.

For unresolved product terminology, answer from the manual plus the allowed official next source. If those sources do not establish the term, answer with bounded uncertainty from those sources.

### Surface Map

When Codex nouns or durable-instruction surfaces overlap, recommend the smallest surface that matches the scope:

- Prompt or thread context -> one-off task constraints.
- `AGENTS.md` -> durable repo conventions, commands, verification steps, and review expectations; closer nested files apply under their subtree.
- Project `.codex/config.toml` -> trusted-repo Codex settings such as sandbox, MCP, hooks, model, or reasoning defaults.
- Global config or global guidance -> personal defaults across repos.
- Skill -> reusable task workflow with references or scripts.
- Plugin -> installable bundle with skills plus commands, tools, MCP config, hooks, assets, apps, or marketplace metadata.
- MCP server or app connector -> live external data/actions or authorized private app/workspace data. Use connectors for private Google Docs, Calendar, Slack, GitHub, Notion, and similar data instead of web search or model memory.
- Automation -> scheduled checks, reminders, monitors, or follow-up work; use a thread heartbeat when continuity in an existing thread matters.
- Hook -> lifecycle enforcement around tool calls, commands, or file edits.

Split mixed-scope requests instead of forcing one answer. Example: "always do X, but only for this PR" defaults to prompt/thread context for the current run; use `AGENTS.md` or project config only if it should persist, hooks only for mechanical enforcement, and automations only for scheduled or follow-up work.

Use this quick product map when needed: CLI is terminal-first local repo work; IDE extension is editor-attached coding; Codex app is desktop planning, review, and interactive work; cloud/web is hosted parallel/offloaded work; Browser Use/in-app browser is Codex-controlled web testing; Chrome extension uses the user's Chrome profile; Computer Use controls desktop apps and OS UI. Keep `config.toml` defaults, `requirements.toml` constraints, and managed/admin policy separate.

### Boundaries And Output

- API key auth does not imply ChatGPT, cloud task, or connector access. For plugin/app/auth failures, check bundle availability, plugin installed/enabled state, connector/app authorization, MCP setup, restart/refresh expectations, workspace policy, and per-surface availability before answering.
- Sandbox or network denials need scoped escalation with a clear justification. Destructive commands, writes outside the workspace, or broad access changes require explicit approval.
- Memory can provide user preference or context, but explicit prompt instructions win and memory is not a source for current external facts.
- For affirmative surface-selection answers, use this shape: recommendation, why, what to avoid, and the manual/source evidence used.
- When page-specific Codex citations are actually needed, these anchors often fit: `concepts/customization#agents-guidance` for `AGENTS.md`, `concepts/customization#skills` for skills, `plugins/build#plugin-structure` for plugins, `concepts/customization#mcp` for MCP, `config-advanced#hooks` for hooks, `app/automations#thread-automations` for thread automations, and `config-reference#configtoml` for config.

## If MCP server is missing

If MCP tools fail or no OpenAI docs resources are available:

1. Run the install command yourself: `aicodex mcp add openaiDeveloperDocs --url https://developers.openai.com/mcp`
2. If it fails due to permissions/sandboxing, immediately retry the same command with escalated permissions and include a 1-sentence justification for approval. Do not ask the user to run it yet.
3. Only if the escalated attempt fails, ask the user to run the install command.
4. Ask the user to restart Codex.
5. Re-run the doc search/fetch after restart.

## Workflow

1. Clarify whether the request is general docs lookup, model selection, a model-string upgrade, prompt-upgrade guidance, or broader API/provider migration.
2. For Codex self-knowledge requests, follow the Codex self-knowledge source procedure above.
3. For model-selection or upgrade requests, prefer current remote docs over bundled references when the user asks for latest/current/default guidance.
   - For docs-only model-selection questions, fetch `https://developers.openai.com/api/docs/guides/latest-model.md`, find the latest model ID and explicit migration or prompt-guidance links, and prefer explicit links over derived URLs.
   - For explicit named-model requests, preserve the requested model target. Mention newer remote guidance only as optional.
   - For latest/current/default prompting guidance or dynamic upgrades, run the resolver with the platform-specific command above as the first docs operation before any direct latest-model.md fetch or API-key credential gate, then fetch both returned guide URLs directly when possible.
   - Fetch returned guide URLs exactly; do not derive or substitute model-specific URLs.
   - If a direct guide fetch fails or returns only a title, use the developer-docs MCP tools or official OpenAI-domain search to find the same guide content.
   - If remote docs are unavailable, use bundled fallback references and say that fallback guidance was used.
4. For model upgrades, keep changes behavior-preserving and scoped: update active OpenAI API model defaults, directly related prompts, and the registries, routing, pricing, capability, or picker surfaces that the user actually placed in scope.
5. Leave historical docs, examples, eval baselines, fixtures, provider comparisons, intentionally pinned fallbacks, and ambiguous older model usage unchanged unless the user explicitly asks to upgrade them. Do not collapse a multi-model router or picker into one flagship model; preserve the existing cost, latency, and quality roles.
6. Keep SDK, tooling, IDE, plugin, shell, auth, and provider-environment migrations out of a model-and-prompt upgrade unless the user explicitly asks for them.
7. If a safe upgrade needs API-surface changes, schema rewiring, tool-handler changes, or broader implementation work, classify it explicitly. Make those changes when implementation is within the user's requested scope; otherwise report the exact blocker or confirmation needed instead of silently changing behavior.
8. For general docs lookup, start with a compact, title-like search query of 2-6 essential terms. Do not turn the full user question into a keyword list. Fetch the best page and exact section needed, and answer with concise citations.

## Reference map

Read only what you need:

- `https://developers.openai.com/api/docs/guides/latest-model.md` -> current model-selection and "best/latest/current model" questions.
- `scripts/fetch-codex-manual.mjs` -> current Codex manual fetch, verification, local temp cache, and outline generation.
- `https://developers.openai.com/codex/codex-manual.md` -> current Codex self-knowledge synthesis, including setup, customization, skills, plugins, MCP, hooks, `AGENTS.md`, automations, and surface behavior; normally access it through the helper path and targeted file reads when temp caching is available.
- `references/latest-model.md` -> bundled fallback for model-selection and "best/latest/current model" questions.
- `references/upgrade-guide.md` -> bundled routing fallback for model upgrade and upgrade-planning requests.
- `references/upgrading-to-gpt-5p6-sol.md` -> GPT-5.6 Sol/family migration judgment, compatibility gates, optional feature boundaries, and validation.
- `references/prompting-guide.md` -> bundled GPT-5.6 prompting fallback plus the live Prompting Best Practices extraction contract.

## Quality rules

- Treat OpenAI docs as the source of truth; avoid speculation.
- For Codex self-knowledge, follow the source route above instead of relying on remembered behavior.
- Keep migration changes narrow and behavior-preserving.
- Prefer prompt-only upgrades when possible.
- Avoid inventing pricing, availability, parameters, API changes, or breaking changes.
- Keep quotes short and within policy limits; prefer paraphrase with citations.
- If multiple pages differ, call out the difference and cite both.
- If official docs and verified callable current-session behavior disagree, state the conflict before making broad claims or edits.
- If docs do not cover the user’s need, say so and offer next steps.

## Tooling notes

- Use MCP doc tools before web search for OpenAI-related markdown docs. The Codex manual flow is the exception: follow the Codex self-knowledge source procedure for broad Codex synthesis.
- If the MCP server is installed but returns no meaningful results, then use web search as a fallback.
- When falling back to web search, restrict to official OpenAI domains (developers.openai.com, platform.openai.com) and cite sources.
 - Search, open, fetch, and cite only `developers.openai.com`, `platform.openai.com`, and `learn.chatgpt.com`. Cite the page that supports the claim. State uncertainty when official sources do not establish pricing, availability, account access, limits, or behavior.
 - Preserve an explicitly requested model for selection, migration, and prompting. Resolve an unspecified latest or current migration target only after searching and fetching current official guidance.
 - Say "OpenAI Docs" or "official OpenAI documentation" in user-facing answers. Keep exact official citations and examples concise.
