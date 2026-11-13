//! A tiny HTTP/1.1 file server on `127.0.0.1` for tests, on `std::net` only.
//!
//! It serves `GET` and `HEAD` from an in-memory map, honors
//! `Range: bytes=N-`, and can misbehave per path (ignore `Range`, drop the
//! connection partway, redirect) to exercise resume and error handling.
//! Shared by unit tests (`#[path]`-included) and integration tests.

#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

#[derive(Debug, Clone, Default)]
pub struct File {
    pub data: Vec<u8>,
    /// Answer `Range` requests with the whole file (200).
    pub ignore_range: bool,
    /// Announce the full length but close after this many body bytes, once;
    /// later requests get the whole file.
    pub cut_after: Option<usize>,
    /// Answer with a 302 to this path instead.
    pub redirect: Option<String>,
}

impl File {
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self {
            data: data.into(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub range: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Default)]
struct State {
    files: BTreeMap<String, File>,
    log: Vec<Request>,
}

pub struct Server {
    pub base: String,
    state: Arc<Mutex<State>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                thread::spawn(move || {
                    let _ = serve(stream, &state);
                });
            }
        });
        Self { base, state }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn put(&self, path: &str, file: File) {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(path.to_owned(), file);
    }

    pub fn put_bytes(&self, path: &str, data: impl Into<Vec<u8>>) {
        self.put(path, File::new(data));
    }

    pub fn remove(&self, path: &str) {
        self.state.lock().unwrap().files.remove(path);
    }

    pub fn log(&self) -> Vec<Request> {
        self.state.lock().unwrap().log.clone()
    }

    pub fn clear_log(&self) {
        self.state.lock().unwrap().log.clear();
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_owned(),
        parts.next().unwrap_or("").to_owned(),
    );
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let request = Request {
        method: method.clone(),
        path: path.clone(),
        range: headers.get("range").cloned(),
        user_agent: headers.get("user-agent").cloned(),
    };
    let file = {
        let mut state = state.lock().unwrap();
        state.log.push(request.clone());
        let file = state.files.get(&path).cloned();
        if let Some(stored) = state.files.get_mut(&path) {
            stored.cut_after = None;
        }
        file
    };
    let mut out = stream;
    let Some(file) = file else {
        return write!(
            out,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    };
    if let Some(target) = &file.redirect {
        return write!(
            out,
            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }
    let len = file.data.len();
    let start = request
        .range
        .as_deref()
        .filter(|_| !file.ignore_range)
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok());
    let (status, body, extra) = match start {
        Some(start) if start >= len => {
            write!(
                out,
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )?;
            return Ok(());
        }
        Some(start) => (
            "206 Partial Content",
            &file.data[start..],
            format!("Content-Range: bytes {start}-{}/{len}\r\n", len - 1),
        ),
        None => ("200 OK", &file.data[..], String::new()),
    };
    write!(
        out,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Last-Modified: Thu, 12 Nov 2026 10:00:00 GMT\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if method == "HEAD" {
        return Ok(());
    }
    let body = match file.cut_after {
        Some(cut) => &body[..cut.min(body.len())],
        None => body,
    };
    out.write_all(body)?;
    out.flush()
}
