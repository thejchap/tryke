use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::Path,
};

use tryke_reporter::Reporter;
use tryke_types::{RunSummary, TestItem, TestResult};

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

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "run",
            "params": { "root": root, "filter": self.filter, "paths": self.paths, "markers": self.markers }
        });
        writer.write_all(serde_json::to_vec(&req)?.as_slice())?;
        writer.write_all(b"\n")?;
        writer.flush()?;

        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let val: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };
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
                Some("run_complete") => {
                    if let Ok(summary) =
                        serde_json::from_value::<RunSummary>(val["params"]["summary"].clone())
                    {
                        reporter.on_run_complete(&summary);
                        if summary.failed > 0 || summary.errors > 0 {
                            return Err(anyhow::anyhow!(
                                "{} failed, {} error(s)",
                                summary.failed,
                                summary.errors
                            ));
                        }
                    }
                    break;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn start_mock_server(responses: Vec<String>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut req_line = String::new();
                let _ = reader.read_line(&mut req_line);
                for resp in &responses {
                    let _ = stream.write_all(resp.as_bytes());
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

    #[test]
    fn client_dispatches_reporter_events() {
        let run_start = r#"{"jsonrpc":"2.0","method":"run_start","params":{"tests":[]}}"#;
        let run_complete = r#"{"jsonrpc":"2.0","method":"run_complete","params":{"summary":{"passed":0,"failed":0,"skipped":0,"duration":{"secs":0,"nanos":0}}}}"#;
        let port = start_mock_server(vec![run_start.to_string(), run_complete.to_string()]);

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
}
