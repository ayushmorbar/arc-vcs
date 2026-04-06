//! Task registry harness for engine-side health checks and benchmarks.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::anyhow;

/// Result of one executed task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRunResult {
    /// Task identifier.
    pub id: String,
    /// Task wall-clock duration.
    pub duration: Duration,
}

/// A unit of work that can be executed by the harness.
pub trait EngineTask: Send + Sync {
    /// Stable short identifier used by the registry.
    fn id(&self) -> &'static str;

    /// Execute task logic.
    fn run(&self) -> anyhow::Result<()>;
}

/// In-process task registry and execution harness.
#[derive(Default)]
pub struct TaskRegistry {
    tasks: BTreeMap<&'static str, Box<dyn EngineTask>>,
}

impl TaskRegistry {
    /// Create an empty task registry.
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
        }
    }

    /// Register a new task by id.
    pub fn register(&mut self, task: Box<dyn EngineTask>) -> anyhow::Result<()> {
        let id = task.id();
        if self.tasks.contains_key(id) {
            return Err(anyhow!("task '{id}' is already registered"));
        }
        self.tasks.insert(id, task);
        Ok(())
    }

    /// Return all registered task ids in deterministic order.
    pub fn ids(&self) -> Vec<&'static str> {
        self.tasks.keys().copied().collect()
    }

    /// Run one task by id.
    pub fn run_one(&self, id: &str) -> anyhow::Result<TaskRunResult> {
        let task = self
            .tasks
            .get(id)
            .ok_or_else(|| anyhow!("task '{id}' not found"))?;

        let start = Instant::now();
        task.run()?;

        Ok(TaskRunResult {
            id: id.to_string(),
            duration: start.elapsed(),
        })
    }

    /// Run all registered tasks in deterministic order.
    pub fn run_all(&self) -> anyhow::Result<Vec<TaskRunResult>> {
        let mut results = Vec::with_capacity(self.tasks.len());
        for id in self.ids() {
            results.push(self.run_one(id)?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopTask;

    impl EngineTask for NoopTask {
        fn id(&self) -> &'static str {
            "noop"
        }

        fn run(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct FailingTask;

    impl EngineTask for FailingTask {
        fn id(&self) -> &'static str {
            "fail"
        }

        fn run(&self) -> anyhow::Result<()> {
            Err(anyhow!("boom"))
        }
    }

    #[test]
    fn registration_rejects_duplicates() {
        let mut registry = TaskRegistry::new();
        registry
            .register(Box::new(NoopTask))
            .expect("first registration should succeed");
        let duplicate = registry.register(Box::new(NoopTask));
        assert!(duplicate.is_err());
    }

    #[test]
    fn run_one_returns_duration() {
        let mut registry = TaskRegistry::new();
        registry
            .register(Box::new(NoopTask))
            .expect("registration should succeed");

        let result = registry
            .run_one("noop")
            .expect("task run should succeed");
        assert_eq!(result.id, "noop");
        assert!(result.duration >= Duration::from_nanos(0));
    }

    #[test]
    fn run_all_is_deterministic_and_propagates_error() {
        let mut registry = TaskRegistry::new();
        registry
            .register(Box::new(NoopTask))
            .expect("registration should succeed");
        registry
            .register(Box::new(FailingTask))
            .expect("registration should succeed");

        let err = registry.run_all().expect_err("failing task should bubble up");
        assert!(err.to_string().contains("boom"));
    }
}
