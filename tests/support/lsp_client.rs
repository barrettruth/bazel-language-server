use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(15);

pub struct Timed {
    pub value: Value,
    pub elapsed: Duration,
}

pub struct Client {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Value>,
    backlog: VecDeque<Value>,
    stderr: Option<JoinHandle<String>>,
    next_id: i64,
    root: PathBuf,
}

impl Client {
    pub fn spawn(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bazel-language-server"))
            .arg("server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn language server");
        let input = child.stdin.take().expect("server stdin");
        let output = child.stdout.take().expect("server stdout");
        let errors = child.stderr.take().expect("server stderr");
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || read_messages(output, &tx));
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            BufReader::new(errors)
                .read_to_string(&mut text)
                .expect("read server stderr");
            text
        });
        Self {
            child,
            input,
            messages,
            backlog: VecDeque::new(),
            stderr: Some(stderr),
            next_id: 0,
            root: root.to_path_buf(),
        }
    }

    pub fn initialize(&mut self, options: &Value) -> Timed {
        let root = self.uri("");
        let response = self.request(
            "initialize",
            &json!({
                "processId": null,
                "rootUri": root,
                "workspaceFolders": [{"uri": root, "name": "fixture"}],
                "initializationOptions": options,
                "capabilities": {
                    "textDocument": {
                        "definition": {"linkSupport": true},
                        "publishDiagnostics": {},
                        "synchronization": {"didSave": true}
                    },
                    "workspace": {"symbol": {}, "workspaceFolders": true},
                    "window": {"workDoneProgress": true}
                }
            }),
        );
        self.notify("initialized", &json!({}));
        response
    }

    pub fn open(&mut self, relative: &str, version: i32, text: &str) {
        self.notify(
            "textDocument/didOpen",
            &json!({
                "textDocument": {
                    "uri": self.uri(relative),
                    "languageId": "starlark",
                    "version": version,
                    "text": text
                }
            }),
        );
    }

    pub fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn request(&mut self, method: &str, params: &Value) -> Timed {
        let started = Instant::now();
        let id = self.send_request(method, params);
        let value = self.wait_response(id);
        Timed {
            value,
            elapsed: started.elapsed(),
        }
    }

    pub fn send_request(&mut self, method: &str, params: &Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        id
    }

    pub fn wait_response(&mut self, id: i64) -> Value {
        self.wait_for(|message| message.get("id") == Some(&json!(id)))
    }

    #[allow(dead_code)]
    pub fn wait_notification(&mut self, method: &str) -> Value {
        self.wait_notification_matching(method, |_| true)
    }

    pub fn wait_notification_matching(
        &mut self,
        method: &str,
        matches: impl Fn(&Value) -> bool,
    ) -> Value {
        self.wait_for(|message| {
            message.get("method").and_then(Value::as_str) == Some(method) && matches(message)
        })
    }

    pub fn uri(&self, relative: &str) -> String {
        let path = self.root.join(relative);
        let mut uri = String::from("file://");
        for byte in path.as_os_str().as_encoded_bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
                uri.push(char::from(*byte));
            } else {
                use std::fmt::Write as _;
                write!(uri, "%{byte:02X}").expect("write URI escape");
            }
        }
        uri
    }

    pub fn shutdown(mut self) -> String {
        let response = self.request("shutdown", &Value::Null);
        assert!(response.value.get("error").is_none(), "{response:?}");
        self.notify("exit", &Value::Null);
        let status = self.child.wait().expect("wait for language server");
        assert!(status.success(), "language server exited with {status}");
        self.stderr.take().expect("stderr reader").join().unwrap()
    }

    #[allow(dead_code)]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("encode message");
        write!(self.input, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
        self.input.write_all(&body).expect("write body");
        self.input.flush().expect("flush message");
    }

    fn wait_for(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(index) = self.backlog.iter().position(&wanted) {
                return self.backlog.remove(index).expect("queued message");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(remaining)
                .unwrap_or_else(|err| panic!("language server response: {err}"));
            if wanted(&message) {
                return message;
            }
            self.backlog.push_back(message);
        }
    }
}

impl std::fmt::Debug for Timed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timed")
            .field("value", &self.value)
            .field("elapsed", &self.elapsed)
            .finish()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            drop(self.child.kill());
            drop(self.child.wait());
        }
        if let Some(stderr) = self.stderr.take() {
            drop(stderr.join());
        }
    }
}

fn read_messages(output: impl Read, tx: &mpsc::Sender<Value>) {
    let mut output = BufReader::new(output);
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            let Ok(read) = output.read_line(&mut line) else {
                return;
            };
            if read == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else { return };
        let mut body = vec![0; length];
        if output.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(message) = serde_json::from_slice(&body) else {
            return;
        };
        if tx.send(message).is_err() {
            return;
        }
    }
}
