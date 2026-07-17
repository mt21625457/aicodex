use std::time::Duration;
use tokio::time::Instant;

pub(super) struct ProgressDeadline {
    idle_timeout: Duration,
    last_progress: Instant,
}

impl ProgressDeadline {
    pub(super) fn new(idle_timeout: Duration) -> Self {
        Self {
            idle_timeout,
            last_progress: Instant::now(),
        }
    }

    pub(super) fn remaining(&self) -> Duration {
        self.idle_timeout
            .saturating_sub(self.last_progress.elapsed())
    }

    pub(super) fn mark_progress(&mut self) {
        self.last_progress = Instant::now();
    }
}
