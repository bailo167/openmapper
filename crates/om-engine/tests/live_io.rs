// SPDX-License-Identifier: Apache-2.0
//! Live I/O through the engine: one project publishes its output, another
//! receives it as live media.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use om_engine::{MediaRuntime, PublishRuntime};
use om_media_core::{LiveOpener, SinkOpener, SinkState, StillImage};
use om_project::{LiveInput, Media, MediaSource, Output, Project, Publish, StreamCodec};
use om_time::RationalTime;
use om_types::{MediaId, OutputId};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn frame(i: u32) -> Arc<StillImage> {
    let (w, h) = (40u32, 30u32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            #[allow(clippy::cast_possible_truncation)]
            px.extend_from_slice(&[(x * 5 + i) as u8, (y * 7) as u8, i as u8, 255]);
        }
    }
    Arc::new(StillImage::from_rgba8(w, h, px).unwrap())
}

const LIVE: MediaId = MediaId::from_u128(7);
const OUT: OutputId = OutputId::from_u128(9);

#[test]
fn published_output_arrives_as_live_media() {
    let port = free_port();
    let mut receiver = Project::new("receiver");
    receiver.media.push(Media {
        id: LIVE,
        name: "loopback".into(),
        source: MediaSource::Live {
            input: LiveInput::Stream {
                url: format!("tcp://127.0.0.1:{port}?listen=1"),
            },
        },
        playback: Default::default(),
        extensions: Default::default(),
    });
    receiver.validate().unwrap();
    let mut sender = Project::new("sender");
    sender.outputs.push(Output {
        id: OUT,
        name: "Main".into(),
        enabled: false,
        display: None,
        publish: vec![Publish::Stream {
            url: format!("tcp://127.0.0.1:{port}"),
            codec: StreamCodec::Lossless,
            fps: 30,
        }],
        extensions: Default::default(),
    });
    sender.validate().unwrap();
    // A saved project round-trips the new fields.
    let text = sender.to_canonical_json().unwrap();
    assert_eq!(Project::from_json(&text).unwrap().project, sender);

    let live: Arc<dyn LiveOpener> = Arc::new(om_media_ffmpeg::FfmpegLiveOpener);
    let sinks: Arc<dyn SinkOpener> = Arc::new(om_media_ffmpeg::FfmpegSinkOpener);
    let mut media = MediaRuntime::new(None).with_live(Some(live));
    let mut publish = PublishRuntime::new(Some(sinks));
    publish.sync(&sender);
    assert!(publish.is_active());

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut i = 0;
    let mut matched = 0;
    while matched < 10 && Instant::now() < deadline {
        let f = frame(i);
        publish.submit(OUT, &f);
        let changes = media.update(&receiver, None, RationalTime::ZERO);
        for (id, img) in &changes.upload {
            assert_eq!(*id, LIVE);
            // Must equal one of the frames sent so far (bit-exact).
            assert!(
                (0..=i).any(|k| frame(k).rgba8() == img.rgba8()),
                "received frame matches no sent frame"
            );
            matched += 1;
        }
        i += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = media.status(&receiver, LIVE, RationalTime::ZERO).unwrap();
    assert!(matched >= 10, "matched {matched}; status {status:?}");
    assert_eq!(status.error, None);
    assert!(status.summary.unwrap().contains("ffv1"));
    let out = publish.status(OUT, &sender.outputs[0].publish[0]).unwrap();
    assert_eq!(out.state, SinkState::Sending);

    // Removing the target stops the feed.
    sender.outputs[0].publish.clear();
    publish.sync(&sender);
    assert!(!publish.is_active());
}

#[test]
fn live_media_without_adapter_reports_why() {
    let mut p = Project::new("x");
    p.media.push(Media {
        id: LIVE,
        name: "cam".into(),
        source: MediaSource::Live {
            input: LiveInput::Camera {
                device: "Nope".into(),
            },
        },
        playback: Default::default(),
        extensions: Default::default(),
    });
    let mut media = MediaRuntime::new(None);
    let changes = media.update(&p, None, RationalTime::ZERO);
    assert_eq!(changes.unload, vec![LIVE]);
    let status = media.status(&p, LIVE, RationalTime::ZERO).unwrap();
    assert!(status.error.unwrap().contains("not available"));
}
