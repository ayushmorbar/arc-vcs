//! Native-only scoped worker orchestration utilities.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, mpsc};

    /// Process all `items` in scoped worker threads and return results in input order.
    pub fn run_scoped_map<T, R, F>(items: Vec<T>, threads: usize, worker: F) -> Vec<R>
    where
        T: Send,
        R: Send,
        F: Fn(T) -> R + Send + Sync,
    {
        if items.is_empty() {
            return Vec::new();
        }

        let thread_count = threads.max(1).min(items.len());
        let worker = Arc::new(worker);
        let mut buckets = Vec::with_capacity(thread_count);
        for _ in 0..thread_count {
            buckets.push(Vec::new());
        }

        for (index, item) in items.into_iter().enumerate() {
            buckets[index % thread_count].push((index, item));
        }

        let (tx, rx) = mpsc::channel::<Vec<(usize, R)>>();
        std::thread::scope(|scope| {
            for bucket in buckets {
                let tx = tx.clone();
                let worker = Arc::clone(&worker);
                scope.spawn(move || {
                    let mut partial = Vec::with_capacity(bucket.len());
                    for (index, item) in bucket {
                        partial.push((index, worker(item)));
                    }
                    tx.send(partial).expect("scoped worker should send partial result");
                });
            }
        });
        drop(tx);

        let mut flattened = Vec::new();
        for partial in rx {
            flattened.extend(partial);
        }
        flattened.sort_by_key(|(index, _)| *index);
        flattened.into_iter().map(|(_, value)| value).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::run_scoped_map;

        #[test]
        fn scoped_map_preserves_input_order() {
            let values = vec![1_u32, 2, 3, 4, 5, 6];
            let squared = run_scoped_map(values, 3, |value| value * value);
            assert_eq!(squared, vec![1, 4, 9, 16, 25, 36]);
        }

        #[test]
        fn scoped_map_handles_empty_input() {
            let values: Vec<u32> = Vec::new();
            let out = run_scoped_map(values, 4, |value| value + 1);
            assert!(out.is_empty());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::run_scoped_map;
