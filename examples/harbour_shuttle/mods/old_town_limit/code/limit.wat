;; Old Town 25: WebAssembly text, compiled by the engine when the game loads.
;; Host functions come from the "waypoint" module (`wp docs mods`).
(module
  (import "waypoint" "player" (func $player (result i32)))
  (import "waypoint" "vehicle_y" (func $y (param i32) (result f32)))
  (import "waypoint" "vehicle_speed_kmh" (func $speed (param i32) (result f32)))
  (import "waypoint" "input_steer" (func $steer (param i32) (result f32)))
  (import "waypoint" "input_throttle" (func $throttle (param i32) (result f32)))
  (import "waypoint" "set_input" (func $set (param i32 f32 f32 f32)))
  (import "waypoint" "log" (func $log (param i32 i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "entering the old town: 25 km/h")
  (global $inside (mut i32) (i32.const 0))

  (func (export "on_tick") (param $dt f32)
    (local $p i32)
    (local.set $p (call $player))
    (if (i32.lt_s (local.get $p) (i32.const 0)) (then (return)))
    (if (f32.gt (call $y (local.get $p)) (f32.const 60))
      (then
        (if (i32.eqz (global.get $inside))
          (then (call $log (i32.const 0) (i32.const 30)) (global.set $inside (i32.const 1))))
        ;; Over the limit: ease off and brake gently, keeping the steering.
        (if (f32.gt (call $speed (local.get $p)) (f32.const 25))
          (then
            (call $set (local.get $p)
              (f32.const 0) (f32.const 0.15) (call $steer (local.get $p))))))
      (else (global.set $inside (i32.const 0))))))
