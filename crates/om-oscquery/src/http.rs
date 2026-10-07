// SPDX-License-Identifier: Apache-2.0
//! A minimal, bounded HTTP/1.1 GET server for the OSCQuery tree.
//!
//! Every request is read with a size cap and a deadline, at most
//! [`MAX_CONNECTIONS`] are served at once (extra connections are closed
//! immediately), and every response closes the connection, so a slow or
//! hostile client can neither exhaust memory or threads nor hold the
//! service for long.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Largest request head accepted (request line plus headers).
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
/// Connections served concurrently.
pub const MAX_CONNECTIONS: usize = 16;
/// Time a client has to send its request.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(2);

/// Answers a request target (`/path?QUERY`) with a status and JSON body
/// (empty when `None`).
pub type Handler = dyn Fn(&str) -> (u16, Option<String>) + Send + Sync;

/// Accepts connections until `stop` is set.
pub fn serve(listener: &TcpListener, handler: &Arc<Handler>, stop: &AtomicBool) {
    let active = Arc::new(AtomicUsize::new(0));
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        if active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::AcqRel);
            drop(stream);
            continue;
        }
        // Released when the connection ends, or if the thread never starts
        // (the closure is then dropped unrun).
        let slot = Slot(Arc::clone(&active));
        let handler = Arc::clone(handler);
        let _ = std::thread::Builder::new()
            .name("om-oscquery-conn".into())
            .spawn(move || {
                let _slot = slot;
                handle(stream, &*handler);
            });
    }
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reads the request head: `Ok(Some(head))`, `Ok(None)` when too large.
fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left))?;
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(Some(buf));
        }
        if buf.len() > MAX_REQUEST_BYTES {
            return Ok(None);
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    }
}

fn handle(mut stream: TcpStream, handler: &Handler) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(REQUEST_DEADLINE));
    let (status, body) = match read_head(&mut stream) {
        Err(_) => return,
        Ok(None) => (431, None),
        Ok(Some(head)) => {
            let line = head.split(|b| *b == b'\r').next().unwrap_or_default();
            let line = String::from_utf8_lossy(line);
            let mut parts = line.split(' ');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("GET"), Some(target), Some(v)) if v.starts_with("HTTP/1.") => handler(target),
                (Some(_), Some(_), Some(v)) if v.starts_with("HTTP/1.") => (405, None),
                _ => (400, None),
            }
        }
    };
    let body = body.unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}
