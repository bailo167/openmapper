// SPDX-License-Identifier: Apache-2.0
//! OSCQuery: an HTTP/JSON description of OpenMapper's OSC address space,
//! following the public OSCQuery proposal.
//!
//! The engine publishes a [`Snapshot`] (parameters with live values, plus
//! actions); any HTTP client can then discover the tree:
//!
//! - `GET /` — the whole tree; `GET /openmapper/master` — a subtree.
//! - `GET /<path>?VALUE` (or `TYPE`, `RANGE`, `ACCESS`, …) — one attribute.
//! - `GET /?HOST_INFO` — server name, OSC port and transport.
//!
//! The service is optionally advertised over mDNS as `_oscjson._tcp`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use om_project::{ParamKind, ParamValue};
use serde_json::{Map, Value, json};

/// One parameter leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamNode {
    /// Full OSC address, e.g. `/openmapper/master/opacity`.
    pub address: String,
    pub kind: ParamKind,
    pub value: ParamValue,
    pub description: String,
}

/// One write-only action leaf (no arguments or one float).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionNode {
    pub address: String,
    pub description: String,
    /// OSC type tag of its argument, if any (`"f"`).
    pub argument: Option<&'static str>,
}

/// What the service describes right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub params: Vec<ParamNode>,
    pub actions: Vec<ActionNode>,
}

/// Service failure.
#[derive(Debug, thiserror::Error)]
#[error("cannot start OSCQuery on port {port}: {message}")]
pub struct OscQueryError {
    pub port: u16,
    pub message: String,
}

/// Builds the JSON tree for a snapshot.
#[must_use]
pub fn tree(snapshot: &Snapshot) -> Value {
    let mut root = container("/");
    for p in &snapshot.params {
        let mut leaf = Map::new();
        leaf.insert("FULL_PATH".into(), json!(p.address));
        leaf.insert("DESCRIPTION".into(), json!(p.description));
        leaf.insert("ACCESS".into(), json!(3));
        match p.kind {
            ParamKind::Bool => {
                let b = p.value.as_bool();
                leaf.insert("TYPE".into(), json!(if b { "T" } else { "F" }));
                leaf.insert("VALUE".into(), json!([b]));
            }
            ParamKind::Float { min, max } => {
                leaf.insert("TYPE".into(), json!("f"));
                leaf.insert("VALUE".into(), json!([p.value.as_f64()]));
                if min > f64::MIN && max < f64::MAX {
                    leaf.insert("RANGE".into(), json!([{ "MIN": min, "MAX": max }]));
                }
            }
        }
        insert(&mut root, &p.address, Value::Object(leaf));
    }
    for a in &snapshot.actions {
        let mut leaf = Map::new();
        leaf.insert("FULL_PATH".into(), json!(a.address));
        leaf.insert("DESCRIPTION".into(), json!(a.description));
        leaf.insert("ACCESS".into(), json!(2));
        if let Some(t) = a.argument {
            leaf.insert("TYPE".into(), json!(t));
        }
        insert(&mut root, &a.address, Value::Object(leaf));
    }
    root
}

fn container(path: &str) -> Value {
    json!({ "FULL_PATH": path, "ACCESS": 0, "CONTENTS": {} })
}

fn insert(root: &mut Value, address: &str, leaf: Value) {
    let parts: Vec<&str> = address.trim_matches('/').split('/').collect();
    let mut node = root;
    let mut path = String::new();
    for (i, part) in parts.iter().enumerate() {
        path.push('/');
        path.push_str(part);
        let Some(contents) = node.get_mut("CONTENTS").and_then(Value::as_object_mut) else {
            return;
        };
        if i + 1 == parts.len() {
            // A leaf may also be a container (e.g. /cue/go next to /cue/<id>);
            // keep existing CONTENTS when both exist.
            let existing = contents.remove(*part);
            let mut leaf = leaf;
            if let (Some(Value::Object(old)), Value::Object(new)) = (existing, &mut leaf)
                && let Some(c) = old.get("CONTENTS")
            {
                new.insert("CONTENTS".into(), c.clone());
            }
            contents.insert((*part).to_owned(), leaf);
            return;
        }
        node = contents
            .entry((*part).to_owned())
            .or_insert_with(|| container(&path));
        if node.get("CONTENTS").is_none()
            && let Some(obj) = node.as_object_mut()
        {
            obj.insert("CONTENTS".into(), json!({}));
        }
    }
}

/// Answers one OSCQuery request: `(status, json body)`.
#[must_use]
pub fn respond(snapshot: &Snapshot, host_info: &Value, url: &str) -> (u16, Value) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if query == "HOST_INFO" {
        return (200, host_info.clone());
    }
    let full = tree(snapshot);
    let mut node = &full;
    for part in path.trim_matches('/').split('/').filter(|p| !p.is_empty()) {
        match node.get("CONTENTS").and_then(|c| c.get(part)) {
            Some(n) => node = n,
            None => return (404, json!({ "error": "no such node" })),
        }
    }
    if query.is_empty() {
        return (200, node.clone());
    }
    match node.get(query) {
        Some(v) => (200, json!({ query: v })),
        None => (204, Value::Null),
    }
}

/// The running HTTP service.
pub struct OscQueryServer {
    port: u16,
    snapshot: Arc<RwLock<Snapshot>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    mdns: Option<mdns_sd::ServiceDaemon>,
}

impl std::fmt::Debug for OscQueryServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OscQueryServer(:{})", self.port)
    }
}

impl OscQueryServer {
    /// Serves on `port` (0 picks a free one). `osc_port` is reported in
    /// HOST_INFO. With `advertise`, registers `_oscjson._tcp` over mDNS
    /// (failure to advertise is not an error).
    pub fn start(
        port: u16,
        osc_port: u16,
        name: &str,
        advertise: bool,
    ) -> Result<Self, OscQueryError> {
        let err = |m: String| OscQueryError { port, message: m };
        let server = tiny_http::Server::http(("0.0.0.0", port)).map_err(|e| err(e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| err("no TCP address".into()))?;
        let snapshot = Arc::new(RwLock::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let host_info = json!({
            "NAME": name,
            "OSC_PORT": osc_port,
            "OSC_TRANSPORT": "UDP",
            "EXTENSIONS": {
                "ACCESS": true, "VALUE": true, "RANGE": true, "DESCRIPTION": true,
                "TYPE": true, "LISTEN": false, "PATH_CHANGED": false
            }
        });
        let (snap, s) = (Arc::clone(&snapshot), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name("om-oscquery".into())
            .spawn(move || serve(&server, &snap, &host_info, &s))
            .ok();
        let mdns = advertise.then(|| advertise_service(name, port)).flatten();
        Ok(Self {
            port,
            snapshot,
            stop,
            thread,
            mdns,
        })
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Replaces the described namespace and values.
    pub fn update(&self, snapshot: Snapshot) {
        if let Ok(mut s) = self.snapshot.write() {
            *s = snapshot;
        }
    }
}

fn serve(
    server: &tiny_http::Server,
    snapshot: &RwLock<Snapshot>,
    host_info: &Value,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::Relaxed) {
        let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        let (status, body) = match snapshot.read() {
            Ok(s) => respond(&s, host_info, request.url()),
            Err(_) => (500, json!({ "error": "unavailable" })),
        };
        let text = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let mut response = tiny_http::Response::from_string(text).with_status_code(status);
        if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        {
            response.add_header(h);
        }
        let _ = request.respond(response);
    }
}

fn advertise_service(name: &str, port: u16) -> Option<mdns_sd::ServiceDaemon> {
    let daemon = mdns_sd::ServiceDaemon::new().ok()?;
    let host = "openmapper.local.";
    let props: [(&str, &str); 0] = [];
    let info = mdns_sd::ServiceInfo::new("_oscjson._tcp.local.", name, host, "", port, &props[..])
        .ok()?
        .enable_addr_auto();
    daemon.register(info).ok()?;
    Some(daemon)
}

impl Drop for OscQueryServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(d) = self.mdns.take() {
            let _ = d.shutdown();
        }
    }
}

/// Convenience: group snapshot leaves by top-level address (for UIs).
#[must_use]
pub fn group_by_prefix(snapshot: &Snapshot) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for p in &snapshot.params {
        let key = p.address.split('/').nth(2).unwrap_or("").to_owned();
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests;
