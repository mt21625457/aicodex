use codex_utils_path_uri::PathUri;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Weak;
use tokio::sync::Mutex;
use tokio::sync::OwnedMutexGuard;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FileMutationKey {
    environment_id: String,
    path: PathUri,
}

/// Serializes Codex-originated mutations to the same executor path within a turn.
///
/// Callers still need executor-side preconditions because external processes do not
/// participate in these locks.
#[derive(Debug, Default)]
pub(crate) struct FileMutationLocks {
    locks: Mutex<HashMap<FileMutationKey, Weak<Mutex<()>>>>,
}

impl FileMutationLocks {
    pub(crate) async fn lock_paths(
        &self,
        environment_id: &str,
        paths: &[PathUri],
    ) -> Vec<OwnedMutexGuard<()>> {
        let mut keys = paths
            .iter()
            .cloned()
            .map(|path| FileMutationKey {
                environment_id: environment_id.to_string(),
                path,
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| {
            left.environment_id
                .cmp(&right.environment_id)
                .then_with(|| left.path.to_string().cmp(&right.path.to_string()))
        });

        let locks = {
            let mut registry = self.locks.lock().await;
            registry.retain(|_, lock| lock.strong_count() > 0);
            keys.into_iter()
                .map(|key| {
                    if let Some(lock) = registry.get(&key).and_then(Weak::upgrade) {
                        lock
                    } else {
                        let lock = Arc::new(Mutex::new(()));
                        registry.insert(key, Arc::downgrade(&lock));
                        lock
                    }
                })
                .collect::<Vec<_>>()
        };

        let mut guards = Vec::with_capacity(locks.len());
        for lock in locks {
            guards.push(lock.lock_owned().await);
        }
        guards
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn same_environment_and_path_are_serialized() {
        let locks = Arc::new(FileMutationLocks::default());
        let path = PathUri::parse("file:///workspace/file.txt").expect("valid path");
        let first = locks.lock_paths("local", std::slice::from_ref(&path)).await;
        let acquired = Arc::new(AtomicBool::new(false));
        let task = {
            let locks = Arc::clone(&locks);
            let acquired = Arc::clone(&acquired);
            let path = path.clone();
            tokio::spawn(async move {
                let _guard = locks.lock_paths("local", &[path]).await;
                acquired.store(true, Ordering::SeqCst);
            })
        };

        tokio::task::yield_now().await;
        assert!(!acquired.load(Ordering::SeqCst));
        drop(first);
        task.await.expect("lock waiter should finish");
        assert!(acquired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn different_environments_do_not_share_a_lock() {
        let locks = FileMutationLocks::default();
        let path = PathUri::parse("file:///workspace/file.txt").expect("valid path");
        let _local = locks.lock_paths("local", std::slice::from_ref(&path)).await;
        let remote = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            locks.lock_paths("remote", &[path]),
        )
        .await;
        assert!(remote.is_ok());
    }
}
