// SPDX-License-Identifier: Apache-2.0

use super::*;
use om_project::{Cue, CueValue, Keyframe, Modulator, Surface, Timeline, Track};
use om_time::Rate;
use om_types::{Finite, ModulatorId, SurfaceId};

fn s(n: u128) -> SurfaceId {
    SurfaceId::from_u128(n)
}

fn secs(x: f64) -> RationalTime {
    RationalTime::new((x * 1000.0).round() as i128, 1000).unwrap()
}

fn fin(x: f64) -> Finite {
    Finite::new(x).unwrap()
}

fn opacity(id: SurfaceId) -> ParamId {
    ParamId::SurfaceOpacity(id)
}

fn project() -> Project {
    let mut p = Project::new("show");
    p.surfaces.push(Surface::new(s(1), "a"));
    p.surfaces.push(Surface::new(s(2), "b"));
    p.show.cues = vec![
        Cue {
            id: CueId::from_u128(1),
            name: "dim".into(),
            fade: fin(2.0),
            values: vec![CueValue {
                param: opacity(s(1)),
                value: ParamValue::Float(0.0),
            }],
        },
        Cue {
            id: CueId::from_u128(2),
            name: "b off".into(),
            fade: fin(0.0),
            values: vec![CueValue {
                param: ParamId::SurfaceEnabled(s(2)),
                value: ParamValue::Bool(false),
            }],
        },
        Cue {
            id: CueId::from_u128(3),
            name: "up".into(),
            fade: fin(4.0),
            values: vec![CueValue {
                param: opacity(s(1)),
                value: ParamValue::Float(1.0),
            }],
        },
    ];
    p
}

fn get(o: &Overrides, id: &ParamId) -> Option<f64> {
    o.get(id).map(|v| v.as_f64())
}

#[test]
fn cue_fades_are_exact_and_tracking() {
    let p = project();
    let mut st = ShowState::new();
    let none = AudioLevels::default();
    assert!(st.evaluate(&p, secs(0.0), none).is_empty());
    assert_eq!(st.go_next(&p, secs(10.0)), Some(CueId::from_u128(1)));
    assert_eq!(
        get(&st.evaluate(&p, secs(10.0), none), &opacity(s(1))),
        Some(1.0)
    );
    assert_eq!(
        get(&st.evaluate(&p, secs(11.0), none), &opacity(s(1))),
        Some(0.5)
    );
    assert_eq!(
        get(&st.evaluate(&p, secs(11.5), none), &opacity(s(1))),
        Some(0.25)
    );
    assert_eq!(
        get(&st.evaluate(&p, secs(13.0), none), &opacity(s(1))),
        Some(0.0)
    );
    // Next cue changes something else; the first cue's value tracks.
    st.go_next(&p, secs(20.0));
    let o = st.evaluate(&p, secs(20.0), none);
    assert_eq!(get(&o, &opacity(s(1))), Some(0.0), "tracked");
    assert_eq!(
        o.get(&ParamId::SurfaceEnabled(s(2))),
        Some(&ParamValue::Bool(false))
    );
}

#[test]
fn interrupted_fade_continues_from_where_it_was() {
    let p = project();
    let mut st = ShowState::new();
    let none = AudioLevels::default();
    st.go(&p, CueId::from_u128(1), secs(0.0)); // 1 -> 0 over 2 s
    assert_eq!(
        get(&st.evaluate(&p, secs(1.0), none), &opacity(s(1))),
        Some(0.5)
    );
    st.go(&p, CueId::from_u128(3), secs(1.0)); // from 0.5 -> 1 over 4 s
    assert_eq!(
        get(&st.evaluate(&p, secs(1.0), none), &opacity(s(1))),
        Some(0.5)
    );
    assert_eq!(
        get(&st.evaluate(&p, secs(3.0), none), &opacity(s(1))),
        Some(0.75)
    );
    assert_eq!(
        get(&st.evaluate(&p, secs(5.0), none), &opacity(s(1))),
        Some(1.0)
    );
}

#[test]
fn release_fades_back_to_the_document() {
    let p = project();
    let mut st = ShowState::new();
    let none = AudioLevels::default();
    st.go(&p, CueId::from_u128(1), secs(0.0));
    st.evaluate(&p, secs(5.0), none);
    st.release(&p, secs(5.0), 1.0);
    assert_eq!(
        get(&st.evaluate(&p, secs(5.5), none), &opacity(s(1))),
        Some(0.5)
    );
    let done = st.evaluate(&p, secs(6.0), none);
    assert!(done.is_empty(), "document values again: {done:?}");
    assert_eq!(st.current_cue(), None);
}

fn timeline(looping: bool, ease: Ease) -> Timeline {
    let k = |t: f64, v: f64| Keyframe {
        at: secs(t),
        value: ParamValue::Float(v),
        ease,
    };
    Timeline {
        id: TimelineId::from_u128(1),
        name: "t".into(),
        duration: secs(4.0),
        looping,
        tracks: vec![Track {
            param: opacity(s(2)),
            keys: vec![k(3.0, 0.0), k(1.0, 1.0)],
        }],
        markers: vec![],
    }
}

#[test]
fn timelines_interpolate_exactly_and_loop() {
    let mut p = project();
    p.show.timelines = vec![timeline(true, Ease::Linear)];
    let mut st = ShowState::new();
    let none = AudioLevels::default();
    let id = TimelineId::from_u128(1);
    assert!(
        st.evaluate(&p, secs(0.0), none).is_empty(),
        "inactive until played"
    );
    st.play_timeline(id, secs(100.0));
    let at = |st: &mut ShowState, t: f64| get(&st.evaluate(&p, secs(t), none), &opacity(s(2)));
    assert_eq!(at(&mut st, 100.5), Some(1.0), "before first key holds it");
    assert_eq!(at(&mut st, 102.0), Some(0.5));
    assert_eq!(at(&mut st, 103.5), Some(0.0), "after last key holds it");
    assert_eq!(at(&mut st, 106.0), Some(0.5), "looped (4 s)");
    // Exact frame stepping at 29.97 fps: frame 30 of the second pass.
    let t = secs(104.0)
        .checked_add(RationalTime::from_frame(30, Rate::FPS_29_97).unwrap())
        .unwrap();
    let pos = st.timeline_position(&p.show.timelines[0], t).unwrap();
    assert_eq!(pos, RationalTime::from_frame(30, Rate::FPS_29_97).unwrap());
    st.pause_timeline(id, secs(106.0));
    assert_eq!(at(&mut st, 200.0), Some(0.5), "paused holds");
    st.seek_timeline(id, secs(3.0), secs(200.0));
    assert_eq!(at(&mut st, 200.0), Some(0.0));
    st.stop_timeline(id);
    assert_eq!(at(&mut st, 200.0), None);
}

#[test]
fn eases() {
    for (ease, expected) in [
        (Ease::Linear, 0.75),
        (Ease::Step, 1.0),
        (Ease::Smooth, 0.84375),
    ] {
        let tl = timeline(false, ease);
        // Between 1 s (1.0) and 3 s (0.0), at 1.5 s: alpha = 0.25.
        let v = track_value(&tl.tracks[0], secs(1.5)).unwrap().as_f64();
        assert!((v - expected).abs() < 1e-12, "{ease:?}: {v} vs {expected}");
    }
}

#[test]
fn modulators_and_precedence() {
    let mut p = project();
    p.show.timelines = vec![timeline(true, Ease::Linear)];
    p.show.modulators = vec![
        Modulator {
            id: ModulatorId::from_u128(1),
            name: "lfo".into(),
            enabled: true,
            param: opacity(s(2)),
            source: ModSource::Lfo {
                shape: LfoShape::Square,
                rate: fin(1.0),
                phase: fin(0.0),
            },
            depth: fin(0.5),
            offset: fin(0.25),
        },
        Modulator {
            id: ModulatorId::from_u128(2),
            name: "audio".into(),
            enabled: true,
            param: ParamId::MasterOpacity,
            source: ModSource::Audio {
                band: AudioBand::Low,
                gain: fin(2.0),
            },
            depth: fin(1.0),
            offset: fin(0.0),
        },
    ];
    let mut st = ShowState::new();
    st.play_timeline(TimelineId::from_u128(1), secs(0.0));
    let audio = AudioLevels {
        low: 0.3,
        ..Default::default()
    };
    let o = st.evaluate(&p, secs(0.25), audio);
    assert_eq!(
        get(&o, &opacity(s(2))),
        Some(0.75),
        "modulator beats timeline"
    );
    let master = get(&o, &ParamId::MasterOpacity).unwrap();
    assert!((master - 0.6).abs() < 1e-6, "audio 0.3 × gain 2: {master}");
    let o = st.evaluate(&p, secs(0.75), audio);
    assert_eq!(get(&o, &opacity(s(2))), Some(0.25));
    assert_eq!(lfo(LfoShape::Sine, 0.25), 1.0);
    assert_eq!(lfo(LfoShape::Triangle, 0.5), 1.0);
    assert_eq!(lfo(LfoShape::Saw, 1.75), 0.75);
}

#[test]
fn deleted_targets_are_ignored() {
    let mut p = project();
    let mut st = ShowState::new();
    st.go(&p, CueId::from_u128(1), secs(0.0));
    p.surfaces.retain(|x| x.id != s(1));
    assert!(
        st.evaluate(&p, secs(1.0), AudioLevels::default())
            .is_empty()
    );
}
