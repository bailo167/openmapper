// SPDX-License-Identifier: Apache-2.0
//! Camera-assisted calibration, synthetically: a projector shows the
//! Gray-code sequence on a flat screen; a simulated camera photographs it
//! in perspective with uneven surface reflectance, ambient light and
//! sensor noise. Decoding must recover the camera ↔ projector mapping.
#![allow(
    clippy::unwrap_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use om_calibration::Homography;
use om_calibration::structured_light::{Gray, decode, render, sequence};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

const PW: u32 = 320;
const PH: u32 = 200;
const CW: u32 = 400;
const CH: u32 = 300;

/// Camera pixel → projector pixel (the truth the decoder must find).
fn truth() -> Homography {
    let cam = [(40.0, 30.0), (370.0, 55.0), (350.0, 280.0), (25.0, 250.0)];
    let proj = [(0.0, 0.0), (320.0, 0.0), (320.0, 200.0), (0.0, 200.0)];
    Homography::fit(&cam, &proj).unwrap()
}

fn capture(pattern: &Gray, h: &Homography, rng: &mut Rng) -> Gray {
    let mut pixels = Vec::with_capacity((CW * CH) as usize);
    for y in 0..CH {
        for x in 0..CW {
            let c = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            // Uneven reflectance (a dark stripe and a gradient).
            let albedo = 0.35
                + 0.5 * (f64::from(x) / f64::from(CW)) * if (y / 40) % 3 == 0 { 0.4 } else { 1.0 };
            let ambient = 25.0;
            let lit = h.apply(c).and_then(|(u, v)| {
                (u >= 0.0 && v >= 0.0 && u < f64::from(PW) && v < f64::from(PH))
                    .then(|| f64::from(pattern.get(u as u32, v as u32)))
            });
            let value = ambient + albedo * lit.unwrap_or(0.0) + (rng.next() - 0.5) * 8.0;
            pixels.push(value.clamp(0.0, 255.0) as u8);
        }
    }
    Gray {
        width: CW,
        height: CH,
        pixels,
    }
}

#[test]
fn decoded_correspondences_recover_the_camera_to_projector_map() {
    let h = truth();
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let captures: Vec<Gray> = sequence(PW, PH)
        .into_iter()
        .map(|k| capture(&render(k, PW, PH), &h, &mut rng))
        .collect();
    let decoded = decode(&captures, PW, PH, 12).unwrap();

    // Decoding accuracy over camera pixels that see the projection.
    let (mut inside, mut good, mut decoded_outside) = (0, 0, 0);
    for y in 0..CH {
        for x in 0..CW {
            let c = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let t = h.apply(c).unwrap();
            let in_view =
                t.0 >= 1.0 && t.1 >= 1.0 && t.0 < f64::from(PW) - 1.0 && t.1 < f64::from(PH) - 1.0;
            let got = decoded.map[(y * CW + x) as usize];
            if in_view {
                inside += 1;
                if let Some((u, v)) = got
                    && (f64::from(u) + 0.5 - t.0).abs() <= 1.0
                    && (f64::from(v) + 0.5 - t.1).abs() <= 1.0
                {
                    good += 1;
                }
            } else if got.is_some()
                && (t.0 < -1.0
                    || t.1 < -1.0
                    || t.0 > f64::from(PW) + 1.0
                    || t.1 > f64::from(PH) + 1.0)
            {
                decoded_outside += 1;
            }
        }
    }
    let rate = f64::from(good) / f64::from(inside);
    assert!(rate > 0.95, "only {:.1}% decoded within 1 px", rate * 100.0);
    assert_eq!(decoded_outside, 0, "pixels outside the projection decoded");

    // The fitted map agrees with the truth to well under a pixel.
    let (cam, proj): (Vec<_>, Vec<_>) = decoded.pairs().into_iter().unzip();
    let fit = Homography::fit_robust(&cam, &proj, 2.0).unwrap();
    let mut worst = 0.0f64;
    for y in (0..CH).step_by(10) {
        for x in (0..CW).step_by(10) {
            let c = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let (a, b) = (fit.apply(c).unwrap(), h.apply(c).unwrap());
            if b.0 >= 0.0 && b.1 >= 0.0 && b.0 <= f64::from(PW) && b.1 <= f64::from(PH) {
                worst = worst.max((a.0 - b.0).hypot(a.1 - b.1));
            }
        }
    }
    assert!(worst < 0.75, "fitted map is off by {worst} px");
}
