use std::{
    io::{BufRead, BufReader, ErrorKind, Write},
    net::TcpStream,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tryke_reporter::Reporter;
use tryke_types::{RunSummary, TestItem, TestResult};

const RPC_REQUEST_ID: i64 = 1;

/// Max time we keep reading notifications after the RPC response arrives,
/// waiting for the matching `run_complete`. The notification writer task and
/// the RPC response writer race for the connection's write lock, so late
/// `test_complete` / `run_complete` messages can arrive after the response.
/// If `run_complete` never shows up (e.g. broadcast channel lagged), the
/// drain simply terminates with the summary from the RPC response.
const POST_RESPONSE_DRAIN: Duration = Duration::from_secs(2);

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

        let summary = read_until_summary(&mut reader, &run_id, reporter)?;
        reporter.on_run_complete(&summary);
        if summary.failed > 0 || summary.errors > 0 {
            return Err(anyhow::anyhow!(
                "{} failed, {} error(s)",
                summary.failed,
                summary.errors
            ));
        }
        Ok(())
    }
}

/// Read newline-delimited messages until we have the authoritative summary
/// from the RPC response, then keep draining notifications for our `run_id`
/// until `run_complete` arrives or `POST_RESPONSE_DRAIN` elapses. The drain
/// phase exists because the server writes the response and the broadcast
/// notifications from separate tasks, so late notifications can land on the
/// socket after the response.
fn read_until_summary(
    reader: &mut BufReader<TcpStream>,
    run_id: &str,
    reporter: &mut dyn Reporter,
) -> anyhow::Result<RunSummary> {
    let mut line = String::new();
    let mut summary: Option<RunSummary> = None;
    let mut draining = false;

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e)
                if draining && matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        }
        let val: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if val.get("method").is_none() && val["id"].as_i64() == Some(RPC_REQUEST_ID) {
            if let Some(err) = val.get("error") {
                let code = err["code"].as_i64().unwrap_or(0);
                let message = err["message"].as_str().unwrap_or("<missing>");
                return Err(anyhow::anyhow!("server error {code}: {message}"));
            }
            let Ok(s) = serde_json::from_value::<RunSummary>(val["result"]["summary"].clone())
            else {
                return Err(anyhow::anyhow!(
                    "server returned a response with no summary for our run"
                ));
            };
            summary = Some(s);
            reader
                .get_ref()
                .set_read_timeout(Some(POST_RESPONSE_DRAIN))
                .ok();
            draining = true;
            continue;
        }

        // Notifications for runs we didn't initiate arrive on the same
        // broadcast channel (e.g. another client's run, or a watcher
        // rediscovery). Filter them so the reporter only sees our events.
        let notif_run_id = val["params"]["run_id"].as_str();
        if notif_run_id.is_some() && notif_run_id != Some(run_id) {
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
            Some("run_complete") if draining => break,
            _ => {}
        }
    }

    summary
        .ok_or_else(|| anyhow::anyhow!("server closed connection before returning a run summary"))
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

    fn run_complete_notif() -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"run_complete","params":{{"run_id":"{{RID}}","summary":{EMPTY_SUMMARY}}}}}"#
        )
    }

    fn rpc_response() -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"run_id":"{{RID}}","summary":{EMPTY_SUMMARY}}}}}"#
        )
    }

    fn rpc_error_response(code: i32, message: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":1,"error":{{"code":{code},"message":"{message}"}}}}"#)
    }

    fn test_complete_for(run_id: &str, name: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"test_complete","params":{{"run_id":"{run_id}","result":{{"test":{{"name":"{name}","module_path":"x","file_path":null,"line_number":null,"display_name":null,"expected_assertions":[]}},"outcome":{{"status":"passed"}},"duration":{{"secs":0,"nanos":0}},"stdout":"","stderr":""}}}}}}"#
        )
    }

    #[test]
    fn client_dispatches_reporter_events() {
        let port = start_mock_server(
            vec![run_start_notif(), rpc_response(), run_complete_notif()],
            true,
        );

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
        let foreign_test_complete = test_complete_for("other", "foreign");
        let port = start_mock_server(
            vec![
                foreign_run_start.to_string(),
                foreign_test_complete,
                run_start_notif(),
                rpc_response(),
                run_complete_notif(),
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
    fn client_dispatches_notifications_that_arrive_after_rpc_response() {
        // The server writes the RPC response and the broadcast notifications
        // from different tasks, so a `test_complete` or `run_complete` can
        // land on the socket after the response. The client must keep
        // draining notifications (until run_complete or a short timeout) so
        // the reporter sees the full event stream.
        let port = start_mock_server(
            vec![
                run_start_notif(),
                rpc_response(),
                test_complete_for("{RID}", "late_test"),
                run_complete_notif(),
            ],
            true,
        );

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        Client::new(port, None, vec![], None)
            .run(dir.path(), &mut reporter)
            .unwrap();

        assert_eq!(
            reporter.results.len(),
            1,
            "late test_complete must reach the reporter via the drain phase"
        );
        assert!(reporter.summary.is_some());
    }

    #[test]
    fn client_breaks_on_rpc_response_even_without_run_complete() {
        // When `run_complete` is lost to broadcast lag the client must still
        // terminate (via the drain timeout) and report the summary from the
        // authoritative RPC response.
        let port = start_mock_server(vec![run_start_notif(), rpc_response()], true);

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        let start = std::time::Instant::now();
        Client::new(port, None, vec![], None)
            .run(dir.path(), &mut reporter)
            .unwrap();
        let elapsed = start.elapsed();

        assert!(reporter.summary.is_some());
        assert!(
            elapsed < POST_RESPONSE_DRAIN + Duration::from_secs(1),
            "drain took too long: {elapsed:?}"
        );
    }

    #[test]
    fn client_surfaces_rpc_error_response() {
        // If the server rejects our request (e.g. missing run_id, though
        // the CLI client always sends one), the error response must turn
        // into an Err instead of hanging the reader.
        let port = start_mock_server(vec![rpc_error_response(-32602, "bad params")], false);

        let dir = tempfile::tempdir().unwrap();
        let mut reporter = RecordingReporter::new();
        let result = Client::new(port, None, vec![], None).run(dir.path(), &mut reporter);
        let err = result.expect_err("expected Err");
        let msg = err.to_string();
        assert!(msg.contains("-32602"), "missing code in: {msg}");
        assert!(msg.contains("bad params"), "missing message in: {msg}");
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
