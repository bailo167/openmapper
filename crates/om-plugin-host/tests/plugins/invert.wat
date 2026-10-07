;; SPDX-License-Identifier: Apache-2.0
;; ABI v1 fixture: inverts RGB when parameter "amount" >= 0.5 (alpha kept).
;; Uses the log and time capabilities. This file must keep loading in every
;; future OpenMapper with ABI v1 (compatibility fixture).
(module
  (import "openmapper" "log" (func $log (param i32 i32)))
  (import "openmapper" "time" (func $time (result f64)))
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 4096))
  (data (i32.const 16) "{\"name\":\"Invert\",\"version\":\"1.0.0\",\"abi\":1,\"capabilities\":[\"log\",\"time\"],\"params\":[{\"name\":\"amount\",\"min\":0,\"max\":1,\"default\":1}]}")
  (data (i32.const 1024) "processing")
  (global $time_seen (mut f64) (f64.const 0))
  (func (export "om_abi_version") (result i32) (i32.const 1))
  (func (export "om_manifest") (result i64)
    ;; ptr 16, len = length of the JSON above
    (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const 130)))
  (func (export "om_alloc") (param $size i32) (result i32)
    (local $ptr i32) (local $end i32) (local $have i32)
    (local.set $ptr (global.get $heap))
    (local.set $end (i32.add (local.get $ptr) (i32.and (i32.add (local.get $size) (i32.const 15)) (i32.const -16))))
    (local.set $have (i32.mul (memory.size) (i32.const 65536)))
    (if (i32.gt_u (local.get $end) (local.get $have))
      (then
        (if (i32.eq
              (memory.grow (i32.add (i32.div_u (i32.sub (local.get $end) (local.get $have)) (i32.const 65536)) (i32.const 1)))
              (i32.const -1))
          (then (return (i32.const 0))))))
    (global.set $heap (local.get $end))
    (local.get $ptr))
  (func (export "om_process")
    (param $in i32) (param $w i32) (param $h i32) (param $params i32) (param $np i32) (param $out i32)
    (result i32)
    (local $i i32) (local $n i32) (local $invert i32)
    (call $log (i32.const 1024) (i32.const 10))
    (global.set $time_seen (call $time))
    (local.set $n (i32.mul (i32.mul (local.get $w) (local.get $h)) (i32.const 4)))
    (local.set $invert (i32.const 1))
    (if (i32.gt_s (local.get $np) (i32.const 0))
      (then (local.set $invert (f32.ge (f32.load (local.get $params)) (f32.const 0.5)))))
    (block $done
      (loop $px
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (if (local.get $invert)
          (then
            (i32.store8 (i32.add (local.get $out) (local.get $i))
              (i32.sub (i32.const 255) (i32.load8_u (i32.add (local.get $in) (local.get $i)))))
            (i32.store8 (i32.add (local.get $out) (i32.add (local.get $i) (i32.const 1)))
              (i32.sub (i32.const 255) (i32.load8_u (i32.add (local.get $in) (i32.add (local.get $i) (i32.const 1))))))
            (i32.store8 (i32.add (local.get $out) (i32.add (local.get $i) (i32.const 2)))
              (i32.sub (i32.const 255) (i32.load8_u (i32.add (local.get $in) (i32.add (local.get $i) (i32.const 2)))))))
          (else
            (i32.store (i32.add (local.get $out) (local.get $i))
              (i32.load (i32.add (local.get $in) (local.get $i))))))
        (i32.store8 (i32.add (local.get $out) (i32.add (local.get $i) (i32.const 3)))
          (i32.load8_u (i32.add (local.get $in) (i32.add (local.get $i) (i32.const 3)))))
        (local.set $i (i32.add (local.get $i) (i32.const 4)))
        (br $px)))
    (i32.const 0))
)
