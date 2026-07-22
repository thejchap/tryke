use std::path::PathBuf;

use tryke_types::{
    SnapshotError, SnapshotMode, SnapshotRunReport, SnapshotSummary, SnapshotTestContext, TestItem,
    TestResult,
};

pub enum RunCompletion {
    Complete,
    Interrupted,
}

pub struct SnapshotRunOptions {
    pub root: PathBuf,
    pub mode: SnapshotMode,
}

pub struct SnapshotRun {
    options: SnapshotRunOptions,
}

impl SnapshotRun {
    /// Load existing snapshot files and prepare state for the test run.
    ///
    /// - Loads and validates relevant per-module .snap files
    /// - indexes their entries
    /// - records which tests were discovered and selected,
    /// - fingerprints the files so later user edits are not overwritten.
    ///
    /// # Errors
    /// Errors if TODO
    pub fn begin(options: SnapshotRunOptions) -> Result<Self, SnapshotError> {
        Ok(Self { options })
    }

    /// Give one test worker the snapshots and mode it needs.
    ///
    /// Builds the small payload sent to the worker running that test.
    /// Contains compare/update mode, the snapshot-file path, and only that test’s expected snapshot entries.
    ///
    /// The worker does not read the file itself.
    #[must_use]
    pub fn context_for(&self, test: &TestItem) -> Option<SnapshotTestContext> {
        _ = test;
        None
    }

    /// Collect the snapshot values produced by a completed test.
    ///
    /// Accepts a completed test result and its snapshot events.
    /// associates each returned value with the module, test case, and assertion key,
    /// while tracking which tests and assertions were actually reached. It does not write anything.
    ///
    /// # Errors
    /// Errors if TODO
    pub fn record(&mut self, result: &TestResult) -> Result<(), SnapshotError> {
        _ = result;
        Ok(())
    }

    /// Finish the snapshot run.
    ///
    /// Merge everything, calculate the summary, detect obsolete snapshots, and write changes in update mode.
    /// Reconciles recorded values with loaded expectations. It classifies snapshots as matched, missing, mismatched, added, updated, obsolete, or removed.
    /// In update mode it checks that files have not changed, then writes each affected module once atomically. Finally, it returns the summary and changed paths.
    ///
    /// # Errors
    /// Errors if TODO
    pub fn finish(self, completion: &RunCompletion) -> Result<SnapshotRunReport, SnapshotError> {
        if matches!(self.options.mode, SnapshotMode::Compare) {
            return Ok(SnapshotRunReport {
                summary: SnapshotSummary {
                    matched: 0,
                    added: 0,
                    mismatched: 0,
                    missing: 0,
                    obsolete: 0,
                    updated: 0,
                    removed: 0,
                },
                changed_files: vec![],
            });
        }
        _ = completion;
        todo!()
    }
}
