// SPDX-License-Identifier: Apache-2.0
//! Media abstractions.
//!
//! This milestone provides still images (decoded from PNG/JPEG) and
//! procedurally generated patterns. Pixels are 8-bit sRGB-encoded RGBA with
//! straight (non-premultiplied) alpha, row-major, top row first; the renderer
//! converts to linear premultiplied on upload.

mod player;
mod sequence;
mod source;

use std::path::Path;

use om_project::PatternKind;

pub use player::{DEFAULT_QUEUE, PlayerStats, VideoPlayer};
pub use sequence::ImageSequence;
pub use source::{
    AudioBlock, AudioFormat, CursorStats, FrameCursor, MediaDescriptor, MediaSource, VideoFrame,
};

/// Largest accepted image dimension (matches the canvas limit).
pub const MAX_IMAGE_DIMENSION: u32 = 16384;

/// Media loading failure.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{path}: file not found")]
    NotFound { path: String },
    #[error("{path}: could not decode image: {message}")]
    Decode { path: String, message: String },
    #[error("{path}: image is {width}x{height}; maximum is {MAX_IMAGE_DIMENSION} per side")]
    TooLarge {
        path: String,
        width: u32,
        height: u32,
    },
    #[error("image dimensions must be non-zero")]
    Empty,
    #[error("{path}: {message}")]
    Open { path: String, message: String },
    #[error("decode error: {0}")]
    Stream(String),
    #[error("seek to {0} failed: {1}")]
    Seek(String, String),
    #[error("{0} has no video stream")]
    NoVideo(String),
}

/// A decoded still image.
#[derive(Clone, PartialEq, Eq)]
pub struct StillImage {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

impl std::fmt::Debug for StillImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StillImage({}x{})", self.width, self.height)
    }
}

impl StillImage {
    /// Wraps raw sRGB RGBA8 pixels. `rgba8.len()` must equal `width * height * 4`.
    pub fn from_rgba8(width: u32, height: u32, rgba8: Vec<u8>) -> Result<Self, MediaError> {
        let expected = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || rgba8.len() != expected {
            return Err(MediaError::Empty);
        }
        Ok(Self {
            width,
            height,
            rgba8,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn rgba8(&self) -> &[u8] {
        &self.rgba8
    }

    /// Pixel at `(x, y)`; `None` outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + x as usize) * 4;
        let p = self.rgba8.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    /// Decodes a PNG or JPEG file.
    pub fn load(path: &Path) -> Result<Self, MediaError> {
        let shown = path.display().to_string();
        if !path.is_file() {
            return Err(MediaError::NotFound { path: shown });
        }
        let reader = image::ImageReader::open(path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|e| MediaError::Decode {
                path: shown.clone(),
                message: e.to_string(),
            })?;
        let (w, h) = reader.into_dimensions().map_err(|e| MediaError::Decode {
            path: shown.clone(),
            message: e.to_string(),
        })?;
        if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
            return Err(MediaError::TooLarge {
                path: shown,
                width: w,
                height: h,
            });
        }
        let img = image::ImageReader::open(path)
            .and_then(image::ImageReader::with_guessed_format)
            .map_err(|e| MediaError::Decode {
                path: shown.clone(),
                message: e.to_string(),
            })?
            .decode()
            .map_err(|e| MediaError::Decode {
                path: shown.clone(),
                message: e.to_string(),
            })?
            .to_rgba8();
        let (width, height) = img.dimensions();
        Self::from_rgba8(width, height, img.into_raw())
    }

    /// Generates a built-in pattern at the given size.
    pub fn pattern(kind: PatternKind, width: u32, height: u32) -> Result<Self, MediaError> {
        if width == 0 || height == 0 {
            return Err(MediaError::Empty);
        }
        let mut px = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for y in 0..height {
            for x in 0..width {
                px.extend_from_slice(&pattern_pixel(kind, x, y, width, height));
            }
        }
        Self::from_rgba8(width, height, px)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pattern_pixel(kind: PatternKind, x: u32, y: u32, w: u32, h: u32) -> [u8; 4] {
    // Integer cell maths keeps patterns exact at any resolution.
    let cell_x = (u64::from(x) * 8) / u64::from(w);
    let cell_y = (u64::from(y) * 8) / u64::from(h);
    match kind {
        PatternKind::White => [255, 255, 255, 255],
        PatternKind::Checkerboard => {
            if (cell_x + cell_y) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        }
        PatternKind::UvGrid => {
            // A pixel is on a grid line if it is the first pixel of a cell
            // (or the last pixel of the image, closing the border).
            let line_x = x == w - 1
                || cell_x != (u64::from(x.saturating_sub(1)) * 8) / u64::from(w)
                || x == 0;
            let line_y = y == h - 1
                || cell_y != (u64::from(y.saturating_sub(1)) * 8) / u64::from(h)
                || y == 0;
            if line_x || line_y {
                return [255, 255, 255, 255];
            }
            // Red encodes horizontal position, green vertical; blue marks the
            // top-left quadrant so orientation/mirroring is unambiguous.
            let r = ((u64::from(x) * 255) / u64::from(w - 1).max(1)) as u8;
            let g = ((u64::from(y) * 255) / u64::from(h - 1).max(1)) as u8;
            let b = if cell_x < 4 && cell_y < 4 { 200 } else { 40 };
            [r, g, b, 255]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_are_deterministic_and_oriented() {
        let a = StillImage::pattern(PatternKind::UvGrid, 64, 32).unwrap();
        let b = StillImage::pattern(PatternKind::UvGrid, 64, 32).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.pixel(0, 0), Some([255, 255, 255, 255]), "border");
        let tl = a.pixel(3, 3).unwrap();
        let br = a.pixel(60, 29).unwrap();
        assert!(
            tl[0] < br[0] && tl[1] < br[1],
            "red/green increase right/down"
        );
        assert!(tl[2] > br[2], "blue marks top-left");
        let c = StillImage::pattern(PatternKind::Checkerboard, 16, 16).unwrap();
        assert_eq!(c.pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(c.pixel(2, 0), Some([0, 0, 0, 255]));
        assert!(StillImage::pattern(PatternKind::White, 0, 4).is_err());
    }

    #[test]
    fn load_round_trips_png_and_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.png");
        let img = StillImage::pattern(PatternKind::UvGrid, 17, 9).unwrap();
        image::save_buffer(&path, img.rgba8(), 17, 9, image::ExtendedColorType::Rgba8).unwrap();
        assert_eq!(StillImage::load(&path).unwrap(), img);

        assert!(matches!(
            StillImage::load(&dir.path().join("missing.png")),
            Err(MediaError::NotFound { .. })
        ));
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"\\x89PNG not really").unwrap();
        assert!(matches!(
            StillImage::load(&bad),
            Err(MediaError::Decode { .. })
        ));
    }

    #[test]
    fn from_rgba8_checks_length() {
        assert!(StillImage::from_rgba8(2, 2, vec![0; 15]).is_err());
        assert!(StillImage::from_rgba8(2, 2, vec![0; 16]).is_ok());
    }
}
