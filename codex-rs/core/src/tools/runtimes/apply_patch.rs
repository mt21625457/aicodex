//! Apply Patch runtime: executes verified patches under the orchestrator.
//!
//! Assumes `apply_patch` verification/approval happened upstream. Reuses the
//! selected turn environment filesystem for both local and remote turns, with
//! sandboxing enforced by the explicit filesystem sandbox context.
use crate::exec::is_likely_sandbox_denied;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalAction;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::executor_windows_sandbox_level;
use codex_apply_patch::AppliedPatchDelta;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::ApplyPatchOptions;
use codex_exec_server::FileSystemSandboxContext;
use codex_file_system::ConditionalWritePrecondition;
use codex_protocol::error::CodexErr;
use codex_protocol::error::SandboxErr;
use codex_protocol::exec_output::ExecToolCallOutput;
use codex_protocol::exec_output::StreamOutput;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
use codex_sandboxing::SandboxType;
use codex_sandboxing::SandboxablePreference;
use codex_sandboxing::is_likely_executor_managed_sandbox_denied;
use codex_sandboxing::policy_transforms::effective_permission_profile;
use codex_sandboxing::record_filesystem_sandbox_violation;
use codex_utils_path_uri::PathUri;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize)]
pub(crate) struct ApplyPatchApprovalKey {
    pub(crate) environment_id: String,
    pub(crate) path: PathUri,
}

#[derive(Debug)]
pub struct ApplyPatchRequest {
    pub turn_environment: TurnEnvironment,
    pub action: ApplyPatchAction,
    pub file_paths: Vec<PathUri>,
    pub changes: Arc<std::collections::HashMap<PathBuf, FileChange>>,
    pub exec_approval_requirement: ExecApprovalRequirement,
    pub additional_permissions: Option<AdditionalPermissionProfile>,
    pub permissions_preapproved: bool,
}

#[derive(Debug)]
pub struct ConditionalWriteRequest {
    pub turn_environment: TurnEnvironment,
    pub path: PathUri,
    pub canonical_path: PathUri,
    pub contents: Vec<u8>,
    pub precondition: ConditionalWritePrecondition,
    pub changes: HashMap<PathBuf, FileChange>,
    pub patch: String,
    pub delta: AppliedPatchDelta,
    pub exec_approval_requirement: ExecApprovalRequirement,
}

#[derive(Default)]
pub struct ApplyPatchRuntime {
    committed_delta: AppliedPatchDelta,
}

#[derive(Debug)]
pub struct ApplyPatchRuntimeOutput {
    pub exec_output: ExecToolCallOutput,
    pub delta: AppliedPatchDelta,
}

impl ApplyPatchRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn committed_delta(&self) -> &AppliedPatchDelta {
        &self.committed_delta
    }

    fn build_approval_action(req: &ApplyPatchRequest, call_id: &str) -> ApprovalAction {
        ApprovalAction::ApplyPatch {
            id: call_id.to_string(),
            environment_id: req.turn_environment.selection.environment_id.clone(),
            cwd: req.action.cwd.clone(),
            files: req.file_paths.clone(),
            patch: req.action.patch.clone(),
            changes: Arc::clone(&req.changes),
            permissions_preapproved: req.permissions_preapproved,
        }
    }

    fn file_system_sandbox_context_for_attempt(
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
    ) -> Option<FileSystemSandboxContext> {
        file_system_sandbox_context_for_attempt(
            req.additional_permissions.as_ref(),
            attempt,
            req.turn_environment.user_home_dir.clone(),
        )
    }
}

fn file_system_sandbox_context_for_attempt(
    additional_permissions: Option<&AdditionalPermissionProfile>,
    attempt: &SandboxAttempt<'_>,
    user_home_dir: Option<PathUri>,
) -> Option<FileSystemSandboxContext> {
    if !attempt.sandbox_requested {
        return None;
    }

    let permissions =
        effective_permission_profile(attempt.exec_server_permissions, additional_permissions);
    Some(FileSystemSandboxContext {
        permissions: permissions.into(),
        cwd: Some(attempt.sandbox_cwd.clone()),
        workspace_roots: attempt.workspace_roots.to_vec(),
        user_home_dir,
        temporary_directories: None,
        windows_sandbox_level: executor_windows_sandbox_level(
            attempt.windows_sandbox_level,
            attempt.sandbox_cwd,
        ),
        windows_sandbox_private_desktop: attempt.windows_sandbox_private_desktop,
        windows_sandbox_proxy_settings_mode: None,
        use_legacy_landlock: attempt.use_legacy_landlock,
    })
}

async fn run_conditional_write_with_sandbox<T>(
    req: &ConditionalWriteRequest,
    attempt: &SandboxAttempt<'_>,
    write: impl FnOnce(Option<FileSystemSandboxContext>) -> T,
) -> T::Output
where
    T: Future,
{
    let sandbox = file_system_sandbox_context_for_attempt(
        /*additional_permissions*/ None,
        attempt,
        req.turn_environment.user_home_dir.clone(),
    );
    write(sandbox).await
}

impl Sandboxable for ApplyPatchRuntime {
    fn sandbox_preference(&self) -> SandboxablePreference {
        SandboxablePreference::Auto
    }
    fn escalate_on_failure(&self) -> bool {
        true
    }
}

impl Approvable<ApplyPatchRequest> for ApplyPatchRuntime {
    fn approval_action(
        &self,
        req: &ApplyPatchRequest,
        call_id: &str,
    ) -> std::io::Result<ApprovalAction> {
        Ok(ApplyPatchRuntime::build_approval_action(req, call_id))
    }

    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        match policy {
            AskForApproval::Never => false,
            AskForApproval::Granular(granular_config) => granular_config.allows_sandbox_approval(),
            AskForApproval::OnRequest => true,
            AskForApproval::UnlessTrusted => true,
        }
    }

    // apply_patch approvals are decided upstream by assess_patch_safety.
    //
    // This override ensures the orchestrator runs the patch approval flow when required instead
    // of falling back to the global exec approval policy.
    fn exec_approval_requirement(
        &self,
        req: &ApplyPatchRequest,
    ) -> Option<ExecApprovalRequirement> {
        Some(req.exec_approval_requirement.clone())
    }
}

impl ToolRuntime<ApplyPatchRequest, ApplyPatchRuntimeOutput> for ApplyPatchRuntime {
    fn turn_environment<'a>(&self, req: &'a ApplyPatchRequest) -> &'a TurnEnvironment {
        &req.turn_environment
    }

    fn uses_executor_managed_process_sandbox(&self, req: &ApplyPatchRequest) -> bool {
        req.turn_environment.environment.is_remote()
    }

    fn sandbox_cwd<'a>(&self, req: &'a ApplyPatchRequest) -> Option<&'a PathUri> {
        Some(&req.action.cwd)
    }

    async fn run(
        &mut self,
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<ApplyPatchRuntimeOutput, ToolError> {
        let started_at = Instant::now();
        let fs = req.turn_environment.environment.get_filesystem();
        let sandbox = ApplyPatchRuntime::file_system_sandbox_context_for_attempt(req, attempt);
        let lock_paths = canonical_lock_paths(fs.as_ref(), &req.file_paths, sandbox.as_ref()).await;
        let _mutation_guards = ctx
            .step_context
            .turn
            .file_mutation_locks
            .lock_paths(&req.turn_environment.selection.environment_id, &lock_paths)
            .await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = codex_apply_patch::apply_patch_with_options(
            &req.action.patch,
            ApplyPatchOptions {
                update_file_mode: req.action.update_file_mode(),
                // Only reject links when an otherwise-required sandbox was bypassed.
                // Executor-managed sandboxes can have SandboxType::None.
                follow_symlinks: attempt.sandbox_requested
                    || !attempt.manager.should_sandbox(
                        attempt.permissions,
                        self.sandbox_preference(),
                        attempt.enforce_managed_network,
                    ),
            },
            &req.action.cwd,
            &mut stdout,
            &mut stderr,
            fs.as_ref(),
            sandbox.as_ref(),
        )
        .await;
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        let failed = result.is_err();
        let exit_code = if failed { 1 } else { 0 };
        let delta = match result {
            Ok(delta) => delta,
            Err(failure) => failure.into_parts().1,
        };
        self.committed_delta.append(delta);
        let output = ExecToolCallOutput {
            exit_code,
            stdout: StreamOutput::new(stdout.clone()),
            stderr: StreamOutput::new(stderr.clone()),
            aggregated_output: StreamOutput::new(format!("{stdout}{stderr}")),
            duration: started_at.elapsed(),
            timed_out: false,
        };
        let sandbox_denied = failed
            && if attempt.sandbox == SandboxType::None {
                attempt.sandbox_requested && is_likely_executor_managed_sandbox_denied(&output)
            } else {
                is_likely_sandbox_denied(attempt.sandbox, &output)
            };
        if sandbox_denied {
            // TODO(iceweasel): Report executor filesystem sandbox backends like process/start so
            // executor-managed apply_patch denials can emit backend-specific violation telemetry.
            if attempt.sandbox != SandboxType::None {
                record_filesystem_sandbox_violation(attempt.sandbox, &output);
            }
            return Err(ToolError::Codex(CodexErr::Sandbox(SandboxErr::Denied {
                output: Box::new(output),
                network_policy_decision: None,
            })));
        }
        Ok(ApplyPatchRuntimeOutput {
            exec_output: output,
            delta: self.committed_delta.clone(),
        })
    }
}

impl Approvable<ConditionalWriteRequest> for ApplyPatchRuntime {
    fn approval_action(
        &self,
        req: &ConditionalWriteRequest,
        call_id: &str,
    ) -> std::io::Result<ApprovalAction> {
        Ok(ApprovalAction::ApplyPatch {
            id: call_id.to_string(),
            environment_id: req.turn_environment.selection.environment_id.clone(),
            cwd: req.turn_environment.cwd().clone(),
            files: vec![req.path.clone()],
            patch: req.patch.clone(),
            changes: Arc::new(req.changes.clone()),
            permissions_preapproved: false,
        })
    }

    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        match policy {
            AskForApproval::Never => false,
            AskForApproval::Granular(granular_config) => granular_config.allows_sandbox_approval(),
            AskForApproval::OnRequest | AskForApproval::UnlessTrusted => true,
        }
    }

    fn exec_approval_requirement(
        &self,
        req: &ConditionalWriteRequest,
    ) -> Option<ExecApprovalRequirement> {
        Some(req.exec_approval_requirement.clone())
    }
}

impl ToolRuntime<ConditionalWriteRequest, ApplyPatchRuntimeOutput> for ApplyPatchRuntime {
    fn turn_environment<'a>(&self, req: &'a ConditionalWriteRequest) -> &'a TurnEnvironment {
        &req.turn_environment
    }

    fn uses_executor_managed_process_sandbox(&self, req: &ConditionalWriteRequest) -> bool {
        req.turn_environment.environment.is_remote()
    }

    fn sandbox_cwd<'a>(&self, req: &'a ConditionalWriteRequest) -> Option<&'a PathUri> {
        Some(req.turn_environment.cwd())
    }

    async fn run(
        &mut self,
        req: &ConditionalWriteRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<ApplyPatchRuntimeOutput, ToolError> {
        let started_at = Instant::now();
        let file_system = req.turn_environment.environment.get_filesystem();
        let _mutation_guard = ctx
            .step_context
            .turn
            .file_mutation_locks
            .lock_paths(
                &req.turn_environment.selection.environment_id,
                std::slice::from_ref(&req.canonical_path),
            )
            .await;
        let result = run_conditional_write_with_sandbox(req, attempt, |sandbox| async move {
            file_system
                .write_file_conditional(
                    &req.path,
                    req.contents.clone(),
                    req.precondition,
                    sandbox.as_ref(),
                )
                .await
        })
        .await;
        let (exit_code, stderr, delta) = match result {
            Ok(()) => {
                self.committed_delta.append(req.delta.clone());
                (0, String::new(), req.delta.clone())
            }
            Err(error) => {
                let message = if matches!(
                    error.kind(),
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::InvalidData
                ) {
                    format!("conditional write conflict: {error}")
                } else {
                    error.to_string()
                };
                (1, message, AppliedPatchDelta::default())
            }
        };
        let output = ExecToolCallOutput {
            exit_code,
            stdout: StreamOutput::new(String::new()),
            stderr: StreamOutput::new(stderr.clone()),
            aggregated_output: StreamOutput::new(stderr),
            duration: started_at.elapsed(),
            timed_out: false,
        };
        if exit_code != 0 && is_likely_sandbox_denied(attempt.sandbox, &output) {
            return Err(ToolError::Codex(CodexErr::Sandbox(SandboxErr::Denied {
                output: Box::new(output),
                network_policy_decision: None,
            })));
        }
        Ok(ApplyPatchRuntimeOutput {
            exec_output: output,
            delta,
        })
    }
}

async fn canonical_lock_paths(
    fs: &dyn codex_exec_server::ExecutorFileSystem,
    paths: &[PathUri],
    sandbox: Option<&FileSystemSandboxContext>,
) -> Vec<PathUri> {
    let mut canonical = Vec::with_capacity(paths.len());
    for path in paths {
        let resolved = match fs.canonicalize(path, sandbox).await {
            Ok(path) => path,
            Err(_) => {
                let Some(parent) = path.parent() else {
                    canonical.push(path.clone());
                    continue;
                };
                let Some(basename) = path.basename() else {
                    canonical.push(path.clone());
                    continue;
                };
                match fs.canonicalize(&parent, sandbox).await {
                    Ok(parent) => parent.join(&basename).unwrap_or_else(|_| path.clone()),
                    Err(_) => path.clone(),
                }
            }
        };
        canonical.push(resolved);
    }
    canonical
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
