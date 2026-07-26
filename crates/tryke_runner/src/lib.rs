pub mod pool;
pub mod protocol;
pub mod schedule;
pub mod worker;
pub mod worker_process;

pub use pool::{WorkerPool, WorkerPoolOptions, WorkerRun};
pub use schedule::{DistMode, WorkUnit, partition, partition_with_hooks};
pub use worker_process::WorkerProcess;

/// Return the default number of worker processes for the current machine.
#[must_use]
pub fn default_worker_count() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
}
