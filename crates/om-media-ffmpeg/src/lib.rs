// SPDX-License-Identifier: Apache-2.0
//! FFmpeg adapter.
//!
//! FFmpeg is linked **dynamically** and its types never leave this crate:
//! the rest of OpenMapper sees only `om_media_core` types. Release builds
//! must use an LGPL-only FFmpeg build (no `--enable-gpl`/`--enable-nonfree`);
//! [`licence_profile`] reports what the loaded libraries were built with so
//! diagnostics and the release audit can check it (docs/media/ffmpeg.md).

mod video;

#[cfg(feature = "fixtures")]
pub mod fixtures;

pub use video::FfmpegVideo;

use std::sync::OnceLock;

/// Licence profile of the loaded FFmpeg libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenceProfile {
    /// LGPL-2.1-or-later (or LGPL-3 with `--enable-version3`): distributable
    /// with OpenMapper under the dynamic-linking compliance path.
    Lgpl,
    /// Built with `--enable-gpl`: fine for development, not for release.
    Gpl,
    /// Built with `--enable-nonfree`: never redistributable.
    Nonfree,
}

impl std::fmt::Display for LicenceProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Lgpl => "LGPL",
            Self::Gpl => "GPL (development only, not for release)",
            Self::Nonfree => "nonfree (not redistributable)",
        })
    }
}

/// Classifies an FFmpeg `configure` string.
#[must_use]
pub fn classify_configuration(configuration: &str) -> LicenceProfile {
    let has = |flag: &str| configuration.split_whitespace().any(|f| f == flag);
    if has("--enable-nonfree") {
        LicenceProfile::Nonfree
    } else if has("--enable-gpl") {
        LicenceProfile::Gpl
    } else {
        LicenceProfile::Lgpl
    }
}

/// Initialises FFmpeg once. Returns an error message if the libraries fail
/// to initialise.
pub fn init() -> Result<(), String> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        ffmpeg_next::init().map_err(|e| e.to_string())?;
        ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Error);
        Ok(())
    })
    .clone()
}

/// Versions and licence of the loaded FFmpeg libraries.
#[derive(Debug, Clone)]
pub struct FfmpegInfo {
    pub avcodec_version: String,
    pub configuration: String,
    pub licence: LicenceProfile,
}

#[must_use]
pub fn info() -> FfmpegInfo {
    let v = ffmpeg_next::codec::version();
    let configuration = ffmpeg_next::format::configuration().to_owned();
    FfmpegInfo {
        avcodec_version: format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff),
        licence: classify_configuration(&configuration),
        configuration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_licence_flags() {
        assert_eq!(
            classify_configuration("--enable-shared --enable-version3"),
            LicenceProfile::Lgpl
        );
        assert_eq!(
            classify_configuration("--enable-gpl --enable-libx264"),
            LicenceProfile::Gpl
        );
        assert_eq!(
            classify_configuration("--enable-gpl --enable-nonfree"),
            LicenceProfile::Nonfree
        );
        assert_eq!(
            classify_configuration("--disable-gpl-ish"),
            LicenceProfile::Lgpl
        );
    }
}
