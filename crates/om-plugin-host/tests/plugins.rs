// SPDX-License-Identifier: Apache-2.0
//! Plugin host behaviour with good and hostile plugins: correct output,
//! capability enforcement, and crashes, infinite loops and memory hogs that
//! are stopped without affecting the caller.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use om_plugin_api::Capability;
use om_plugin_host::runner::{Job, PluginRunner, RunnerState};
use om_plugin_host::{Limits, PluginError, PluginHost};

const INVERT: &str = include_str!("plugins/invert.wat");

fn host() -> PluginHost {
    PluginHost::with_limits(Limits {
        memory: 16 << 20,
        fuel: 500_000_000,
        deadline: Duration::from_millis(200),
    })
    .unwrap()
}

/// A plugin with the standard allocator and manifest and a custom
/// `om_process` body (`$in $w $h $params $np $out` are in scope).
fn plugin(imports: &str, manifest: &str, process: &str) -> String {
    format!(
        r#"(module
  {imports}
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (data (i32.const 16) "{json}")
  (func (export "om_abi_version") (result i32) (i32.const 1))
  (func (export "om_manifest") (result i64)
    (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {len})))
  (func $alloc (export "om_alloc") (param $size i32) (result i32)
    (local $ptr i32) (local $end i32) (local $have i32)
    (local.set $ptr (global.get $heap))
    (local.set $end (i32.add (local.get $ptr) (i32.and (i32.add (local.get $size) (i32.const 15)) (i32.const -16))))
    (local.set $have (i32.mul (memory.size) (i32.const 65536)))
    (if (i32.gt_u (local.get $end) (local.get $have))
      (then (if (i32.eq (memory.grow (i32.add (i32.div_u (i32.sub (local.get $end) (local.get $have)) (i32.const 65536)) (i32.const 1))) (i32.const -1))
        (then (return (i32.const 0))))))
    (global.set $heap (local.get $end))
    (local.get $ptr))
  (func (export "om_process")
    (param $in i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $out i32)
    (result i32)
    {process})
)"#,
        json = manifest.replace('"', "\\\""),
        len = manifest.len(),
    )
}

const BASIC: &str = r#"{"name":"Test","abi":1}"#;

fn frame(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            let v = i as u8;
            [v, v.wrapping_mul(3), 200, 128]
        })
        .collect()
}

#[test]
fn good_plugin_processes_frames_and_declares_capabilities() {
    let h = host();
    let p = h.load(INVERT.as_bytes()).unwrap();
    assert_eq!(p.manifest.name, "Invert");
    assert_eq!(p.manifest.params.len(), 1);
    assert_eq!(p.imports, vec![Capability::Log, Capability::Time]);
    let mut inst = h.instantiate(&p).unwrap();
    let input = frame(32, 8);
    let out = inst.process(&input, 32, 8, &[1.0], 12.5).unwrap();
    for (a, b) in input.chunks(4).zip(out.chunks(4)) {
        assert_eq!([255 - a[0], 255 - a[1], 255 - a[2], a[3]], b);
    }
    // Parameter below 0.5 copies; buffers are reused across sizes.
    assert_eq!(inst.process(&input, 32, 8, &[0.2], 0.0).unwrap(), input);
    let small = frame(3, 2);
    assert_eq!(inst.process(&small, 3, 2, &[0.0], 0.0).unwrap(), small);
    assert_eq!(inst.take_log(), vec!["processing"; 3]);
    assert_eq!(
        inst.process(&small, 4, 2, &[], 0.0),
        Err(PluginError::BadFrame)
    );
}

#[test]
fn hostile_imports_and_bad_modules_are_refused() {
    let h = host();
    assert!(matches!(
        h.load(b"\0asm garbage"),
        Err(PluginError::Invalid(_))
    ));
    assert!(matches!(h.load(b"(module"), Err(PluginError::Invalid(_))));
    let wasi = plugin(
        r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#,
        BASIC,
        "(i32.const 0)",
    );
    assert_eq!(
        h.load(wasi.as_bytes()).unwrap_err(),
        PluginError::ForbiddenImport("wasi_snapshot_preview1::fd_write".into())
    );
    let sneaky = plugin(
        r#"(import "openmapper" "open_file" (func))"#,
        BASIC,
        "(i32.const 0)",
    );
    assert!(matches!(
        h.load(sneaky.as_bytes()),
        Err(PluginError::ForbiddenImport(_))
    ));
    let undeclared = plugin(
        r#"(import "openmapper" "log" (func (param i32 i32)))"#,
        BASIC,
        "(i32.const 0)",
    );
    assert_eq!(
        h.load(undeclared.as_bytes()).unwrap_err(),
        PluginError::UndeclaredCapability("log".into())
    );
    let wrong_abi = plugin("", r#"{"name":"Test","abi":2}"#, "(i32.const 0)");
    assert!(matches!(
        h.load(wrong_abi.as_bytes()),
        Err(PluginError::Manifest(_))
    ));
    let no_process = INVERT.replace("(export \"om_process\")", "");
    assert_eq!(
        h.load(no_process.as_bytes()).unwrap_err(),
        PluginError::MissingExport("om_process".into())
    );
}

#[test]
fn crashes_loops_and_memory_hogs_are_contained() {
    let h = host();
    let crash = h
        .load(plugin("", BASIC, "(unreachable)").as_bytes())
        .unwrap();
    let mut inst = h.instantiate(&crash).unwrap();
    let f = frame(4, 4);
    assert!(matches!(
        inst.process(&f, 4, 4, &[], 0.0),
        Err(PluginError::Trap(_))
    ));
    assert!(inst.is_faulted());
    assert!(
        inst.process(&f, 4, 4, &[], 0.0).is_err(),
        "a faulted instance stays failed"
    );

    // An infinite loop is stopped by the instruction budget or the deadline.
    let spin = h
        .load(plugin("", BASIC, "(loop $l (br $l)) (i32.const 0)").as_bytes())
        .unwrap();
    let mut inst = h.instantiate(&spin).unwrap();
    let t = Instant::now();
    let err = inst.process(&f, 4, 4, &[], 0.0).unwrap_err();
    assert!(
        matches!(err, PluginError::Timeout | PluginError::OutOfFuel),
        "{err:?}"
    );
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "stopped in {:?}",
        t.elapsed()
    );

    // Growing memory without bound hits the cap.
    let hog = h
        .load(
            plugin(
                "",
                BASIC,
                "(loop $l (br_if $l (i32.ne (memory.grow (i32.const 16)) (i32.const -1)))) (unreachable)",
            )
            .as_bytes(),
        )
        .unwrap();
    let mut inst = h.instantiate(&hog).unwrap();
    assert!(inst.process(&f, 4, 4, &[], 0.0).is_err());

    // A non-zero result is an error but the instance remains usable.
    let fails = h
        .load(plugin("", BASIC, "(i32.const 7)").as_bytes())
        .unwrap();
    let mut inst = h.instantiate(&fails).unwrap();
    assert_eq!(
        inst.process(&f, 4, 4, &[], 0.0),
        Err(PluginError::Failed(7))
    );
    assert!(!inst.is_faulted());
}

#[test]
fn runner_never_blocks_callers_and_disables_a_hanging_plugin() {
    let h = Arc::new(host());
    let spin = h
        .load(plugin("", BASIC, "(loop $l (br $l)) (i32.const 0)").as_bytes())
        .unwrap();
    let mut runner = PluginRunner::spawn(Arc::clone(&h), spin);
    let f = Arc::new(frame(16, 16));
    let t = Instant::now();
    for _ in 0..100 {
        runner.submit(Job {
            rgba: Arc::clone(&f),
            width: 16,
            height: 16,
            params: Vec::new(),
            time: 0.0,
        });
    }
    assert!(
        t.elapsed() < Duration::from_millis(50),
        "submit never waits on the plugin"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(runner.state(), RunnerState::Disabled { .. }) {
        assert!(Instant::now() < deadline, "still {:?}", runner.state());
        runner.submit(Job {
            rgba: Arc::clone(&f),
            width: 16,
            height: 16,
            params: Vec::new(),
            time: 0.0,
        });
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(runner.output().is_none());
    let t = Instant::now();
    drop(runner);
    assert!(t.elapsed() < Duration::from_secs(1), "drop is prompt");

    // A good plugin through the runner.
    let good = h.load(INVERT.as_bytes()).unwrap();
    let mut runner = PluginRunner::spawn(h, good);
    let seq = runner.submit(Job {
        rgba: Arc::clone(&f),
        width: 16,
        height: 16,
        params: vec![1.0],
        time: 1.0,
    });
    let out = runner.wait_for(seq, Duration::from_secs(5)).unwrap();
    assert_eq!(out.rgba[0], 255 - f[0]);
    assert_eq!(runner.state(), RunnerState::Running);
    assert_eq!(runner.take_log(), vec!["processing"]);
}
