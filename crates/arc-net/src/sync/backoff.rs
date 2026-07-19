use std::time::Duration;

/// Quadratic backoff iterator with optional transform (for jitter in production).
pub struct QuadraticBackoff<Transform = fn(usize) -> usize> {
    multiplier: usize,
    max_multiplier: usize,
    exponent: usize,
    transform: Transform,
}

impl Default for QuadraticBackoff<fn(usize) -> usize> {
    fn default() -> Self {
        Self::new()
    }
}

impl QuadraticBackoff<fn(usize) -> usize> {
    /// Build a backoff iterator without jitter.
    pub fn new() -> Self {
        Self { multiplier: 1, max_multiplier: 1000, exponent: 1, transform: std::convert::identity }
    }
}

impl<Transform> QuadraticBackoff<Transform>
where
    Transform: Fn(usize) -> usize,
{
    /// Build a backoff iterator with an explicit transform.
    pub fn with_transform(transform: Transform) -> Self {
        Self { multiplier: 1, max_multiplier: 1000, exponent: 1, transform }
    }
}

impl<Transform> Iterator for QuadraticBackoff<Transform>
where
    Transform: Fn(usize) -> usize,
{
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        let wait = Duration::from_millis((self.transform)(self.multiplier) as u64);
        self.multiplier += 2 * self.exponent + 1;
        if self.multiplier > self.max_multiplier {
            self.multiplier = self.max_multiplier;
        } else {
            self.exponent += 1;
        }
        Some(wait)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::QuadraticBackoff;

    #[test]
    fn default_sequence_is_quadratic() {
        let waits: Vec<Duration> = QuadraticBackoff::default().take(5).collect();
        assert_eq!(
            waits,
            vec![
                Duration::from_millis(1),
                Duration::from_millis(4),
                Duration::from_millis(9),
                Duration::from_millis(16),
                Duration::from_millis(25)
            ]
        );
    }

    #[test]
    fn transform_is_applied() {
        let waits: Vec<Duration> = QuadraticBackoff::with_transform(|v| v * 2).take(3).collect();
        assert_eq!(
            waits,
            vec![Duration::from_millis(2), Duration::from_millis(8), Duration::from_millis(18)]
        );
    }
}
