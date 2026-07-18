use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ToolInvocation;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::hook_names::HookToolName;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::runtimes::apply_patch::ApplyPatchRuntime;
use crate::tools::runtimes::apply_patch::ConditionalWriteRequest;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::default_exec_approval_requirement;
use codex_apply_patch::AppliedPatchChange;
use codex_apply_patch::AppliedPatchDelta;
use codex_apply_patch::AppliedPatchFileChange;
use codex_file_system::ConditionalWritePrecondition;
use codex_protocol::protocol::FileChange;
use codex_utils_path_uri::PathUri;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) struct ReviewableFileMutation<'a> {
    pub path: &'a PathUri,
    pub canonical_path: &'a PathUri,
    pub contents: Vec<u8>,
    pub precondition: ConditionalWritePrecondition,
    pub old_text: Option<&'a str>,
    pub new_text: &'a str,
}

pub(super) async fn commit_reviewable_mutation(
    invocation: &ToolInvocation,
    environment: &TurnEnvironment,
    mutation: ReviewableFileMutation<'_>,
) -> Result<(), FunctionCallError> {
    let (changes, patch, delta) =
        mutation_artifacts(mutation.path, mutation.old_text, mutation.new_text);
    let hook_input = match &invocation.payload {
        crate::tools::context::ToolPayload::Function { arguments } => {
            serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.clone()))
        }
        _ => Value::Null,
    };
    let request = ConditionalWriteRequest {
        turn_environment: environment.clone(),
        path: mutation.path.clone(),
        canonical_path: mutation.canonical_path.clone(),
        contents: mutation.contents,
        precondition: mutation.precondition,
        changes: changes.clone(),
        patch,
        delta,
        exec_approval_requirement: default_exec_approval_requirement(
            invocation.turn.approval_policy.value(),
            &invocation.turn.file_system_sandbox_policy(),
        ),
        hook_tool_name: HookToolName::new(invocation.tool_name.to_string()),
        hook_input,
    };
    let emitter = ToolEmitter::apply_patch_for_environment(
        changes,
        /*auto_approved*/ false,
        environment.environment_id.clone(),
    );
    let event_ctx = ToolEventCtx::new(
        invocation.session.as_ref(),
        invocation.turn.as_ref(),
        &invocation.call_id,
        Some(&invocation.tracker),
    );
    emitter.begin(event_ctx).await;

    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = ApplyPatchRuntime::new();
    let tool_ctx = ToolCtx {
        session: invocation.session.clone(),
        turn: invocation.turn.clone(),
        call_id: invocation.call_id.clone(),
        tool_name: invocation.tool_name.clone(),
    };
    let result = orchestrator
        .run(
            &mut runtime,
            &request,
            &tool_ctx,
            invocation.turn.as_ref(),
            invocation.turn.approval_policy.value(),
        )
        .await
        .map(|result| result.output);
    let (event_result, delta, failure) = match result {
        Ok(output) if output.exec_output.exit_code == 0 => {
            (Ok(output.exec_output), Some(output.delta), None)
        }
        Ok(output) => {
            let message = output.exec_output.stderr.text.clone();
            (Ok(output.exec_output), Some(output.delta), Some(message))
        }
        Err(error) => (Err(error), Some(runtime.committed_delta().clone()), None),
    };
    let event_ctx = ToolEventCtx::new(
        invocation.session.as_ref(),
        invocation.turn.as_ref(),
        &invocation.call_id,
        Some(&invocation.tracker),
    );
    emitter
        .finish(event_ctx, event_result, delta.as_ref())
        .await?;
    if let Some(message) = failure {
        return Err(FunctionCallError::RespondToModel(format!(
            "file operation failed: {message}"
        )));
    }
    Ok(())
}

fn mutation_artifacts(
    path: &PathUri,
    old_text: Option<&str>,
    new_text: &str,
) -> (HashMap<PathBuf, FileChange>, String, AppliedPatchDelta) {
    let (tracked_change, change, patch) = match old_text {
        Some(old_text) => {
            let old = bounded_event_text(old_text);
            let new = bounded_event_text(new_text);
            let patch = format!("--- {path}\n+++ {path}\n@@\n-{old}\n+{new}\n");
            (
                AppliedPatchFileChange::Update {
                    move_path: None,
                    old_content: old_text.to_string(),
                    overwritten_move_content: None,
                    new_content: new_text.to_string(),
                },
                FileChange::Update {
                    unified_diff: patch.clone(),
                    move_path: None,
                },
                patch,
            )
        }
        None => {
            let content = bounded_event_text(new_text);
            (
                AppliedPatchFileChange::Add {
                    content: new_text.to_string(),
                    overwritten_content: None,
                },
                FileChange::Add {
                    content: content.clone(),
                },
                format!("*** Add File: {path}\n{content}"),
            )
        }
    };
    let delta = AppliedPatchDelta::from_change(AppliedPatchChange {
        path: path.to_path_buf(),
        change: tracked_change,
    });
    (HashMap::from([(path.to_path_buf(), change)]), patch, delta)
}

fn bounded_event_text(text: &str) -> String {
    const MAX_EVENT_BYTES: usize = 64 * 1024;
    if text.len() <= MAX_EVENT_BYTES {
        return text.to_string();
    }
    format!(
        "{}\n[event content truncated]",
        &text[..text.floor_char_boundary(MAX_EVENT_BYTES)]
    )
}
