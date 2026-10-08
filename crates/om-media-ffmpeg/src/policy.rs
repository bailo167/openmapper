// SPDX-License-Identifier: Apache-2.0
//! Release policy for the FFmpeg libraries OpenMapper ships (D-031).
//!
//! Releases bundle an LGPL FFmpeg built by `tools/ffmpeg/build.sh` from the
//! component list in `tools/ffmpeg/components.txt`. This module states the
//! policy in code so `cargo xtask dist` can check the libraries it is about
//! to ship — what they were *built with* ([`check_configuration`]) and what
//! they *actually contain* ([`check_codecs`]) — and so a test keeps the
//! component list and the policy in agreement.
//!
//! Policy:
//! - LGPL only: no `--enable-gpl`, no `--enable-nonfree`.
//! - No external libraries (`--enable-lib*`): one source archive, one
//!   licence, no GPL or patent-pool codec libraries.
//! - Encoders only for codecs without an active patent licensing programme
//!   ([`ALLOWED_ENCODERS`]): publishing uses MPEG-2 and FFV1 (sink.rs).
//! - No decoders for formats whose patent pools have no royalty-free tier
//!   ([`DENIED_DECODERS`]); users who need them replace the libraries
//!   (docs/media/ffmpeg.md).

use std::ffi::CStr;

use ffmpeg_next::ffi;

use crate::{FfmpegInfo, LicenceProfile};

/// `--extra-version` suffix of the libraries built by `tools/ffmpeg/build.sh`;
/// appears in [`version_info`], so the packaging smoke test can tell the
/// bundled libraries from a system installation.
pub const VERSION_MARKER: &str = "openmapper";

/// Encoders a release build may contain.
pub const ALLOWED_ENCODERS: &[&str] = &["ffv1", "mpeg2video", "rawvideo", "pcm_s16le", "pcm_f32le"];

/// Decoders a release build must not contain.
pub const DENIED_DECODERS: &[&str] = &["hevc", "vvc", "evc", "vc1", "wmv3"];

/// Names of the registered encoders and decoders of the loaded libraries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Codecs {
    pub encoders: Vec<String>,
    pub decoders: Vec<String>,
}

/// Enumerates the codecs registered in the loaded libavcodec.
#[must_use]
#[allow(unsafe_code)]
pub fn codecs() -> Codecs {
    let mut out = Codecs::default();
    if crate::init().is_err() {
        return out;
    }
    let mut opaque: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: av_codec_iterate walks libavcodec's static codec table; each
    // returned pointer is to a static AVCodec with a static NUL-terminated
    // name, valid for the life of the process. The opaque cursor is only
    // handed back to the same function.
    unsafe {
        loop {
            let codec = ffi::av_codec_iterate(&raw mut opaque);
            if codec.is_null() {
                break;
            }
            if (*codec).name.is_null() {
                continue;
            }
            let name = CStr::from_ptr((*codec).name).to_string_lossy().into_owned();
            if ffi::av_codec_is_encoder(codec) != 0 {
                out.encoders.push(name);
            } else if ffi::av_codec_is_decoder(codec) != 0 {
                out.decoders.push(name);
            }
        }
    }
    out.encoders.sort();
    out.decoders.sort();
    out
}

/// The libraries' version string (e.g. `n9.0.2-openmapper` for a bundled
/// build, `n9.0.2` or a distribution's own suffix otherwise).
#[must_use]
#[allow(unsafe_code)]
pub fn version_info() -> String {
    // SAFETY: av_version_info returns a pointer to a static NUL-terminated
    // string (never null).
    unsafe {
        let p = ffi::av_version_info();
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// Checks the `configure` line the libraries were built with.
pub fn check_configuration(info: &FfmpegInfo) -> Result<(), Vec<String>> {
    let mut v = check_flags(info.configuration.split_whitespace());
    // The profile is derived from the same flags; this guards against a
    // configuration string that was truncated or rewritten.
    if info.licence != LicenceProfile::Lgpl && v.is_empty() {
        v.push(format!("licence profile is {}", info.licence));
    }
    if v.is_empty() { Ok(()) } else { Err(v) }
}

/// Checks the registered codecs against the encoder allow-list and the
/// decoder deny-list.
pub fn check_codecs(codecs: &Codecs) -> Result<(), Vec<String>> {
    let mut v: Vec<String> = codecs
        .encoders
        .iter()
        .filter(|e| !ALLOWED_ENCODERS.contains(&e.as_str()))
        .map(|e| format!("encoder {e} is not in the release allow-list"))
        .collect();
    v.extend(
        codecs
            .decoders
            .iter()
            .filter(|d| DENIED_DECODERS.contains(&d.as_str()))
            .map(|d| format!("decoder {d} is excluded from releases")),
    );
    if v.is_empty() { Ok(()) } else { Err(v) }
}

/// Checks a list of `configure` flags (a configuration string or the
/// recipe's `components.txt`): licence flags, external libraries, and the
/// encoder/decoder lists.
pub fn check_flags<'a>(flags: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut v = Vec::new();
    for f in flags {
        if f == "--enable-gpl" {
            v.push("built with --enable-gpl (GPL)".to_owned());
        } else if f == "--enable-nonfree" {
            v.push("built with --enable-nonfree".to_owned());
        } else if let Some(lib) = f.strip_prefix("--enable-lib") {
            v.push(format!("external library lib{lib}"));
        } else if let Some(list) = f.strip_prefix("--enable-encoder=") {
            for e in list.split(',').filter(|e| !ALLOWED_ENCODERS.contains(e)) {
                v.push(format!("encoder {e} is not in the release allow-list"));
            }
        } else if let Some(list) = f.strip_prefix("--enable-decoder=") {
            for d in list.split(',').filter(|d| DENIED_DECODERS.contains(d)) {
                v.push(format!("decoder {d} is excluded from releases"));
            }
        }
    }
    v
}

/// Parses `tools/ffmpeg/components.txt` (one flag per line, `#` comments).
#[must_use]
pub fn component_flags(text: &str) -> Vec<&str> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPONENTS: &str = include_str!("../../../tools/ffmpeg/components.txt");

    fn info(configuration: &str) -> FfmpegInfo {
        FfmpegInfo {
            avcodec_version: "62.0.0".into(),
            version_info: "test".into(),
            licence: crate::classify_configuration(configuration),
            configuration: configuration.to_owned(),
        }
    }

    #[test]
    fn recipe_components_satisfy_the_policy() {
        let flags = component_flags(COMPONENTS);
        assert!(
            flags.len() > 50,
            "components.txt parsed to {} flags",
            flags.len()
        );
        assert!(flags.iter().all(|f| f.starts_with("--")), "{flags:?}");
        assert_eq!(check_flags(flags.iter().copied()), Vec::<String>::new());
        // Publishing (sink.rs) needs these encoders.
        for needed in ["--enable-encoder=ffv1", "--enable-encoder=mpeg2video"] {
            assert!(flags.contains(&needed), "{needed} missing");
        }
        assert!(flags.contains(&"--enable-decoder=h264"));
        assert!(!flags.iter().any(|f| f.contains("hevc")));
    }

    #[test]
    fn configuration_checks() {
        assert_eq!(
            check_configuration(&info(
                "--disable-everything --enable-shared --enable-decoder=h264"
            )),
            Ok(())
        );
        let e = check_configuration(&info(
            "--enable-gpl --enable-libx264 --enable-encoder=libx264",
        ))
        .unwrap_err();
        assert!(e.iter().any(|m| m.contains("--enable-gpl")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("libx264")), "{e:?}");
        let e = check_configuration(&info("--enable-nonfree --enable-libfdk-aac")).unwrap_err();
        assert!(e[0].contains("nonfree"), "{e:?}");
        assert!(e[1].contains("libfdk-aac"), "{e:?}");
        let e = check_configuration(&info("--enable-decoder=h264,hevc --enable-encoder=aac"))
            .unwrap_err();
        assert_eq!(
            e,
            vec![
                "decoder hevc is excluded from releases".to_owned(),
                "encoder aac is not in the release allow-list".to_owned(),
            ]
        );
    }

    #[test]
    fn codec_checks() {
        let ok = Codecs {
            encoders: vec!["ffv1".into(), "mpeg2video".into(), "pcm_s16le".into()],
            decoders: vec!["h264".into(), "prores".into()],
        };
        assert_eq!(check_codecs(&ok), Ok(()));
        let bad = Codecs {
            encoders: vec!["ffv1".into(), "libx264".into()],
            decoders: vec!["h264".into(), "hevc".into()],
        };
        assert_eq!(
            check_codecs(&bad),
            Err(vec![
                "encoder libx264 is not in the release allow-list".to_owned(),
                "decoder hevc is excluded from releases".to_owned(),
            ])
        );
    }

    #[test]
    fn enumerates_loaded_codecs() {
        // Any FFmpeg build in CI/dev has the H.264 decoder and reports a
        // version string; the policy outcome depends on the build.
        let c = codecs();
        assert!(c.decoders.iter().any(|d| d == "h264"), "{:?}", c.decoders);
        assert!(!c.encoders.is_empty());
        assert!(!version_info().is_empty());
    }
}
