#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "HTTP queue fixture assertions"
)]
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
#[derive(Default)]
pub struct State {
    pub messages: Vec<Value>,
    pub requests: Vec<(String, Value)>,
    pub fail_pull: bool,
    pub ack_failures: usize,
}
pub struct FakeQueue {
    pub endpoint: String,
    pub state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl FakeQueue {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let worker_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("queue accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if !matches!(reader.read_line(&mut line), Ok(count) if count > 0) {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                let mut size = 0;
                let mut token = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            size = value.trim().parse().unwrap();
                        }
                        if name.eq_ignore_ascii_case("authorization") {
                            token = value.trim().to_string();
                        }
                    }
                }
                assert_eq!(token, "Bearer queue-test-token");
                let mut bytes = vec![0; size];
                reader.read_exact(&mut bytes).unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let mut state = shared.lock().unwrap();
                state.requests.push((path.clone(), body.clone()));
                let (status, result) = if path.ends_with("/pull") {
                    if state.fail_pull {
                        (503, json!({"success":false}))
                    } else {
                        (
                            200,
                            json!({"success":true,"result":{"messages":state.messages}}),
                        )
                    }
                } else if path.ends_with("/ack") {
                    if state.ack_failures > 0 {
                        state.ack_failures -= 1;
                        (503, json!({"success":false}))
                    } else {
                        state.messages.retain(|message| {
                            !body["acks"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|ack| ack["lease_id"] == message["lease_id"])
                        });
                        (200, json!({"success":true,"result":{}}))
                    }
                } else {
                    panic!("unexpected queue path {path}");
                };
                drop(state);
                let bytes = serde_json::to_vec(&result).unwrap();
                if write!(
                    stream,
                    "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .is_ok()
                {
                    let _ = stream.write_all(&bytes);
                }
            }
        });
        Self {
            endpoint,
            state,
            stop,
            thread: Some(thread),
        }
    }
    pub fn notify(&self, key: &str, raw: &[u8], lease: &str) {
        use base64::Engine as _;
        let event = json!({"account":"a".repeat(32),"action":"PutObject","bucket":"spool","object":{"key":key,"size":raw.len(),"eTag":"etag"}});
        let body =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap());
        self.state
            .lock()
            .unwrap()
            .messages
            .push(json!({"body":body,"lease_id":lease}));
    }
}
impl Drop for FakeQueue {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "queue server panicked");
            }
        }
    }
}
