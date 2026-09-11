(component
 (import "wasi:io/error@0.2.12" (instance $error
  (export "error" (type (sub resource)))))
 (alias export $error "error" (type $error-type))
 (import "wasi:io/streams@0.2.12" (instance $streams
  (alias outer 1 $error-type (type $outer-error))
  (export "error" (type $error (eq $outer-error)))
  (export "output-stream" (type $output (sub resource)))
  (type $stream-error-def (variant (case "last-operation-failed" (own $error)) (case "closed")))
  (export "stream-error" (type $stream-error (eq $stream-error-def)))
  (export "[method]output-stream.check-write" (func (param "self" (borrow $output)) (result (result u64 (error $stream-error)))))
  (export "[method]output-stream.blocking-write-and-flush" (func (param "self" (borrow $output)) (param "contents" (list u8)) (result (result (error $stream-error)))))))
 (alias export $streams "output-stream" (type $output))
 (import "wasi:cli/stdout@0.2.12" (instance $stdout
  (alias outer 1 $output (type $outer-output))
  (export "output-stream" (type $output (eq $outer-output)))
  (export "get-stdout" (func (result (own $output))))))
 (import "wasi:io/poll@0.2.12" (instance $poll
  (export "pollable" (type $pollable (sub resource)))
  (export "[method]pollable.ready" (func (param "self" (borrow $pollable)) (result bool)))))
 (alias export $poll "pollable" (type $pollable))
 (import "wasi:clocks/monotonic-clock@0.2.12" (instance $clock
  (alias outer 1 $pollable (type $outer-pollable))
  (export "pollable" (type $pollable (eq $outer-pollable)))
  (export "now" (func (result u64)))
  (export "subscribe-instant" (func (param "when" u64) (result (own $pollable))))))
 (import "wasi:random/random@0.2.12" (instance $random
  (export "get-random-u64" (func (result u64)))))
 (import "wasi:cli/environment@0.2.12" (instance $environment
  (export "get-arguments" (func (result (list string))))
  (export "get-environment" (func (result (list (tuple string string)))))))
 (import "wasi:filesystem/types@0.2.12" (instance $fs
  (export "descriptor" (type $descriptor (sub resource)))
  (type $error-def (enum "access" "would-block" "already" "bad-descriptor" "busy" "deadlock" "quota" "exist" "file-too-large" "illegal-byte-sequence" "in-progress" "interrupted" "invalid" "io" "is-directory" "loop" "too-many-links" "message-size" "name-too-long" "no-device" "no-entry" "no-lock" "insufficient-memory" "insufficient-space" "not-directory" "not-empty" "not-recoverable" "unsupported" "no-tty" "no-such-device" "overflow" "not-permitted" "pipe" "read-only" "invalid-seek" "text-file-busy" "cross-device"))
  (export "error-code" (type $error (eq $error-def)))
  (type $descriptor-flags-def (flags "read" "write" "file-integrity-sync" "data-integrity-sync" "requested-write-sync" "mutate-directory"))
  (export "descriptor-flags" (type $descriptor-flags (eq $descriptor-flags-def)))
  (type $path-flags-def (flags "symlink-follow"))
  (export "path-flags" (type $path-flags (eq $path-flags-def)))
  (type $open-flags-def (flags "create" "directory" "exclusive" "truncate"))
  (export "open-flags" (type $open-flags (eq $open-flags-def)))
  (export "[method]descriptor.open-at" (func (param "self" (borrow $descriptor)) (param "path-flags" $path-flags) (param "path" string) (param "open-flags" $open-flags) (param "flags" $descriptor-flags) (result (result (own $descriptor) (error $error)))))
  (export "[method]descriptor.read" (func (param "self" (borrow $descriptor)) (param "length" u64) (param "offset" u64) (result (result (tuple (list u8) bool) (error $error)))))
  (export "[method]descriptor.write" (func (param "self" (borrow $descriptor)) (param "buffer" (list u8)) (param "offset" u64) (result (result u64 (error $error)))))))
 (alias export $fs "descriptor" (type $descriptor))
 (import "wasi:filesystem/preopens@0.2.12" (instance $preopens
  (alias outer 1 $descriptor (type $outer-descriptor))
  (export "descriptor" (type $descriptor (eq $outer-descriptor)))
  (export "get-directories" (func (result (list (tuple (own $descriptor) string)))))))
 (core module $memory
  (memory (export "memory") 2)
  (global $next (mut i32) (i32.const 4096))
  (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $ptr i32)
   global.get $next local.set $ptr
   global.get $next local.get 3 i32.add i32.const 7 i32.add i32.const -8 i32.and global.set $next
   local.get $ptr))
 (core instance $memory (instantiate $memory))
 (core func $directories (canon lower (func $preopens "get-directories") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $open-file (canon lower (func $fs "[method]descriptor.open-at") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $read-file (canon lower (func $fs "[method]descriptor.read") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $write-file (canon lower (func $fs "[method]descriptor.write") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $get-stdout (canon lower (func $stdout "get-stdout")))
 (core func $check-write (canon lower (func $streams "[method]output-stream.check-write") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $write (canon lower (func $streams "[method]output-stream.blocking-write-and-flush") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $now (canon lower (func $clock "now")))
 (core func $subscribe (canon lower (func $clock "subscribe-instant")))
 (core func $ready (canon lower (func $poll "[method]pollable.ready")))
 (core func $random (canon lower (func $random "get-random-u64")))
 (core func $arguments (canon lower (func $environment "get-arguments") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $environment (canon lower (func $environment "get-environment") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core module $m
  (import "host" "memory" (memory 2))
  (import "host" "directories" (func $directories (param i32)))
  (import "host" "open-file" (func $open-file (param i32 i32 i32 i32 i32 i32 i32)))
  (import "host" "read-file" (func $read-file (param i32 i64 i64 i32)))
  (import "host" "write-file" (func $write-file (param i32 i32 i32 i64 i32)))
  (import "host" "stdout" (func $stdout (result i32)))
  (import "host" "check" (func $check (param i32 i32)))
  (import "host" "write" (func $write (param i32 i32 i32 i32)))
  (import "host" "now" (func $now (result i64)))
  (import "host" "subscribe" (func $subscribe (param i64) (result i32)))
  (import "host" "ready" (func $ready (param i32) (result i32)))
  (import "host" "random" (func $random (result i64)))
  (import "host" "arguments" (func $arguments (param i32)))
  (import "host" "environment" (func $environment (param i32)))
  (data (i32.const 256) "../secret") (data (i32.const 272) "/secret") (data (i32.const 288) "fixture.txt")
  (data (i32.const 128) "wasi-fixture: streams clocks random environment ok\0a")
  (func (export "run") (result i32) (local $output i32) (local $before i64) (local $dir i32) (local $file i32)
   i32.const 0 call $arguments
   i32.const 16 call $environment
   ;; Boot has no preopens. Disk launch must access only its granted package.
   i32.const 80 call $directories
   i32.const 84 i32.load
   if
    i32.const 80 i32.load i32.load local.set $dir
    local.get $dir i32.const 0 i32.const 256 i32.const 9 i32.const 0 i32.const 1 i32.const 96 call $open-file
    i32.const 96 i32.load i32.const 1 i32.ne if unreachable end
    local.get $dir i32.const 0 i32.const 272 i32.const 7 i32.const 0 i32.const 1 i32.const 96 call $open-file
    i32.const 96 i32.load i32.const 1 i32.ne if unreachable end
    local.get $dir i32.const 0 i32.const 288 i32.const 11 i32.const 0 i32.const 1 i32.const 96 call $open-file
    i32.const 96 i32.load if unreachable end
    i32.const 100 i32.load local.set $file
    local.get $file i64.const 4 i64.const 0 i32.const 512 call $read-file
    i32.const 512 i32.load if unreachable end
    i32.const 520 i32.load i32.const 4 i32.ne if unreachable end
    i32.const 516 i32.load i32.load i32.const 0x6d736177 i32.ne if unreachable end
    ;; A descriptor opened for reading must reject writes before reaching BexOS.
    local.get $file i32.const 128 i32.const 1 i64.const 0 i32.const 512 call $write-file
    i32.const 512 i32.load i32.const 1 i32.ne if unreachable end
   end
   call $now local.set $before
   call $now local.get $before i64.lt_u if unreachable end
   local.get $before call $subscribe call $ready i32.eqz if unreachable end
   call $random drop
   call $stdout local.set $output
   local.get $output i32.const 32 call $check
   i32.const 32 i32.load if unreachable end
   i32.const 40 i64.load i64.const 51 i64.lt_u if unreachable end
   local.get $output i32.const 128 i32.const 51 i32.const 64 call $write
   i32.const 64 i32.load if unreachable end
   i32.const 0))
 (core instance $host
  (export "memory" (memory $memory "memory"))
  (export "directories" (func $directories)) (export "open-file" (func $open-file))
  (export "read-file" (func $read-file)) (export "write-file" (func $write-file))
  (export "stdout" (func $get-stdout)) (export "check" (func $check-write)) (export "write" (func $write))
  (export "now" (func $now)) (export "subscribe" (func $subscribe)) (export "ready" (func $ready)) (export "random" (func $random))
  (export "arguments" (func $arguments)) (export "environment" (func $environment)))
 (core instance $i (instantiate $m (with "host" (instance $host))))
 (func $run (result (result)) (canon lift (core func $i "run")))
 (instance $cli (export "run" (func $run)))
 (export "wasi:cli/run@0.2.12" (instance $cli)))
