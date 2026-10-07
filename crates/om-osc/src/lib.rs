// SPDX-License-Identifier: Apache-2.0
//! OSC control.
//!
//! OpenMapper's OSC namespace lives under `/openmapper`:
//!
//! | Address | Arguments | Effect |
//! |---|---|---|
//! | `/openmapper/<param>` | float, int or bool | set a parameter (see `ParamId`) |
//! | `/openmapper/transport/play` · `pause` · `restart` | — | show transport |
//! | `/openmapper/cue/go` | — | run the next cue |
//! | `/openmapper/cue/<id>/go` | — | run a cue |
//! | `/openmapper/cue/release` | [fade seconds] | release cue values |
//! | `/openmapper/timeline/<id>/play` · `pause` · `stop` | — | timeline playback |
//! | `/openmapper/timeline/<id>/seek` | seconds | timeline position |
//!
//! Parameter addresses are the parameter's string form, e.g.
//! `/openmapper/surface/<id>/opacity`. The namespace is generated from the
//! parameter registry; it is OpenMapper's own (no other product's address
//! space is reproduced).

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use om_project::{ParamId, ParamValue};
use om_show::control::{Action, ControlMessage};
use rosc::{OscMessage, OscPacket, OscType};

/// Root of OpenMapper's OSC namespace.
pub const ROOT: &str = "/openmapper";

/// Why an OSC message was not understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OscError {
    #[error("not an OpenMapper address: {0}")]
    UnknownAddress(String),
    #[error("{address}: expected {expected}")]
    BadArguments {
        address: String,
        expected: &'static str,
    },
    #[error("malformed OSC packet: {0}")]
    Decode(String),
    #[error("cannot bind OSC port {port}: {message}")]
    Bind { port: u16, message: String },
}

fn number(arg: &OscType) -> Option<f64> {
    match arg {
        OscType::Float(f) => Some(f64::from(*f)),
        OscType::Double(d) => Some(*d),
        OscType::Int(i) => Some(f64::from(*i)),
        OscType::Long(l) => Some(*l as f64),
        OscType::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn value(arg: &OscType) -> Option<ParamValue> {
    match arg {
        OscType::Bool(b) => Some(ParamValue::Bool(*b)),
        other => number(other)
            .filter(|v| v.is_finite())
            .map(ParamValue::Float),
    }
}

/// Translates one OSC message into a control message.
pub fn translate(msg: &OscMessage) -> Result<ControlMessage, OscError> {
    let unknown = || OscError::UnknownAddress(msg.addr.clone());
    let rest = msg
        .addr
        .strip_prefix(ROOT)
        .and_then(|r| r.strip_prefix('/'))
        .ok_or_else(unknown)?;
    let parts: Vec<&str> = rest.split('/').collect();
    let action = |a| Ok(ControlMessage::Action(a));
    match parts.as_slice() {
        ["transport", "play"] => action(Action::Play),
        ["transport", "pause"] => action(Action::Pause),
        ["transport", "restart"] => action(Action::Restart),
        ["cue", "go"] => action(Action::CueGoNext),
        ["cue", "release"] => action(Action::CueRelease {
            fade: msg.args.first().and_then(number).unwrap_or(0.0).max(0.0),
        }),
        ["cue", id, "go"] => action(Action::CueGo(id.parse().map_err(|_| unknown())?)),
        ["timeline", id, op] => {
            let id = id.parse().map_err(|_| unknown())?;
            match *op {
                "play" => action(Action::TimelinePlay(id)),
                "pause" => action(Action::TimelinePause(id)),
                "stop" => action(Action::TimelineStop(id)),
                "seek" => {
                    let t = msg
                        .args
                        .first()
                        .and_then(number)
                        .ok_or(OscError::BadArguments {
                            address: msg.addr.clone(),
                            expected: "a position in seconds",
                        })?;
                    action(Action::TimelineSeek(id, t.max(0.0)))
                }
                _ => Err(unknown()),
            }
        }
        _ => {
            let param: ParamId = rest.parse().map_err(|_| unknown())?;
            let v = msg
                .args
                .first()
                .and_then(value)
                .ok_or(OscError::BadArguments {
                    address: msg.addr.clone(),
                    expected: "one float, int or bool",
                })?;
            Ok(ControlMessage::Set { param, value: v })
        }
    }
}

/// Decodes a UDP datagram (message or bundle) into control messages.
pub fn decode(datagram: &[u8]) -> Vec<Result<ControlMessage, OscError>> {
    match rosc::decoder::decode_udp(datagram) {
        Ok((_, packet)) => {
            let mut out = Vec::new();
            flatten(&packet, &mut out);
            out
        }
        Err(e) => vec![Err(OscError::Decode(e.to_string()))],
    }
}

fn flatten(p: &OscPacket, out: &mut Vec<Result<ControlMessage, OscError>>) {
    match p {
        OscPacket::Message(m) => out.push(translate(m)),
        OscPacket::Bundle(b) => b.content.iter().for_each(|c| flatten(c, out)),
    }
}

/// The OSC address for a parameter.
#[must_use]
pub fn address(param: &ParamId) -> String {
    format!("{ROOT}/{param}")
}

/// Receives OSC on a UDP port in a background thread.
pub struct OscServer {
    port: u16,
    rx: Receiver<Result<ControlMessage, OscError>>,
    stop: Arc<AtomicBool>,
    received: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for OscServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OscServer(:{})", self.port)
    }
}

impl OscServer {
    /// Binds `0.0.0.0:port` (0 picks a free port; see [`OscServer::port`]).
    pub fn start(port: u16) -> Result<Self, OscError> {
        let bind_err = |e: std::io::Error| OscError::Bind {
            port,
            message: e.to_string(),
        };
        let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(bind_err)?;
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(bind_err)?;
        let port = socket.local_addr().map_err(bind_err)?.port();
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let received = Arc::new(AtomicU64::new(0));
        let (s, r) = (Arc::clone(&stop), Arc::clone(&received));
        let thread = std::thread::Builder::new()
            .name("om-osc".into())
            .spawn(move || receive_loop(&socket, &tx, &s, &r))
            .ok();
        Ok(Self {
            port,
            rx,
            stop,
            received,
            thread,
        })
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Datagrams received so far.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// Messages received since the last call (never blocks).
    pub fn drain(&self) -> Vec<Result<ControlMessage, OscError>> {
        self.rx.try_iter().collect()
    }
}

fn receive_loop(
    socket: &UdpSocket,
    tx: &Sender<Result<ControlMessage, OscError>>,
    stop: &AtomicBool,
    received: &AtomicU64,
) {
    let mut buf = vec![0u8; rosc::decoder::MTU];
    while !stop.load(Ordering::Relaxed) {
        match socket.recv(&mut buf) {
            Ok(n) => {
                received.fetch_add(1, Ordering::Relaxed);
                for m in decode(&buf[..n]) {
                    if tx.send(m).is_err() {
                        return;
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

impl Drop for OscServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests;
