// SPDX-License-Identifier: Apache-2.0
//! Network stream loopbacks: OpenMapper publishing to OpenMapper over
//! localhost, in-process and across processes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use om_media_core::{
    LiveConfig, LiveFeed, LiveOpener, LiveState, PublishFeed, SinkOpener, SinkState, StillImage,
};
use om_media_ffmpeg::{FfmpegLiveOpener, FfmpegSinkOpener};
use om_project::{LiveInput, Publish, StreamCodec};

const W: u32 = 96;
const H: u32 = 64;

fn quick() -> LiveConfig {
    LiveConfig {
        min_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(400),
        poll: Duration::from_millis(20),
    }
}

/// A frame whose every pixel depends on `i`, including partial alpha.
fn test_frame(i: u32) -> Arc<StillImage> {
    let mut px = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            #[allow(clippy::cast_possible_truncation)]
            px.extend_from_slice(&[
                (x * 7 + i * 11) as u8,
                (y * 13 + i * 3) as u8,
                ((x ^ y) + i * 5) as u8,
                (x * 2 + y + i) as u8 | 1,
            ]);
        }
    }
    Arc::new(StillImage::from_rgba8(W, H, px).unwrap())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn receiver(url: &str) -> LiveFeed {
    LiveFeed::spawn_with(
        Arc::new(FfmpegLiveOpener) as Arc<dyn LiveOpener>,
        LiveInput::Stream { url: url.into() },
        quick(),
    )
}

fn sender(url: &str, codec: StreamCodec) -> PublishFeed {
    PublishFeed::spawn_with(
        Arc::new(FfmpegSinkOpener) as Arc<dyn SinkOpener>,
        Publish::Stream {
            url: url.into(),
            codec,
            fps: 30,
        },
        quick(),
    )
}

/// Feeds frames `range` to `tx` at ~60 Hz, returning a map from pixels to
/// frame index.
fn pump(tx: &PublishFeed, range: std::ops::Range<u32>, sent: &mut HashMap<Vec<u8>, u32>) {
    for i in range {
        let f = test_frame(i);
        sent.insert(f.rgba8().to_vec(), i);
        tx.submit(f);
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Sends frames `first..first + count`, collecting frames received after
/// sequence number `seen` (updated).
fn exchange(
    tx: &PublishFeed,
    rx: &LiveFeed,
    first: u32,
    count: u32,
    sent: &mut HashMap<Vec<u8>, u32>,
    seen: &mut u64,
) -> Vec<Arc<StillImage>> {
    let mut got = Vec::new();
    for i in first..first + count {
        pump(tx, i..i + 1, sent);
        while let Some(f) = rx.newer_than(*seen) {
            *seen = f.seq;
            got.push(f.image);
        }
    }
    got
}

#[test]
fn lossless_tcp_loopback_is_bit_exact() {
    let port = free_port();
    let rx = receiver(&format!("tcp://127.0.0.1:{port}?listen=1"));
    let tx = sender(&format!("tcp://127.0.0.1:{port}"), StreamCodec::Lossless);
    let mut sent = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got = Vec::new();
    let mut i = 0;
    let mut seen = 0;
    while got.len() < 30 && Instant::now() < deadline {
        got.extend(exchange(&tx, &rx, i, 10, &mut sent, &mut seen));
        i += 10;
    }
    assert!(
        got.len() >= 30,
        "received {} frames; rx {:?} tx {:?}",
        got.len(),
        rx.state(),
        tx.state()
    );
    let mut last = 0;
    for img in &got {
        assert_eq!((img.width(), img.height()), (W, H));
        let idx = *sent
            .get(img.rgba8())
            .expect("every received frame is bit-identical to a sent frame");
        assert!(idx >= last, "frames arrive in order ({idx} after {last})");
        last = idx;
    }
    assert_eq!(rx.state(), LiveState::Live);
    assert_eq!(tx.state(), SinkState::Sending);
}

#[test]
fn compatible_udp_stream_round_trips_colour() {
    let port = free_port();
    // UDP: the receiver binds the port; the sender just sends datagrams.
    let rx = receiver(&format!("udp://127.0.0.1:{port}"));
    let tx = sender(
        &format!("udp://127.0.0.1:{port}?pkt_size=1316"),
        StreamCodec::Compatible,
    );
    // Flat colour blocks survive MPEG-2 coding; check them within ±4.
    let colours: [[u8; 4]; 4] = [
        [200, 40, 40, 255],
        [40, 200, 40, 255],
        [40, 40, 200, 255],
        [128, 128, 128, 255],
    ];
    let (w, h) = (128u32, 96u32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&colours[((y / 48) * 2 + x / 64) as usize]);
        }
    }
    let img = Arc::new(StillImage::from_rgba8(w, h, px).unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut frame = None;
    while frame.is_none() && Instant::now() < deadline {
        tx.submit(Arc::clone(&img));
        std::thread::sleep(Duration::from_millis(30));
        // Skip the first frames (decoder warm-up after joining mid-GOP).
        if rx.stats().frames > 10 {
            frame = rx.newer_than(0);
        }
    }
    let frame =
        frame.unwrap_or_else(|| panic!("no frames; rx {:?} tx {:?}", rx.state(), tx.state()));
    assert_eq!((frame.image.width(), frame.image.height()), (w, h));
    for (k, c) in colours.iter().enumerate() {
        let (x, y) = (32 + 64 * (k as u32 % 2), 24 + 48 * (k as u32 / 2));
        let p = frame.image.pixel(x, y).unwrap();
        for ch in 0..3 {
            let d = (i32::from(p[ch]) - i32::from(c[ch])).abs();
            assert!(d <= 4, "block {k} channel {ch}: sent {c:?}, got {p:?}");
        }
    }
}

#[test]
fn receiver_and_sender_reconnect_repeatedly() {
    let port = free_port();
    let url_rx = format!("tcp://127.0.0.1:{port}?listen=1");
    let url_tx = format!("tcp://127.0.0.1:{port}");
    let mut rx = receiver(&url_rx);
    let mut sent = HashMap::new();
    let mut next = 0;
    for round in 0..6 {
        // Alternate which side goes away.
        let tx = sender(&url_tx, StreamCodec::Lossless);
        if round % 2 == 1 {
            rx = receiver(&url_rx);
        }
        let before = rx.stats().frames;
        let mut seen = before;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut got = Vec::new();
        while got.len() < 5 && Instant::now() < deadline {
            got.extend(exchange(&tx, &rx, next, 5, &mut sent, &mut seen));
            next += 5;
        }
        assert!(
            got.len() >= 5,
            "round {round}: only {} frames (rx {:?}, tx {:?})",
            got.len(),
            rx.state(),
            tx.state()
        );
        for img in &got {
            assert!(
                sent.contains_key(img.rgba8()),
                "round {round}: corrupted frame"
            );
        }
        assert!(rx.stats().frames > before);
        drop(tx);
    }
}

#[test]
fn unreachable_stream_reports_and_stops_promptly() {
    let port = free_port();
    // Nothing sends to this port: the receiver must time out, not hang.
    let rx = receiver(&format!("udp://127.0.0.1:{port}"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(rx.state(), LiveState::Retrying { .. }) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        matches!(rx.state(), LiveState::Retrying { .. }),
        "{:?}",
        rx.state()
    );
    let t = Instant::now();
    drop(rx);
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

/// Child half of `cross_process_stream_is_bit_exact`: publishes frames to
/// the port in `OM_TEST_STREAM_PORT` until killed. Does nothing otherwise.
#[test]
fn child_stream_sender() {
    let Ok(port) = std::env::var("OM_TEST_STREAM_PORT") else {
        return;
    };
    let tx = sender(&format!("tcp://127.0.0.1:{port}"), StreamCodec::Lossless);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut i = 0;
    while Instant::now() < deadline {
        tx.submit(test_frame(i));
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
}

#[test]
fn cross_process_stream_is_bit_exact() {
    let port = free_port();
    let rx = receiver(&format!("tcp://127.0.0.1:{port}?listen=1"));
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "child_stream_sender",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("OM_TEST_STREAM_PORT", port.to_string())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // The child's frames follow test_frame(i); index them up front.
    let sent: HashMap<Vec<u8>, u32> = (0..4000)
        .map(|i| (test_frame(i).rgba8().to_vec(), i))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = 0;
    let mut matched = 0;
    while matched < 20 && Instant::now() < deadline {
        if let Some(f) = rx.wait_newer_than(seen, Duration::from_millis(200)) {
            seen = f.seq;
            assert!(
                sent.contains_key(f.image.rgba8()),
                "frame from the other process differs from what it sent"
            );
            matched += 1;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        matched >= 20,
        "matched {matched} frames; rx {:?}",
        rx.state()
    );
}
