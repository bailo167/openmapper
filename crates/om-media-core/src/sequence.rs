// SPDX-License-Identifier: Apache-2.0
//! Numbered image sequences played as video.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use om_time::{Rate, RationalTime};

use crate::source::{MediaDescriptor, MediaSource, VideoFrame};
use crate::{MediaError, StillImage};

/// A folder of still images (PNG/JPEG) shown in natural filename order at a
/// fixed rate. Frames are decoded on demand.
#[derive(Debug)]
pub struct ImageSequence {
    files: Vec<PathBuf>,
    rate: Rate,
    descriptor: MediaDescriptor,
    next: usize,
}

/// Natural-order key: digit runs compare numerically ("f2" < "f10").
fn natural_key(name: &str) -> Vec<(u8, u128, String)> {
    let mut out = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n = String::new();
            while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                n.push(d);
                chars.next();
            }
            out.push((0, n.parse().unwrap_or(u128::MAX), n));
        } else {
            let mut s = String::new();
            while let Some(&d) = chars.peek().filter(|d| !d.is_ascii_digit()) {
                s.push(d.to_ascii_lowercase());
                chars.next();
            }
            out.push((1, 0, s));
        }
    }
    out
}

impl ImageSequence {
    /// Opens every PNG/JPEG in `dir`. The first image fixes the size.
    pub fn open(dir: &Path, rate: Rate) -> Result<Self, MediaError> {
        let shown = dir.display().to_string();
        let entries = std::fs::read_dir(dir).map_err(|e| MediaError::Open {
            path: shown.clone(),
            message: e.to_string(),
        })?;
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
                })
            })
            .collect();
        files.sort_by_cached_key(|p| {
            natural_key(
                &p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        });
        let first = files.first().ok_or_else(|| MediaError::Open {
            path: shown.clone(),
            message: "folder contains no PNG or JPEG images".into(),
        })?;
        let img = StillImage::load(first)?;
        let count = i64::try_from(files.len()).unwrap_or(i64::MAX);
        let descriptor = MediaDescriptor {
            width: img.width(),
            height: img.height(),
            frame_rate: Some(rate),
            duration: RationalTime::from_frame(count, rate).ok(),
            audio: None,
            codec: format!("image sequence ({} files)", files.len()),
        };
        Ok(Self {
            files,
            rate,
            descriptor,
            next: 0,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl MediaSource for ImageSequence {
    fn descriptor(&self) -> &MediaDescriptor {
        &self.descriptor
    }

    fn seek(&mut self, t: RationalTime) -> Result<(), MediaError> {
        let i = t
            .to_frame_floor(self.rate)
            .map_err(|e| MediaError::Seek(t.to_string(), e.to_string()))?;
        self.next = usize::try_from(i.max(0))
            .unwrap_or(usize::MAX)
            .min(self.files.len());
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        let Some(path) = self.files.get(self.next) else {
            return Ok(None);
        };
        let img = StillImage::load(path)?;
        if (img.width(), img.height()) != (self.descriptor.width, self.descriptor.height) {
            return Err(MediaError::Stream(format!(
                "{}: size {}x{} differs from the sequence's {}x{}",
                path.display(),
                img.width(),
                img.height(),
                self.descriptor.width,
                self.descriptor.height
            )));
        }
        let index = i64::try_from(self.next).unwrap_or(i64::MAX);
        self.next += 1;
        Ok(Some(VideoFrame {
            pts: RationalTime::from_frame(index, self.rate)
                .map_err(|e| MediaError::Stream(e.to_string()))?,
            duration: self.rate.period().ok(),
            image: Arc::new(img),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FrameCursor;
    use om_project::PatternKind;

    fn write(dir: &Path, name: &str, shade: u8) {
        let mut img = StillImage::pattern(PatternKind::White, 4, 2)
            .unwrap()
            .rgba8()
            .to_vec();
        img[0] = shade;
        image::save_buffer(dir.join(name), &img, 4, 2, image::ExtendedColorType::Rgba8).unwrap();
    }

    #[test]
    fn natural_order_and_exact_timing() {
        let dir = tempfile::tempdir().unwrap();
        for (i, n) in [1, 2, 10, 3].iter().enumerate() {
            write(dir.path(), &format!("f{n}.png"), *n as u8);
            let _ = i;
        }
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let seq = ImageSequence::open(dir.path(), Rate::FPS_29_97).unwrap();
        assert_eq!(seq.len(), 4);
        let mut c = FrameCursor::new(seq);
        let shade = |c: &mut FrameCursor<ImageSequence>, t| {
            c.frame_at(t).unwrap().unwrap().image.pixel(0, 0).unwrap()[0]
        };
        // Frame 3 (f10) starts at exactly 3 * 1001/30000 s.
        let f3 = RationalTime::from_frame(3, Rate::FPS_29_97).unwrap();
        assert_eq!(shade(&mut c, f3), 10);
        let just_before = f3
            .checked_sub(RationalTime::new(1, 1_000_000).unwrap())
            .unwrap();
        assert_eq!(shade(&mut c, just_before), 3);
        assert_eq!(shade(&mut c, RationalTime::ZERO), 1);
        assert_eq!(
            shade(&mut c, RationalTime::from_seconds(100)),
            10,
            "holds last frame"
        );
    }

    #[test]
    fn empty_and_mismatched_folders_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ImageSequence::open(dir.path(), Rate::FPS_25).is_err());
        write(dir.path(), "a1.png", 0);
        let img = StillImage::pattern(PatternKind::White, 8, 8).unwrap();
        image::save_buffer(
            dir.path().join("a2.png"),
            img.rgba8(),
            8,
            8,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        let mut seq = ImageSequence::open(dir.path(), Rate::FPS_25).unwrap();
        seq.next_frame().unwrap();
        assert!(seq.next_frame().is_err());
    }
}
