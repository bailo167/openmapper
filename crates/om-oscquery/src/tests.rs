// SPDX-License-Identifier: Apache-2.0

use std::io::{Read, Write};

use super::*;

fn snapshot() -> Snapshot {
    Snapshot {
        params: vec![
            ParamNode {
                address: "/openmapper/master/opacity".into(),
                kind: ParamKind::Float { min: 0.0, max: 1.0 },
                value: ParamValue::Float(0.75),
                description: "Master opacity".into(),
            },
            ParamNode {
                address: "/openmapper/master/blackout".into(),
                kind: ParamKind::Bool,
                value: ParamValue::Bool(false),
                description: "Blackout".into(),
            },
        ],
        actions: vec![
            ActionNode {
                address: "/openmapper/cue/go".into(),
                description: "Next cue".into(),
                argument: None,
            },
            ActionNode {
                address: "/openmapper/cue/release".into(),
                description: "Release".into(),
                argument: Some("f"),
            },
        ],
    }
}

#[test]
fn tree_follows_the_proposal() {
    let t = tree(&snapshot());
    let opacity = &t["CONTENTS"]["openmapper"]["CONTENTS"]["master"]["CONTENTS"]["opacity"];
    assert_eq!(opacity["FULL_PATH"], "/openmapper/master/opacity");
    assert_eq!(opacity["TYPE"], "f");
    assert_eq!(opacity["VALUE"], json!([0.75]));
    assert_eq!(opacity["RANGE"], json!([{ "MIN": 0.0, "MAX": 1.0 }]));
    assert_eq!(opacity["ACCESS"], 3);
    let blackout = &t["CONTENTS"]["openmapper"]["CONTENTS"]["master"]["CONTENTS"]["blackout"];
    assert_eq!(blackout["TYPE"], "F");
    let go = &t["CONTENTS"]["openmapper"]["CONTENTS"]["cue"]["CONTENTS"]["go"];
    assert_eq!(go["ACCESS"], 2);
    assert_eq!(
        t["CONTENTS"]["openmapper"]["CONTENTS"]["cue"]["FULL_PATH"],
        "/openmapper/cue"
    );
}

#[test]
fn queries() {
    let info = json!({ "NAME": "x" });
    let s = snapshot();
    assert_eq!(respond(&s, &info, "/?HOST_INFO"), (200, info.clone()));
    assert_eq!(
        respond(&s, &info, "/openmapper/master/opacity?VALUE"),
        (200, json!({ "VALUE": [0.75] }))
    );
    assert_eq!(respond(&s, &info, "/openmapper/master").0, 200);
    assert_eq!(respond(&s, &info, "/openmapper/nope").0, 404);
    assert_eq!(respond(&s, &info, "/openmapper/cue/go?RANGE").0, 204);
}

fn http_get(port: u16, path: &str) -> (u16, Value) {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

#[test]
fn served_over_http_with_live_updates() {
    let server = OscQueryServer::start(0, 9999, "OpenMapper test", false).unwrap();
    server.update(snapshot());
    let (status, root) = http_get(server.port(), "/");
    assert_eq!(status, 200);
    assert_eq!(
        root["CONTENTS"]["openmapper"]["CONTENTS"]["master"]["CONTENTS"]["opacity"]["VALUE"],
        json!([0.75])
    );
    let (_, info) = http_get(server.port(), "/?HOST_INFO");
    assert_eq!(info["OSC_PORT"], 9999);
    let mut s = snapshot();
    s.params[0].value = ParamValue::Float(0.1);
    server.update(s);
    assert_eq!(
        http_get(server.port(), "/openmapper/master/opacity?VALUE").1,
        json!({ "VALUE": [0.1] })
    );
    let port = server.port();
    drop(server);
    // Restartable on the same port.
    let again = OscQueryServer::start(port, 9999, "OpenMapper test", false).unwrap();
    assert_eq!(again.port(), port);
}

#[test]
fn hostile_clients_are_bounded() {
    use std::time::{Duration, Instant};
    let server = OscQueryServer::start(0, 9999, "OpenMapper test", false).unwrap();
    assert!(server.ip().is_loopback(), "local only unless enabled");
    server.update(snapshot());
    let port = server.port();
    let raw = |request: &[u8]| {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _ = s.write_all(request);
        let mut text = String::new();
        let _ = s.read_to_string(&mut text);
        text
    };
    // Endless headers: refused at the cap, not buffered forever.
    let mut big = b"GET / HTTP/1.1\r\n".to_vec();
    // Just over the cap, so the server consumes it all (no TCP reset).
    big.extend(std::iter::repeat_n(b'x', http::MAX_REQUEST_BYTES + 500));
    assert!(raw(&big).starts_with("HTTP/1.1 431"));
    assert!(raw(b"POST / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
    assert!(raw(b"garbage\r\n\r\n").starts_with("HTTP/1.1 400"));
    // Idle connections beyond the cap are closed, and the server still
    // answers once their deadline passes.
    let idle: Vec<_> = (0..http::MAX_CONNECTIONS + 4)
        .map(|_| std::net::TcpStream::connect(("127.0.0.1", port)).unwrap())
        .collect();
    let t = Instant::now();
    loop {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = write!(s, "GET /?HOST_INFO HTTP/1.1\r\n\r\n");
        let mut text = String::new();
        let _ = s.read_to_string(&mut text);
        if text.starts_with("HTTP/1.1 200") {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "server stays usable");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        t.elapsed() < http::REQUEST_DEADLINE * 3,
        "slow clients time out: {:?}",
        t.elapsed()
    );
    drop(idle);
}
