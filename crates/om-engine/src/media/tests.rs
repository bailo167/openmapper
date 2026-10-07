// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::time::{Duration, Instant};

use om_media_core::{
    MediaDescriptor, MediaError, MediaSource as Decoder, StillImage, VideoFrame, VideoOpener,
};
use om_project::{Canvas, Media, MediaSource, PatternKind, Playback, Project};
use om_time::{Rate, RationalTime, Speed};
use om_types::MediaId;

use super::*;

fn rt(n: i128, d: i128) -> RationalTime {
    RationalTime::new(n, d).unwrap()
}

#[test]
fn transport_time_is_exact_and_pausable() {
    let t0 = Instant::now();
    let mut tr = Transport::default();
    assert_eq!(tr.time(t0), RationalTime::ZERO);
    tr.play(t0);
    assert_eq!(tr.time(t0 + Duration::from_millis(1500)), rt(3, 2));
    tr.pause(t0 + Duration::from_millis(2000));
    assert_eq!(
        tr.time(t0 + Duration::from_secs(99)),
        RationalTime::from_seconds(2)
    );
    tr.play(t0 + Duration::from_secs(10));
    assert_eq!(
        tr.time(t0 + Duration::from_secs(11)),
        RationalTime::from_seconds(3)
    );
    tr.seek(RationalTime::ZERO, t0 + Duration::from_secs(11));
    assert!(tr.is_playing());
    // 24 hours later, still exact to the nanosecond.
    let day = Duration::from_secs(86_400);
    assert_eq!(
        tr.time(t0 + Duration::from_secs(11) + day),
        RationalTime::from_seconds(86_400)
    );
}

#[test]
fn media_time_loops_clamps_and_reverses() {
    let d = Some(RationalTime::from_seconds(10));
    let pb = |looping, n, den| Playback {
        looping,
        speed: Speed::new(n, den).unwrap(),
    };
    let s = RationalTime::from_seconds;
    assert_eq!(media_time(s(25), s(0), pb(true, 1, 1), d), s(5));
    assert_eq!(media_time(s(25), s(0), pb(false, 1, 1), d), s(10));
    assert_eq!(
        media_time(s(25), s(20), pb(true, 1, 1), d),
        s(5),
        "restart offset"
    );
    assert_eq!(
        media_time(s(3), s(0), pb(true, 1, 2), d),
        rt(3, 2),
        "half speed"
    );
    assert_eq!(
        media_time(s(3), s(0), pb(true, -1, 1), d),
        s(7),
        "reverse wraps"
    );
    assert_eq!(
        media_time(s(3), s(0), pb(false, -1, 1), d),
        s(0),
        "reverse clamps"
    );
    assert_eq!(media_time(s(3), s(0), pb(true, 0, 1), d), s(0), "stopped");
}

/// Fake decoder: 25 fps, frame i is 1x1 with red = i.
struct Fake {
    next: i64,
    frames: i64,
    desc: MediaDescriptor,
}

impl Fake {
    fn new(frames: i64) -> Self {
        Self {
            next: 0,
            frames,
            desc: MediaDescriptor {
                width: 1,
                height: 1,
                frame_rate: Some(Rate::FPS_25),
                duration: RationalTime::from_frame(frames, Rate::FPS_25).ok(),
                audio: None,
                codec: "fake".into(),
            },
        }
    }
}

impl Decoder for Fake {
    fn descriptor(&self) -> &MediaDescriptor {
        &self.desc
    }
    fn seek(&mut self, t: RationalTime) -> Result<(), MediaError> {
        self.next = t
            .to_frame_floor(Rate::FPS_25)
            .unwrap_or(0)
            .clamp(0, self.frames);
        Ok(())
    }
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        if self.next >= self.frames {
            return Ok(None);
        }
        let i = self.next;
        self.next += 1;
        Ok(Some(VideoFrame {
            pts: RationalTime::from_frame(i, Rate::FPS_25).unwrap(),
            duration: None,
            image: Arc::new(StillImage::from_rgba8(1, 1, vec![i as u8, 0, 0, 255]).unwrap()),
        }))
    }
}

struct FakeOpener;

impl VideoOpener for FakeOpener {
    fn open_video(&self, _path: &std::path::Path) -> Result<Box<dyn Decoder>, MediaError> {
        Ok(Box::new(Fake::new(50)))
    }
}

fn media(id: u128, source: MediaSource, playback: Playback) -> Media {
    Media {
        id: MediaId::from_u128(id),
        name: "m".into(),
        source,
        playback,
        extensions: Default::default(),
    }
}

fn red(changes: &MediaChanges, id: u128) -> Option<u8> {
    changes
        .upload
        .iter()
        .find(|(m, _)| *m == MediaId::from_u128(id))
        .and_then(|(_, img)| img.pixel(0, 0))
        .map(|p| p[0])
}

#[test]
fn runtime_plays_video_at_show_time_and_loops() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("clip.mp4"), b"fake").unwrap();
    let mut p = Project::new("p");
    p.canvas = Canvas {
        width: 8,
        height: 8,
    };
    p.media.push(media(
        1,
        MediaSource::Video {
            path: "clip.mp4".into(),
        },
        Playback::default(),
    ));
    p.media.push(media(
        2,
        MediaSource::Pattern {
            pattern: PatternKind::White,
        },
        Playback::default(),
    ));
    let mut rtm = MediaRuntime::new(Some(Arc::new(FakeOpener)));
    let wait = Duration::from_secs(5);
    let at = |s: i64, f: i64| {
        RationalTime::from_seconds(s)
            .checked_add(RationalTime::from_frame(f, Rate::FPS_25).unwrap())
            .unwrap()
    };

    let c = rtm.update_blocking(&p, Some(dir.path()), at(0, 0), wait);
    assert_eq!(red(&c, 1), Some(0));
    assert_eq!(c.upload.len(), 2, "pattern uploaded once");
    let c = rtm.update_blocking(&p, Some(dir.path()), at(1, 7), wait);
    assert_eq!(red(&c, 1), Some(32));
    assert_eq!(c.upload.len(), 1, "unchanged pattern not re-uploaded");
    // 50 frames = 2 s; at 2 s + 3 frames it has looped.
    let c = rtm.update_blocking(&p, Some(dir.path()), at(2, 3), wait);
    assert_eq!(red(&c, 1), Some(3));
    // Same frame again: nothing to upload.
    assert!(
        rtm.update_blocking(&p, Some(dir.path()), at(2, 3), wait)
            .upload
            .is_empty()
    );

    let st = rtm.status(&p, MediaId::from_u128(1), at(2, 3)).unwrap();
    assert_eq!(st.position, Some(at(0, 3)));
    assert_eq!(st.duration, Some(RationalTime::from_seconds(2)));
}

#[test]
fn missing_files_and_missing_opener_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::new("p");
    p.media.push(media(
        1,
        MediaSource::Video {
            path: "nope.mp4".into(),
        },
        Playback::default(),
    ));
    p.media.push(media(
        2,
        MediaSource::Image {
            path: "nope.png".into(),
        },
        Playback::default(),
    ));
    let mut rtm = MediaRuntime::new(None);
    let c = rtm.update(&p, Some(dir.path()), RationalTime::ZERO);
    assert_eq!(c.unload.len(), 2);
    let s = rtm
        .status(&p, MediaId::from_u128(1), RationalTime::ZERO)
        .unwrap();
    assert!(s.error.unwrap().contains("not available"), "no opener");
    let s = rtm
        .status(&p, MediaId::from_u128(2), RationalTime::ZERO)
        .unwrap();
    assert!(s.error.unwrap().contains("not found"));
}

#[test]
fn sequences_play_from_folders() {
    let dir = tempfile::tempdir().unwrap();
    let seq = dir.path().join("seq");
    std::fs::create_dir(&seq).unwrap();
    for i in 0..5u8 {
        image::save_buffer(
            seq.join(format!("img_{i:03}.png")),
            &[i * 10, 0, 0, 255],
            1,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    }
    let mut p = Project::new("p");
    p.media.push(media(
        1,
        MediaSource::Sequence {
            path: "seq".into(),
            rate: Rate::FPS_25,
        },
        Playback {
            looping: false,
            speed: Speed::NORMAL,
        },
    ));
    let mut rtm = MediaRuntime::new(None);
    let c = rtm.update_blocking(
        &p,
        Some(dir.path()),
        RationalTime::from_frame(3, Rate::FPS_25).unwrap(),
        Duration::from_secs(5),
    );
    assert_eq!(red(&c, 1), Some(30));
    // Not looping: holds the last frame.
    let c = rtm.update_blocking(
        &p,
        Some(dir.path()),
        RationalTime::from_seconds(10),
        Duration::from_secs(5),
    );
    assert_eq!(red(&c, 1), Some(40));
}

#[test]
fn storage_paths_are_relative_under_project_dir() {
    let dir = std::path::Path::new("/shows/demo");
    assert_eq!(
        path_for_storage(Some(dir), std::path::Path::new("/shows/demo/media/a b.png")),
        "media/a b.png"
    );
    assert_eq!(
        path_for_storage(Some(dir), std::path::Path::new("/elsewhere/x.png")),
        "/elsewhere/x.png"
    );
    assert_eq!(
        resolve_media_path(Some(dir), "media/a.png"),
        std::path::PathBuf::from("/shows/demo/media/a.png")
    );
}
