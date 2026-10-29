//! 数据库与正文故意保存不同模型；验证数据库模式不被正文覆盖，普通模式仍补充。
use super::*;
use crate::ThreadStore;
use crate::local::test_support::{test_config, write_archived_session_file, write_session_file};
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::SessionSource;
use std::io::Write;
use tempfile::TempDir;
use uuid::Uuid;

async fn fixture(archived: bool) -> (TempDir, LocalThreadStore) {
    let home = TempDir::new().expect("temporary home");
    let config = test_config(home.path());
    let state = codex_state::StateRuntime::init(config.sqlite.clone(), "test-provider".into())
        .await
        .expect("state database");
    state
        .mark_backfill_complete(None)
        .await
        .expect("backfill complete");
    for index in 0..2 {
        let uuid = Uuid::from_u128(9200 + index);
        let path = if archived {
            write_archived_session_file(home.path(), "2025-01-03T12-00-00", uuid)
        } else {
            write_session_file(home.path(), "2025-01-03T12-00-00", uuid)
        }
        .expect("rollout fixture");
        let context = serde_json::json!({
            "timestamp": "2025-01-03T12:00:00Z", "type": "turn_context",
            "payload": {"cwd": home.path(), "approval_policy": "never",
                "sandbox_policy": {"type": "read-only"}, "model": "rollout-model", "effort": "high"}
        });
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open fixture");
        writeln!(file, "{context}").expect("write context");
        let id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
        let builder =
            codex_state::ThreadMetadataBuilder::new(id, path, Utc::now(), SessionSource::Cli);
        let mut metadata = builder.build("test-provider");
        metadata.cwd = home.path().to_owned();
        metadata.first_user_message = Some("fixture user message".into());
        metadata.preview = metadata.first_user_message.clone();
        metadata.model = Some("database-model".into());
        metadata.reasoning_effort = Some(ReasoningEffort::Low);
        metadata.archived_at = archived.then(Utc::now);
        state
            .upsert_thread(&metadata)
            .await
            .expect("insert indexed thread");
    }
    (home, LocalThreadStore::new(config, Some(state)))
}

fn params(archived: bool, db_only: bool) -> ListThreadsParams {
    ListThreadsParams {
        page_size: 1,
        cursor: None,
        sort_key: ThreadSortKey::CreatedAt,
        sort_direction: SortDirection::Desc,
        allowed_sources: Vec::new(),
        model_providers: None,
        cwd_filters: None,
        section: None,
        project_id: None,
        archived,
        search_term: None,
        relation_filter: None,
        use_state_db_only: db_only,
    }
}

#[tokio::test]
async fn db_only_lists_preserve_database_metadata_across_active_and_archived_pages() {
    for archived in [false, true] {
        let (_home, store) = fixture(archived).await;
        let mut request = params(archived, true);
        let mut ids = std::collections::HashSet::new();
        for page_number in 0..2 {
            let page = store
                .list_threads(request.clone())
                .await
                .expect("database page");
            assert_eq!(page.items.len(), 1);
            let thread = &page.items[0];
            assert_eq!(thread.model.as_deref(), Some("database-model"));
            assert_eq!(thread.reasoning_effort, Some(ReasoningEffort::Low));
            assert!(ids.insert(thread.thread_id));
            assert_eq!(page.next_cursor.is_some(), page_number == 0);
            request.cursor = page.next_cursor;
        }
    }
}

#[tokio::test]
async fn ordinary_lists_still_enrich_model_from_rollout() {
    for archived in [false, true] {
        let (_home, store) = fixture(archived).await;
        let page = store
            .list_threads(params(archived, false))
            .await
            .expect("ordinary page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].model.as_deref(), Some("rollout-model"));
        assert_eq!(page.items[0].reasoning_effort, Some(ReasoningEffort::High));
    }
}
