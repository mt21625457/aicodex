# Claude Native Protocol Manual Checks

Use these checks when validating a Claude Messages provider against the native
protocol plan.

1. Anthropic native text editor
   - Configure an Anthropic Claude Messages provider with `apply_patch`
     available.
   - Ask Claude to create, view, replace, and insert text in a workspace file.
   - Verify the request advertises `str_replace_based_edit_tool` with a
     `text_editor_*` type and that mutations emit the same patch/update events
     as `apply_patch`.
   - Repeat with an absolute path outside the workspace and verify the tool
     result is an error and no file is written.

2. DeepSeek Claude fallback
   - Configure the DeepSeek Claude-compatible endpoint.
   - Ask for a file edit.
   - Verify the request includes the `apply_patch` fallback tool and does not
     include `text_editor_*` or `bash_20250124`.

3. Native bash
   - Ask Claude to run a simple command through native `bash`.
   - Verify stdout/stderr/exit status are returned through the existing shell
     result path.
   - Repeat with a command that requires approval and deny it. Verify the
     returned Claude `tool_result` has `is_error: true`.

4. Server tools
   - Enable a policy that allows the desired Anthropic server tool version.
   - Verify supported models can emit `web_search_20260209`,
     `web_fetch_20260209`, `code_execution_20260120`, and `advisor_20260301`
     when their capability gates pass.
   - Verify unsupported platforms fall back to older tools or disabled/local
     behavior before a request is sent.
   - Confirm server result blocks are preserved in the next request and are not
     routed through local tool execution.

5. Remote MCP connector
   - Configure one HTTPS remote MCP server in native mode.
   - Verify the request contains `mcp_servers`, one matching `mcp_toolset`,
     and the `mcp-client-2025-11-20` beta header.
   - Verify auth tokens are not present in debug logs or user-visible
     transcripts.
   - Configure an HTTP or duplicate-name server and verify native MCP is
     rejected locally.

6. Native tool search
   - Use a large tool catalog with native tool search enabled.
   - Verify `tool_search_tool_regex_20251119` or
     `tool_search_tool_bm25_20251119` is present, the tool-search tool itself
     is not deferred, and ordinary tools remain listed with `defer_loading`
     applied only after the critical non-deferred prefix.
   - Verify follow-up history preserves `tool_search_tool_result` and
     `tool_reference` blocks as provider state.

7. Citations
   - Trigger web-search and document citations.
   - Verify visible source markers are rendered for compatibility and final
     message content also carries structured citation metadata including
     encrypted index, document index, page range, character range, and cited
     text when provided.
