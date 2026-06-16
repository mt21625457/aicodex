use std::sync::Arc;
use std::time::Duration;

use codex_protocol::ThreadId;
use codex_protocol::models::BaseInstructions;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ExecCommandEndEvent;
use codex_protocol::protocol::ExecCommandSource;
use codex_protocol::protocol::ExecCommandStatus;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_rollout::EventPersistenceMode;
use pretty_assertions::assert_eq;

use super::LiveThread;
use crate::CreateThreadParams;
use crate::InMemoryThreadStore;
use crate::LoadThreadHistoryParams;
use crate::ThreadPersistenceMetadata;
use crate::ThreadStore;

#[tokio::test]
async fn append_items_uses_limited_persistence_by_default() {
    let store = Arc::new(InMemoryThreadStore::default());
    let thread_id = ThreadId::default();
    let live_thread = LiveThread::create(store.clone(), create_thread_params(thread_id))
        .await
        .expect("create live thread");

    live_thread
        .append_items(&[exec_command_end_item()])
        .await
        .expect("append items");

    let history = store
        .load_history(LoadThreadHistoryParams {
            thread_id,
            include_archived: true,
        })
        .await
        .expect("load history");
    assert_eq!(
        history.items.len(),
        1,
        "only the session metadata should be persisted"
    );
}

#[tokio::test]
async fn append_items_with_extended_persistence_keeps_command_completion() {
    let store = Arc::new(InMemoryThreadStore::default());
    let thread_id = ThreadId::default();
    let live_thread = LiveThread::create(store.clone(), create_thread_params(thread_id))
        .await
        .expect("create live thread");

    live_thread
        .append_items_with_persistence_mode(
            &[exec_command_end_item()],
            EventPersistenceMode::Extended,
        )
        .await
        .expect("append extended items");

    let history = store
        .load_history(LoadThreadHistoryParams {
            thread_id,
            include_archived: true,
        })
        .await
        .expect("load history");
    let command_end = history.items.iter().find_map(|item| match item {
        RolloutItem::EventMsg(EventMsg::ExecCommandEnd(event)) => Some(event),
        _ => None,
    });

    assert_eq!(
        command_end.map(|event| {
            (
                event.aggregated_output.as_str(),
                event.stdout.as_str(),
                event.stderr.as_str(),
                event.formatted_output.as_str(),
            )
        }),
        Some(("hello from command\n", "", "", ""))
    );
}

fn create_thread_params(thread_id: ThreadId) -> CreateThreadParams {
    CreateThreadParams {
        thread_id,
        extra_config: None,
        forked_from_id: None,
        parent_thread_id: None,
        source: SessionSource::Cli,
        thread_source: None,
        base_instructions: BaseInstructions::default(),
        dynamic_tools: Vec::new(),
        multi_agent_version: None,
        metadata: ThreadPersistenceMetadata {
            cwd: Some(std::env::current_dir().expect("current dir")),
            model_provider: "test-provider".to_string(),
            memory_mode: ThreadMemoryMode::Disabled,
        },
    }
}

fn exec_command_end_item() -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ExecCommandEnd(ExecCommandEndEvent {
        call_id: "call-1".to_string(),
        process_id: None,
        turn_id: "turn-1".to_string(),
        completed_at_ms: 1,
        command: vec!["echo".to_string(), "hello".to_string()],
        cwd: std::env::current_dir()
            .expect("current dir")
            .try_into()
            .expect("absolute cwd"),
        parsed_cmd: Vec::new(),
        source: ExecCommandSource::UserShell,
        interaction_input: None,
        stdout: "hello from stdout\n".to_string(),
        stderr: "hello from stderr\n".to_string(),
        aggregated_output: "hello from command\n".to_string(),
        exit_code: 0,
        duration: Duration::from_millis(1),
        formatted_output: "formatted output".to_string(),
        status: ExecCommandStatus::Completed,
    }))
}
