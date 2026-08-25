//! Circuit-breaker for identical mid-turn assistant follow-ups.
//!
//! MiniMax-M3 can emit the same long commentary after every tool result and
//! keep requesting another tool, sometimes with a short trailing status line
//! that changes. The turn loop continues whenever `needs_follow_up` is true;
//! this detector stops that automatic continuation once the long follow-up
//! body repeats.

/// Minimum normalized length before a follow-up is considered a loop candidate.
/// Short status pings such as "继续构建：" must not trip the breaker.
pub(crate) const MIN_REPEATED_FOLLOW_UP_CHARS: usize = 80;

/// Trailing status text that may change while the long body stays the same.
pub(crate) const MAX_FOLLOW_UP_SUFFIX_CHARS: usize = 40;

/// Stop after this many consecutive matching follow-ups (including the first).
pub(crate) const REPEATED_FOLLOW_UP_LIMIT: usize = 2;

#[derive(Debug, Default)]
pub(crate) struct RepeatedFollowUpDetector {
    last_normalized: Option<String>,
    consecutive: usize,
}

impl RepeatedFollowUpDetector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Records one follow-up sampling that still needs another model request.
    ///
    /// Short or missing text is ignored so a trailing status line cannot wipe a
    /// previous long commentary. Returns `true` when matching long follow-up
    /// text has now appeared [`REPEATED_FOLLOW_UP_LIMIT`] times in a row.
    pub(crate) fn trip_on_repeat(&mut self, follow_up_text: Option<&str>) -> bool {
        let Some(normalized) = normalize_follow_up_text(follow_up_text) else {
            return false;
        };

        if self
            .last_normalized
            .as_deref()
            .is_some_and(|previous| is_same_follow_up(previous, &normalized))
        {
            self.consecutive = self.consecutive.saturating_add(1);
        } else {
            self.last_normalized = Some(normalized);
            self.consecutive = 1;
        }

        self.consecutive >= REPEATED_FOLLOW_UP_LIMIT
    }
}

/// Keeps the longest assistant text from one sampling so a short trailing
/// status line cannot hide the repeated commentary body.
pub(crate) fn retain_longest_follow_up(current: &mut Option<String>, candidate: &str) {
    let candidate_len = candidate.chars().count();
    if current
        .as_ref()
        .is_none_or(|existing| candidate_len > existing.chars().count())
    {
        *current = Some(candidate.to_string());
    }
}

fn normalize_follow_up_text(text: Option<&str>) -> Option<String> {
    let Some(text) = text else {
        return None;
    };
    let mut normalized = String::new();
    for word in text.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    if normalized.chars().count() < MIN_REPEATED_FOLLOW_UP_CHARS {
        return None;
    }
    Some(normalized)
}

fn is_same_follow_up(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }

    let prefix_len = left
        .chars()
        .zip(right.chars())
        .take_while(|(left_ch, right_ch)| left_ch == right_ch)
        .count();
    if prefix_len < MIN_REPEATED_FOLLOW_UP_CHARS {
        return false;
    }

    let left_suffix = left.chars().count().saturating_sub(prefix_len);
    let right_suffix = right.chars().count().saturating_sub(prefix_len);
    left_suffix <= MAX_FOLLOW_UP_SUFFIX_CHARS && right_suffix <= MAX_FOLLOW_UP_SUFFIX_CHARS
}

#[cfg(test)]
#[path = "repeated_follow_up_tests.rs"]
mod tests;
