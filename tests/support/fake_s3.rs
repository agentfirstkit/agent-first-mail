#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "fake S3 fixture assertions"
)]
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

#[derive(Clone)]
pub struct Object {
    pub raw: Vec<u8>,
    pub size_bytes: u64,
    pub recipient: String,
}
#[derive(Default)]
pub struct State {
    pub quota_managed: bool,
    pub control_failures: usize,
    pub objects: BTreeMap<String, Object>,
    pub requests: Vec<(String, String)>,
    pub delete_failures: usize,
    pub copy_failure: bool,
    pub omit_list_size: bool,
    pub pause_delete: bool,
}
pub struct FakeS3 {
    pub endpoint: String,
    pub state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl FakeS3 {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &worker_state),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(error) => panic!("accept fake S3: {error}"),
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
    pub fn put(&self, key: &str, raw: &[u8], recipient: &str) {
        self.state.lock().unwrap().objects.insert(
            key.into(),
            Object {
                raw: raw.to_vec(),
                size_bytes: raw.len() as u64,
                recipient: recipient.into(),
            },
        );
    }
}
impl Drop for FakeS3 {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "fake S3 server panicked");
            }
        }
    }
}
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            output.push(byte);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).unwrap()
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn serve(mut stream: TcpStream, shared: &Mutex<State>) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if !matches!(reader.read_line(&mut first), Ok(count) if count > 0) {
        return;
    }
    let fields: Vec<_> = first.split_whitespace().collect();
    let method = fields[0];
    let target = fields[1];
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let key = decode(path.strip_prefix("/spool/").unwrap_or(""));
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let mut request_body = Vec::new();
    if let Some(size) = headers
        .get("content-length")
        .and_then(|size| size.parse::<usize>().ok())
    {
        request_body.resize(size, 0);
        reader.read_exact(&mut request_body).unwrap();
    }
    let mut state = shared.lock().unwrap();
    state.requests.push((method.into(), key.clone()));
    let mut status = 200;
    let mut extra = String::new();
    let body: Vec<u8>;
    if method == "POST" && path == "/v1/ack" {
        assert_eq!(
            headers.get("authorization"),
            Some(&format!("Bearer {}", "q".repeat(32)))
        );
        if state.control_failures > 0 {
            state.control_failures -= 1;
            status = 503;
            body = b"{}".to_vec();
        } else {
            let value: serde_json::Value = serde_json::from_slice(&request_body).unwrap();
            let source = value["key"].as_str().unwrap();
            if let Some(target) = value["archive_key"].as_str() {
                if let Some(object) = state.objects.get(source).cloned() {
                    state.objects.insert(target.into(), object);
                } else {
                    assert!(state.objects.contains_key(target));
                }
            }
            state.objects.remove(source);
            body = br#"{"kind":"result","result":{"acknowledged":true},"trace":{}}"#.to_vec();
        }
    } else if method == "GET" && query.contains("list-type=2") {
        let query: BTreeMap<_, _> = query
            .split('&')
            .filter_map(|item| item.split_once('='))
            .map(|(name, value)| (name, decode(value)))
            .collect();
        let prefix = query.get("prefix").map(String::as_str).unwrap_or("");
        let offset = query
            .get("continuation-token")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let max = query
            .get("max-keys")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1000)
            .min(1);
        let objects: Vec<_> = state
            .objects
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .collect();
        let mut response =
            String::from("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        for (key, object) in objects.iter().skip(offset).take(max) {
            response.push_str(&format!("<Contents><Key>{}</Key><ETag>etag</ETag><LastModified>2026-10-21T00:00:00Z</LastModified>", xml(key)));
            if !state.omit_list_size {
                response.push_str(&format!("<Size>{}</Size>", object.size_bytes));
            }
            response.push_str("</Contents>");
        }
        if objects.len() > offset + max {
            response.push_str(&format!(
                "<NextContinuationToken>{}</NextContinuationToken>",
                offset + max
            ));
        }
        response.push_str("</ListBucketResult>");
        body = response.into_bytes();
    } else if method == "GET" {
        if let Some(object) = state.objects.get(&key) {
            body = object.raw.clone();
            extra = format!(
                "x-amz-meta-envelope_from: sender@example.net\r\nx-amz-meta-envelope_to: {}\r\nx-amz-meta-received_rfc3339: 2026-10-21T00:00:00Z\r\n",
                object.recipient
            );
            if state.quota_managed {
                extra.push_str("x-amz-meta-quota_managed: true\r\n");
            }
        } else {
            status = 404;
            body = vec![];
        }
    } else if method == "DELETE" {
        while state.pause_delete {
            drop(state);
            thread::sleep(std::time::Duration::from_millis(5));
            state = shared.lock().unwrap();
        }
        if state.delete_failures > 0 {
            state.delete_failures -= 1;
            status = 503;
        } else {
            state.objects.remove(&key);
            status = 204;
        }
        body = vec![];
    } else if method == "PUT" {
        assert!(
            query.contains("x-amz-copy-source")
                || query
                    .to_ascii_lowercase()
                    .contains("x-amz-signedheaders=host%3bx-amz-copy-source"),
            "copy source must be signed: {query}"
        );
        let source = decode(headers.get("x-amz-copy-source").unwrap())
            .trim_start_matches("/spool/")
            .to_string();
        if state.copy_failure {
            body = b"<Error><Code>InternalError</Code></Error>".to_vec();
        } else {
            let object = state.objects.get(&source).unwrap().clone();
            state.objects.insert(key, object);
            body = b"<CopyObjectResult><ETag>etag</ETag></CopyObjectResult>".to_vec();
        }
    } else {
        status = 400;
        body = vec![];
    }
    drop(state);
    if write!(
        stream,
        "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    )
    .is_ok()
    {
        let _ = stream.write_all(&body);
    }
}
