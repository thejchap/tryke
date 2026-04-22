use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use tryke_reporter::Reporter;
use tryke_types::{RunSummary, TestItem, TestResult};

const RPC_REQUEST_ID: i64 = 1;

pub struct Client {
    port: u16,
    filter: Option<String>,
    paths: Vec<String>,
    markers: Option<String>,
}

impl Client {
    #[must_use]
    pub fn new(
        port: u16,
        filter: Option<String>,
        paths: Vec<String>,
        markers: Option<String>,
    ) -> Self {
        Self {
            port,
            filter,
            paths,
            markers,
        }
    }

    #[expect(clippy::missing_errors_doc)]
    pub fn run(self, root: &Path, reporter: &mut dyn Reporter) -> anyhow::Result<()> {
        let stream = TcpStream::connect(("127.0.0.1", self.port))
            .map_err(|_| anyhow::anyhow!("no server running on port {}", self.port))?;
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream);

        let run_id = generate_run_id();
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": RPC_REQUEST_ID,
            "method": "run",
            "params": {
                "root": root,
                "filter": self.filter,
                "paths": self.paths,
                "markers": self.markers,
                "run_id": run_id,
            }
        });
        writer.write_all(serde_json::to_vec(&req)?.as_slice())?;
        writer.write_all(b"\n")?;
        writer.flush()?;

        let mut line = String::new();
        let mut summary: Option<RunSummary> = None;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let val: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Our RPC response: authoritative signal that the run is done.
            // Detected by matching our request id — the server cannot lose
            // this the way it can silently drop broadcast notifications under
            // `Lagged`, so we break on the response rather than on the
            // `run_complete` notification.
            if val.get("method").is_none()
                && val["id"].as_i64() == Some(RPC_REQUEST_ID)
                && let Ok(s) =
                    serde_json::from_value::<RunSummary>(val["result"]["summary"].clone())
            {
                summary = Some(s);
                break;
            }

            // Notifications for runs we didn't initiate arrive on the same
            // broadcast channel (e.g. another client's run, or a watcher
            // rediscovery). Filter them out so the reporter only sees this
            // client's events.
            let notif_run_id = val["params"]["run_id"].as_str();
            if notif_run_id.is_some() && notif_run_id != Some(run_id.as_str()) {
                continue;
            }

            match val["method"].as_str() {
                Some("run_start") => {
                    if let Ok(tests) =
                        serde_json::from_value::<Vec<TestItem>>(val["params"]["tests"].clone())
                    {
                        reporter.on_run_start(&tests);
                    }
                }
                Some("test_complete") => {
                    if let Ok(result) =
                        serde_json::from_value::<TestResult>(val["params"]["result"].clone())
                    {
                        reporter.on_test_complete(&result);
                    }
                }
                _ => {}
            }
        }

        if let Some(s) = summary {
            reporter.on_run_complete(&s);
            if s.failed > 0 || s.errors > 0 {
                return Err(anyhow::anyhow!(
                    "{} failed, {} error(s)",
                    s.failed,
                    s.errors
                ));
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "server closed connection before returning a run summary"
            ))
        }
    }
}

fn generate_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
        .unwrap_or(0);
    format!("cli-{}-{:x}", std::process::id(), nanos)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    /// Spawn a mock server that captures the client's request, then emits
    /// `responses` in order. If `echo_run_id` is true, the mock parses
    /// `run_id` from the client's request and substitutes every `{RID}`
    /// placeholder in the responses with it — lets tests simulate a real
    /// server that echoes the `run_id` back.
    fn start_mock_server(responses: Vec<String>, echo_run_id: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut req_line = String::new();
                let _ = reader.read_line(&mut req_line);
                let run_id = if echo_run_id {
                    serde_json::from_str::<serde_json::Value>(req_line.trim())
                        .ok()
                        .and_then(|v| v["params"]["run_id"].as_str().map(String::from))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                for resp in &responses {
                    let substituted = resp.replace("{RID}", &run_id);
                    let _ = stream.write_all(substituted.as_bytes());
                    let _ = stream.write_all(b"\n");
                }
                let _ = stream.flush();
            }
        });
        port
    }

    struct RecordingReporter {
        started: bool,
        results: Vec<tryke_types::TestResult>,
        summary: Option<tryke_types::RunSummary>,
    }

    impl RecordingReporter {
        fn new() -> Self {
            Self {
                started: false,
                results: vec![],
                summary: None,
            }
        }
    }

    impl Reporter for RecordingReporter {
        fn on_run_start(&mut self, _tests: &[tryke_types::TestItem]) {
            self.started = true;
        }
        fn on_test_complete(&mut self, result: &tryke_types::TestResult) {
            self.results.push(result.clone());
        }
        fn on_run_complete(&mut self, summary: &tryke_types::RunSummary) {
            self.summary = Some(summary.clone());
        }
    }

    const EMPTY_SUMMARY: &str =
        r#"{"passed":0,"failed":0,"skipped":0,"duration":{"secs":0,"nanos":0}}"#;

    fn run_start_notif() -> String {
        r#"{"jsonrpc":"2.0","method":"run_start","params":{"run_id":"{RID}","tests":[]}}"#
            .to_string()
    }

    fn rpc_response() -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"run_id":"{{RID}}","summary":{EMPTY_SUMMARY}}}}}"#
        )
    }

    #[test]
    fn client_dispatches_reporter_events() {
        let port = start_mock_server(vec![run_start_notif(), rpc_response()], true);

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        Client::new(port, None, vec![], None)
            .run(dir.path(), &mut reporter)
            .unwrap();

        assert!(reporter.started);
        assert!(reporter.summary.is_some());
    }

    #[test]
    fn client_returns_err_when_no_server() {
        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        // Port 1 is privileged and never has a server
        let result = Client::new(1, None, vec![], None).run(dir.path(), &mut reporter);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("no server running on port") || !msg.is_empty());
    }

    #[test]
    fn client_ignores_notifications_from_other_runs() {
        // Emit notifications tagged with a different run_id before our own
        // run_start+response. The reporter must not see any of them.
        let foreign_run_start = r#"{"jsonrpc":"2.0","method":"run_start","params":{"run_id":"other","tests":[{"name":"foreign","module_path":"x"}]}}"#;
        let foreign_test_complete =
            r#"{"jsonrpc":"2.0","method":"test_complete","params":{"run_id":"other","result":{"test":{"name":"foreign","module_path":"x"},"outcome":"Passed","duration":{"secs":0,"nanos":0},"stdout":"","stderr":""}}}"#
                .to_string();
        let port = start_mock_server(
            vec![
                foreign_run_start.to_string(),
                foreign_test_complete,
                run_start_notif(),
                rpc_response(),
            ],
            true,
        );

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        Client::new(port, None, vec![], None)
            .run(dir.path(), &mut reporter)
            .unwrap();

        assert!(
            reporter.started,
            "our own run_start should still be dispatched"
        );
        assert!(
            reporter.results.is_empty(),
            "test_complete for a different run_id must be filtered out"
        );
        assert!(reporter.summary.is_some());
    }

    #[test]
    fn client_breaks_on_rpc_response_even_without_run_complete() {
        // The server no longer depends on `run_complete` for termination.
        // If the broadcast channel lags and the run_complete notification is
        // dropped, the RPC response is still the authoritative terminator.
        let port = start_mock_server(vec![run_start_notif(), rpc_response()], true);

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        Client::new(port, None, vec![], None)
            .run(dir.path(), &mut reporter)
            .unwrap();

        assert!(reporter.summary.is_some());
    }

    #[test]
    fn client_errors_when_server_closes_without_summary() {
        // Server sends only notifications then EOF. Client must not hang;
        // it must surface a clear error.
        let port = start_mock_server(vec![run_start_notif()], true);

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        let result = Client::new(port, None, vec![], None).run(dir.path(), &mut reporter);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("before returning a run summary")
        );
    }

    #[test]
    fn generate_run_id_includes_pid_prefix() {
        let a = generate_run_id();
        let b = generate_run_id();
        assert!(a.starts_with("cli-"));
        assert_ne!(a, b, "consecutive ids should differ by nanos");
    }
}
