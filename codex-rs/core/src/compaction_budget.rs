//! 压缩的全局软/硬预算与单次进展截止。策略不拥有网络或历史，重试不能重置全局时钟。
use std::time::Duration;
use tokio::time::Instant;

pub(super) const IDLE_LIMIT: Duration = Duration::from_secs(120);
pub(super) const SOFT_LIMIT: Duration = Duration::from_secs(300);
pub(super) const HARD_LIMIT: Duration = Duration::from_secs(600);

/// 软截止只为已有有效进展的请求提供宽限；新请求必须预留完整无进展窗口。
pub(super) fn deadline(started: Instant, last_progress: Instant, progressing: bool) -> Instant {
    let total = if progressing { HARD_LIMIT } else { SOFT_LIMIT };
    (started + total).min(last_progress + IDLE_LIMIT)
}

pub(super) fn admits_retry(started: Instant, backoff: Duration) -> bool {
    SOFT_LIMIT.saturating_sub(started.elapsed()) >= backoff.saturating_add(IDLE_LIMIT)
}
