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
