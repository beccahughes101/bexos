(component
 (import "wasi:io/error@0.2.12" (instance $error (export "error" (type (sub resource)))))
 (alias export $error "error" (type $error-type))
 (import "wasi:io/poll@0.2.12" (instance $poll
  (export "pollable" (type $pollable (sub resource)))
  (export "[method]pollable.ready" (func (param "self" (borrow $pollable)) (result bool)))))
 (alias export $poll "pollable" (type $pollable))
 (import "wasi:io/streams@0.2.12" (instance $streams
  (alias outer 1 $error-type (type $outer-error)) (export "error" (type $error (eq $outer-error)))
  (alias outer 1 $pollable (type $outer-poll)) (export "pollable" (type $pollable (eq $outer-poll)))
  (export "input-stream" (type $input (sub resource)))
  (type $error-def (variant (case "last-operation-failed" (own $error)) (case "closed")))
  (export "stream-error" (type $stream-error (eq $error-def)))
  (export "[method]input-stream.read" (func (param "self" (borrow $input)) (param "len" u64) (result (result (list u8) (error $stream-error)))))
  (export "[method]input-stream.subscribe" (func (param "self" (borrow $input)) (result (own $pollable))))))
 (alias export $streams "input-stream" (type $input))
 (import "wasi:filesystem/types@0.2.12" (instance $fs
  (alias outer 1 $input (type $outer-input))
  (export "input-stream" (type $input (eq $outer-input)))
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
  (export "[method]descriptor.read-via-stream" (func (param "self" (borrow $descriptor)) (param "offset" u64) (result (result (own $input) (error $error)))))
  (export "[method]descriptor.read" (func (param "self" (borrow $descriptor)) (param "length" u64) (param "offset" u64) (result (result (tuple (list u8) bool) (error $error)))))
  (export "[method]descriptor.write" (func (param "self" (borrow $descriptor)) (param "buffer" (list u8)) (param "offset" u64) (result (result u64 (error $error)))))))
 (alias export $fs "descriptor" (type $descriptor))
 (import "wasi:filesystem/preopens@0.2.12" (instance $preopens
  (alias outer 1 $descriptor (type $outer-descriptor))
  (export "descriptor" (type $descriptor (eq $outer-descriptor)))
  (export "get-directories" (func (result (list (tuple (own $descriptor) string)))))))

 (import "bexos:wasm/checkpoint-resources@1.0.0" (instance $checkpoint
  (alias outer 1 $descriptor (type $outer-descriptor)) (export "descriptor" (type $descriptor (eq $outer-descriptor)))
  (alias outer 1 $input (type $outer-input)) (export "input-stream" (type $input (eq $outer-input)))
  (alias outer 1 $pollable (type $outer-poll)) (export "pollable" (type $pollable (eq $outer-poll)))
  (export "identify-descriptor" (func (param "value" (borrow $descriptor)) (result u32)))
  (export "adopt-descriptor" (func (param "token" u32) (result (result (own $descriptor)))))
  (export "identify-input" (func (param "value" (borrow $input)) (result u32)))
  (export "adopt-input" (func (param "token" u32) (result (result (own $input)))))
  (export "identify-pollable" (func (param "value" (borrow $pollable)) (result u32)))
  (export "adopt-pollable" (func (param "token" u32) (result (result (own $pollable)))))))
 (import "bexos:wasm/kernel@1.0.0" (instance $kernel
  (type $message-def (record (field "data" (list u8)) (field "resources" (list u32))))
  (export "message" (type $message (eq $message-def)))
  (export "channel-read" (func (param "id" u32) (param "max-bytes" u32) (param "max-resources" u32) (result (result $message))))
  (export "channel-write" (func (param "id" u32) (param "message" $message) (result (result))))))
 (core module $memory
  (memory (export "memory") 4)
  (global $next (mut i32) (i32.const 4096))
  (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $ptr i32)
   global.get $next local.set $ptr
   global.get $next local.get 3 i32.add i32.const 7 i32.add i32.const -8 i32.and global.set $next
   local.get $ptr))
 (core instance $memory (instantiate $memory))
 (core func $directories (canon lower (func $preopens "get-directories") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $open (canon lower (func $fs "[method]descriptor.open-at") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $input (canon lower (func $fs "[method]descriptor.read-via-stream") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $read (canon lower (func $streams "[method]input-stream.read") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $subscribe (canon lower (func $streams "[method]input-stream.subscribe")))
 (core func $ready (canon lower (func $poll "[method]pollable.ready")))
 (core func $channel-read (canon lower (func $kernel "channel-read") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $channel-write (canon lower (func $kernel "channel-write") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $identify-descriptor (canon lower (func $checkpoint "identify-descriptor")))
 (core func $adopt-descriptor (canon lower (func $checkpoint "adopt-descriptor") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $identify-input (canon lower (func $checkpoint "identify-input")))
 (core func $adopt-input (canon lower (func $checkpoint "adopt-input") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $identify-pollable (canon lower (func $checkpoint "identify-pollable")))
 (core func $adopt-pollable (canon lower (func $checkpoint "adopt-pollable") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (core func $drop-descriptor (canon resource.drop $descriptor))
 (core module $m
  (import "host" "memory" (memory 4))
  (import "host" "directories" (func $directories (param i32)))
  (import "host" "open" (func $open (param i32 i32 i32 i32 i32 i32 i32)))
  (import "host" "input" (func $input (param i32 i64 i32)))
  (import "host" "read" (func $read (param i32 i64 i32)))
  (import "host" "subscribe" (func $subscribe (param i32) (result i32)))
  (import "host" "ready" (func $ready (param i32) (result i32)))
  (import "host" "channel-read" (func $channel-read (param i32 i32 i32 i32)))
  (import "host" "channel-write" (func $channel-write (param i32 i32 i32 i32 i32) (result i32)))
  (import "host" "drop-descriptor" (func $drop-descriptor (param i32)))
  (import "host" "identify-descriptor" (func $identify-descriptor (param i32) (result i32)))
  (import "host" "adopt-descriptor" (func $adopt-descriptor (param i32 i32)))
  (import "host" "identify-input" (func $identify-input (param i32) (result i32)))
  (import "host" "adopt-input" (func $adopt-input (param i32 i32)))
  (import "host" "identify-pollable" (func $identify-pollable (param i32) (result i32)))
  (import "host" "adopt-pollable" (func $adopt-pollable (param i32 i32)))

  ;; State: client, counter, file descriptor, input stream, pollable, initialized.
  (data (i32.const 256) "sequence.txt")
  (func $initialize (local $ptr i32) (local $end i32) (local $directory i32)
   i32.const 80 call $directories
   i32.const 80 i32.load local.tee $ptr
   i32.const 84 i32.load i32.const 12 i32.mul i32.add local.set $end
   block $done loop $next
    local.get $ptr local.get $end i32.ge_u br_if $done
    local.get $ptr i32.load local.set $directory
    ;; Only /pkg is suitable; drop every preopen after opening its file.
    local.get $ptr i32.const 8 i32.add i32.load i32.const 4 i32.eq
    if local.get $ptr i32.const 4 i32.add i32.load i32.load i32.const 0x676b702f i32.eq
     if
      local.get $directory i32.const 0 i32.const 256 i32.const 12 i32.const 0 i32.const 1 i32.const 96 call $open
      i32.const 96 i32.load if unreachable end
      i32.const 8 i32.const 100 i32.load i32.store
      i32.const 20 i32.const 1 i32.store
     end
    end
    local.get $directory call $drop-descriptor
    local.get $ptr i32.const 12 i32.add local.set $ptr br $next
   end end
   i32.const 20 i32.load i32.eqz if unreachable end
   i32.const 8 i32.load i64.const 0 i32.const 96 call $input
   i32.const 96 i32.load if unreachable end
   i32.const 12 i32.const 100 i32.load i32.store
   i32.const 16 i32.const 12 i32.load call $subscribe i32.store)
  (func (export "dispatch") (param $client i32)
   i32.const 20 i32.load i32.eqz if call $initialize end
   local.get $client if i32.const 0 local.get $client i32.store end
   i32.const 0 i32.load i32.eqz if return end
   i32.const 0 i32.load i32.const 8 i32.const 0 i32.const 512 call $channel-read
   i32.const 512 i32.load if return end
   i32.const 16 i32.load call $ready i32.eqz if unreachable end
   i32.const 12 i32.load i64.const 1 i32.const 512 call $read
   i32.const 512 i32.load if unreachable end
   i32.const 520 i32.load i32.const 1 i32.ne if unreachable end
   i32.const 516 i32.load i32.load8_u
   i32.const 4 i32.load i32.const 10 i32.rem_u i32.const 48 i32.add i32.ne if unreachable end
   i32.const 4 i32.const 4 i32.load i32.const 1 i32.add i32.store
   i32.const 0 i32.load i32.const 4 i32.const 4 i32.const 0 i32.const 0 call $channel-write
   if unreachable end)
  (func (export "checkpoint") (result i32)
   i32.const 128 i32.const 0 i32.load i32.store
   i32.const 132 i32.const 4 i32.load i32.store
   i32.const 136 i32.const 8 i32.load call $identify-descriptor i32.store
   i32.const 140 i32.const 12 i32.load call $identify-input i32.store
   i32.const 144 i32.const 16 i32.load call $identify-pollable i32.store
   i32.const 64 i32.const 128 i32.store i32.const 68 i32.const 20 i32.store i32.const 64)
  (func (export "restore") (param $ptr i32) (param $len i32) (result i32)
   local.get $len i32.const 20 i32.ne if i32.const 1 return end
   i32.const 0 local.get $ptr i32.load i32.store
   i32.const 4 local.get $ptr i32.const 4 i32.add i32.load i32.store
   local.get $ptr i32.const 8 i32.add i32.load i32.const 96 call $adopt-descriptor
   i32.const 96 i32.load if i32.const 1 return end
   i32.const 8 i32.const 100 i32.load i32.store
   local.get $ptr i32.const 12 i32.add i32.load i32.const 96 call $adopt-input
   i32.const 96 i32.load if i32.const 1 return end
   i32.const 12 i32.const 100 i32.load i32.store
   local.get $ptr i32.const 16 i32.add i32.load i32.const 96 call $adopt-pollable
   i32.const 96 i32.load if i32.const 1 return end
   i32.const 16 i32.const 100 i32.load i32.store
   i32.const 20 i32.const 1 i32.store i32.const 0)
  (func (export "activate") i32.const 20 i32.load i32.eqz if call $initialize end) (func (export "abort")))
 (core instance $host (export "memory" (memory $memory "memory"))
  (export "directories" (func $directories))
  (export "open" (func $open))
  (export "input" (func $input))
  (export "read" (func $read))
  (export "subscribe" (func $subscribe))
  (export "ready" (func $ready))
  (export "channel-read" (func $channel-read))
  (export "channel-write" (func $channel-write))
  (export "identify-descriptor" (func $identify-descriptor))
  (export "adopt-descriptor" (func $adopt-descriptor))
  (export "identify-input" (func $identify-input))
  (export "adopt-input" (func $adopt-input))
  (export "identify-pollable" (func $identify-pollable))
  (export "adopt-pollable" (func $adopt-pollable))
  (export "drop-descriptor" (func $drop-descriptor)))
 (core instance $i (instantiate $m (with "host" (instance $host))))
 (func $dispatch (param "resource-id" u32) (canon lift (core func $i "dispatch")))
 (func $checkpoint (result (list u8)) (canon lift (core func $i "checkpoint") (memory $memory "memory")))
 (func $restore (param "checkpoint" (list u8)) (result (result)) (canon lift (core func $i "restore") (memory $memory "memory") (realloc (func $memory "realloc"))))
 (func $activate (canon lift (core func $i "activate"))) (func $abort (canon lift (core func $i "abort")))
 (instance $lifecycle (export "dispatch" (func $dispatch)) (export "checkpoint" (func $checkpoint)) (export "restore" (func $restore)) (export "activate" (func $activate)) (export "abort" (func $abort)))
 (export "bexos:wasm/lifecycle@1.0.0" (instance $lifecycle)))
