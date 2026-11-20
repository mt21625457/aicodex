//! Settles completed responses and releases output according to the configured processing mode.

use crate::client_common::ChargePosting;
use crate::client_common::ResponseEvent;
use crate::client_common::SamplingCharge;
use crate::client_common::SamplingEvent;
use crate::hook_runtime::sampling::SamplingAttempt;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use codex_api::ApiError;
use codex_hooks::ModelRequestHookEvent;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

const MAX_BUFFERED_EVENTS: usize = 65_535;

#[derive(Default)]
pub(super) struct ResponseDelivery {
    attempt: Option<SamplingAttempt>,
    held: Vec<ResponseEvent>,
    pub(super) settlement_started: Arc<AtomicBool>,
    pub(super) incomplete_provider_usage_recorded: Arc<AtomicBool>,
    settlement: Option<(Arc<Session>, Arc<StepContext>)>,
    ordinary_sampling: bool,
}

impl ResponseDelivery {
    pub(super) fn new(
        attempt: &mut Option<SamplingAttempt>,
        settlement: Option<(Arc<Session>, Arc<StepContext>)>,
        ordinary_sampling: bool,
    ) -> Self {
        Self {
            attempt: attempt.take(),
            held: Vec::new(),
            settlement_started: Default::default(),
            incomplete_provider_usage_recorded: Default::default(),
            settlement,
            ordinary_sampling,
        }
    }

    pub(super) fn would_overflow(
        &self,
        event: &std::result::Result<ResponseEvent, ApiError>,
    ) -> bool {
        self.held.len() >= MAX_BUFFERED_EVENTS && event.as_ref().is_ok_and(|item| self.holds(item))
    }

    fn holds(&self, item: &ResponseEvent) -> bool {
        !matches!(
            item,
            ResponseEvent::Completed { .. } | ResponseEvent::SafetyBuffering(_)
        ) && self.settlement.as_ref().is_some_and(|(_, step)| {
            step.turn.config.response_process_mode
                == codex_config::config_toml::ResponseProcessMode::AfterCompleteResponse
        })
    }

    /// Return false when the turn has stopped receiving this attempt's output.
    pub(super) async fn send_or_hold(
        &mut self,
        event: Result<ResponseEvent>,
        tx: &mpsc::Sender<SamplingEvent>,
    ) -> bool {
        let passthrough = self.attempt.is_none() && self.settlement.is_none();
        if passthrough {
            return tx.send(event.into()).await.is_ok();
        }
        let report = match &event {
            Ok(ResponseEvent::Completed {
                response_id,
                token_usage,
                ..
            }) => ModelRequestHookEvent::completed(response_id, token_usage.as_ref()),
            Err(error) => ModelRequestHookEvent::failed(error),
            Ok(item) if self.holds(item) => {
                if let Ok(item) = event {
                    self.held.push(item);
                }
                return true;
            }
            Ok(_) => return tx.send(event.into()).await.is_ok(),
        };
        let hooks = self.attempt.as_mut().and_then(|attempt| {
            let owner = attempt.owner_cancellation.clone();
            attempt.start_post_hooks(report).map(|hooks| (owner, hooks))
        });
        let mut incomplete_provider_usage_recorded = false;
        let hook_output = if let Some((owner, hooks)) = hooks {
            if self.ordinary_sampling
                && let Some((session, step)) = &self.settlement
                && let Err(error) = &event
                && let CodexErrorDetails::ResponseIncomplete(incomplete) = error.details()
                && incomplete.token_usage.is_some()
            {
                // An interrupt can cancel the hook after the provider has reported a bill.
                session
                    .record_incomplete_response_usage(
                        &step.turn,
                        &step.settings,
                        incomplete,
                        SamplingCharge::default(),
                    )
                    .await;
                incomplete_provider_usage_recorded = true;
                self.incomplete_provider_usage_recorded
                    .store(true, Ordering::Release);
            }
            tokio::select! {
                biased;
                () = tx.closed() => return false,
                () = owner.cancelled() => return false,
                output = hooks => output,
            }
        } else {
            Default::default()
        };
        self.attempt = None;
        let terminal = SamplingEvent {
            event,
            hook_output: SamplingCharge {
                hooks: hook_output,
                incomplete_provider_usage_recorded,
                ..Default::default()
            },
        };
        let released = self.release(terminal, tx).await;
        match released {
            Ok(delivered) => delivered,
            Err(error) => tx.send(Err(error).into()).await.is_ok(),
        }
    }

    async fn release(
        &mut self,
        mut terminal: SamplingEvent,
        tx: &mpsc::Sender<SamplingEvent>,
    ) -> Result<bool> {
        if self.settlement.is_some()
            && matches!(terminal.event, Ok(ResponseEvent::Completed { .. }))
        {
            self.settlement_started.store(true, Ordering::Release);
        }
        if let Some((session, step)) = &self.settlement
            && let Ok(ResponseEvent::Completed { token_usage, .. }) = &terminal.event
        {
            let mut usage = crate::client_common::SamplingUsage {
                provider: token_usage.clone(),
                charge: std::mem::take(&mut terminal.hook_output),
            };
            usage.charge.posting = match session
                .record_rollout_budget_usage(&step.turn.sub_id, &usage)
                .await
            {
                Ok(()) => ChargePosting::Posted,
                Err(error)
                    if matches!(error.details(), CodexErrorDetails::SessionBudgetExceeded) =>
                {
                    ChargePosting::PostedTurnExhausted
                }
                Err(error) => return Err(error),
            };
            terminal.hook_output = usage.charge;
        }
        if terminal.event.is_ok() {
            for item in std::mem::take(&mut self.held) {
                if tx.send(Ok(item).into()).await.is_err() {
                    return Ok(false);
                }
            }
        }
        Ok(tx.send(terminal).await.is_ok())
    }

    pub(super) async fn finish_eof(&mut self, tx: &mpsc::Sender<SamplingEvent>) {
        let Some(mut attempt) = self.attempt.take() else {
            return;
        };
        let owner = attempt.owner_cancellation.clone();
        if let Some(hooks) = attempt.start_post_hooks(ModelRequestHookEvent::failed(
            &CodexErr::Stream("stream closed before response.completed".into()),
        )) {
            tokio::select! {
                biased;
                () = tx.closed() => {}
                () = owner.cancelled() => {}
                _ = hooks => {}
            }
        }
    }
}
