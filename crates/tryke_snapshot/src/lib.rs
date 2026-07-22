use std::path::PathBuf;

use tryke_types::{
    SnapshotError, SnapshotMode, SnapshotRunReport, SnapshotTestContext, TestItem, TestResult,
};

enum RunCompletion {
    Complete,
    Interrupted,
}

struct SnapshotRunOptions {
    pub root: PathBuf,
    pub mode: SnapshotMode,
    pub discovered: Vec<TestItem>,
    pub selected: Vec<TestItem>,
}

struct SnapshotRun {}

impl SnapshotRun {
    pub fn start(options: &SnapshotRunOptions) -> Result<Self, SnapshotError> {
        _ = options;
        todo!()
    }

    pub fn context_for(&self, test: &TestItem) -> Option<SnapshotTestContext> {
        _ = test;
        todo!()
    }

    pub fn record(&mut self, result: &TestResult) -> Result<(), SnapshotError> {
        _ = result;
        todo!()
    }

    pub fn finish(self, completion: &RunCompletion) -> Result<SnapshotRunReport, SnapshotError> {
        _ = completion;
        todo!()
    }
}
