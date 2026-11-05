;; The smallest useful plugin: the host calls `alloc` for room for the input,
;; then `handle`, which answers with a pointer and length packed in an i64.
;; The input is JSON of Event::PluginInvoked; this one ignores it. It could also
;; import idea.read_file, idea.write_file and idea.run: see ADR-0023.
(module
  (memory (export "memory") 2)
  (global $next (mut i32) (i32.const 4096))
  (func (export "alloc") (param $len i32) (result i32)
    (local $at i32)
    (local.set $at (global.get $next))
    (global.set $next (i32.add (local.get $at) (local.get $len)))
    (block $fits
      (br_if $fits (i32.le_u (global.get $next)
                             (i32.mul (memory.size) (i32.const 65536))))
      (drop (memory.grow
        (i32.add (i32.div_u (local.get $len) (i32.const 65536)) (i32.const 1)))))
    (local.get $at))
  (data (i32.const 16) "{\"commands\":[{\"Insert\":\"hello from a plugin\"}],\"status\":\"Said hello\"}")
  (func (export "handle") (param i32 i32) (result i64)
    ;; 16 << 32 | 69
    (i64.const 68719476805)))
