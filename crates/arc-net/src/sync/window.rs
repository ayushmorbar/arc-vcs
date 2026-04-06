//! Adaptive batch window growth for round-based sync.
//!
//! This module is pure computation and can be reused by any transport that
//! sends work in bounded batches.

/// Default starting window size.
pub const DEFAULT_INITIAL_WINDOW: usize = 16;
/// Default lower bound for window size.
pub const DEFAULT_MIN_WINDOW: usize = 16;
/// Default threshold after which growth becomes linear-ish.
pub const DEFAULT_LARGE_FLUSH: usize = 16_384;
/// Default additive growth chunk for stateful transports.
pub const DEFAULT_PIPESAFE_FLUSH: usize = 32;

/// Policy controlling adaptive window evolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPolicy {
    /// Minimum allowed window.
    pub min_window: usize,
    /// Threshold for switching from doubling to soft growth in stateless mode.
    pub large_flush: usize,
    /// Additive growth step in stateful mode once `pipesafe_flush` is reached.
    pub pipesafe_flush: usize,
}

impl Default for WindowPolicy {
    fn default() -> Self {
        Self {
            min_window: DEFAULT_MIN_WINDOW,
            large_flush: DEFAULT_LARGE_FLUSH,
            pipesafe_flush: DEFAULT_PIPESAFE_FLUSH,
        }
    }
}

/// Compute the next batch window from the previous value.
///
/// If `current` is `None`, the policy initializes the window to `min_window`.
pub fn next_window_size(transport_is_stateless: bool, current: Option<usize>, policy: WindowPolicy) -> usize {
    let current_size = match current {
        None => return policy.min_window,
        Some(size) => size.max(policy.min_window),
    };

    if transport_is_stateless {
        if current_size < policy.large_flush {
            current_size.saturating_mul(2)
        } else {
            // Approximate +10% growth without float math.
            current_size.saturating_mul(11) / 10
        }
    } else if current_size < policy.pipesafe_flush {
        current_size.saturating_mul(2)
    } else {
        current_size.saturating_add(policy.pipesafe_flush)
    }
}

/// Stateful controller for adaptive batch sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveWindow {
    transport_is_stateless: bool,
    policy: WindowPolicy,
    current: usize,
}

impl AdaptiveWindow {
    /// Create a controller with default policy and default initial size.
    pub fn new(transport_is_stateless: bool) -> Self {
        let policy = WindowPolicy::default();
        Self {
            transport_is_stateless,
            policy,
            current: DEFAULT_INITIAL_WINDOW.max(policy.min_window),
        }
    }

    /// Create a controller with a custom policy and explicit initial size.
    pub fn with_policy(transport_is_stateless: bool, initial: usize, policy: WindowPolicy) -> Self {
        Self {
            transport_is_stateless,
            policy,
            current: initial.max(policy.min_window),
        }
    }

    /// Current window size.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Grow the window after a successful round.
    pub fn on_success(&mut self) {
        self.current = next_window_size(self.transport_is_stateless, Some(self.current), self.policy);
    }

    /// Shrink the window after a failed or backpressured round.
    pub fn on_backpressure(&mut self) {
        self.current = (self.current / 2).max(self.policy.min_window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_to_min_when_missing_current() {
        let policy = WindowPolicy::default();
        assert_eq!(next_window_size(true, None, policy), DEFAULT_MIN_WINDOW);
    }

    #[test]
    fn stateless_doubles_below_large_flush() {
        let policy = WindowPolicy::default();
        assert_eq!(next_window_size(true, Some(16), policy), 32);
    }

    #[test]
    fn stateless_soft_growth_after_large_flush() {
        let policy = WindowPolicy::default();
        assert_eq!(next_window_size(true, Some(20_000), policy), 22_000);
    }

    #[test]
    fn stateful_growth_becomes_additive() {
        let policy = WindowPolicy::default();
        assert_eq!(next_window_size(false, Some(16), policy), 32);
        assert_eq!(next_window_size(false, Some(64), policy), 96);
    }

    #[test]
    fn controller_shrinks_on_backpressure_with_floor() {
        let mut window = AdaptiveWindow::new(true);
        window.on_success();
        window.on_success();
        assert_eq!(window.current(), 64);
        window.on_backpressure();
        assert_eq!(window.current(), 32);
        window.on_backpressure();
        assert_eq!(window.current(), 16);
        window.on_backpressure();
        assert_eq!(window.current(), 16);
    }
}
