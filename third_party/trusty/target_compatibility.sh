#!/bin/bash
# Shared target ABI adaptations. Called only on the action-local source tree.
set -eu
work="$1"
sed_cmd="$2"
case "$3" in
  aarch64) binder_char=u8 ;;
  x86_64) binder_char=i8 ;;
  *) echo "unsupported Trusty guest architecture: $3" >&2; exit 1 ;;
esac
# Binder's tiny Trusty-only log shim uses C++ forwarding headers solely for
# fprintf/abort. Bindgen runs with Trusty's target sysroot, where those host
# libc++ forwarding headers are intentionally unavailable. Use equivalent C
# declarations in the copied source tree without adding a host C++ standard
# library to every target compilation.
log_stub="$work/frameworks/native/libs/binder/liblog_stub/include/log/log.h"
"$sed_cmd" -i \
  -e 's|#include <cstdio>|#include <stdio.h>|' \
  -e 's|#include <cstdlib>|#include <stdlib.h>|' \
  -e 's|std::fprintf|fprintf|g' \
  -e 's|std::abort|abort|g' \
  -e 's|if constexpr (|if (|g' \
  "$log_stub"

# Rust does not mark the custom Trusty target as Unix, so its libstd hides
# std::os::fd despite Trusty's POSIX-compatible libc descriptor ABI. Route
# only Binder's imports through the owned-FD shim copied by the Bazel action.
binder_rust="$work/frameworks/native/libs/binder/rust/src"
"$sed_cmd" -i \
  's|use std::os::fd::|use crate::fd_compat::|' \
  "$binder_rust/binder.rs" \
  "$binder_rust/proxy.rs" \
  "$binder_rust/parcel/file_descriptor.rs"
"$sed_cmd" -i '/mod error;/a\
pub mod fd_compat;' "$binder_rust/lib.rs"

# Bindgen models ARM target C `char` as u8 for these Binder NDK
# declarations while Rust's custom Trusty target models c_char as i8. The
# ABI is byte-identical; normalize only the affected pointer boundaries in
# the copied upstream Rust source instead of changing either target ABI.
"$sed_cmd" -i \
  -e "s|use std::os::raw::c_char;|type BinderChar = $binder_char;|" \
  -e 's|c_char|BinderChar|g' \
  -e 's|descriptor\.as_ptr(),|descriptor.as_ptr() as *const _,|' \
  -e 's|CStr::from_ptr(raw_descriptor)|CStr::from_ptr(raw_descriptor as *const _)|' \
  "$binder_rust/binder.rs"
"$sed_cmd" -i \
  -e "s|use std::os::raw::c_char;|type BinderChar = $binder_char;|" \
  -e 's|c_char|BinderChar|g' \
  "$binder_rust/native.rs"
"$sed_cmd" -i \
  -e 's|message\.as_ptr())|message.as_ptr() as *const _)|g' \
  -e 's|CStr::from_ptr(description_ptr)|CStr::from_ptr(description_ptr as *const _)|' \
  "$binder_rust/error.rs"
"$sed_cmd" -i \
  -e '/use std::os::raw::c_char;/d' \
  -e 's|s\.as_ptr() as \*const c_char|s.as_ptr() as *const _|' \
  "$binder_rust/parcel/parcelable.rs"
"$sed_cmd" -i \
  's#map(|a| a\.as_ptr())#map(|a| a.as_ptr() as *const _)#' \
  "$binder_rust/proxy.rs"

# Like Binder, the pinned TIPC and service-manager Rust wrappers use the
# portable std::os::fd names even though this custom Trusty target does not
# advertise cfg(unix). Preserve the same i32 ownership semantics with an
# inherent TIPC conversion and Binder's local compatibility traits.
tipc_handle="$work/trusty/user/base/lib/tipc/rust/src/handle.rs"
"$sed_cmd" -i \
  -e '/use std::os::fd::{IntoRawFd, RawFd};/d' \
  -e '/use std::os::fd::IntoRawFd;/d' \
  -e 's/^impl IntoRawFd for Handle {/impl Handle {/' \
  -e 's/^    fn into_raw_fd(self) -> RawFd {/    pub fn into_raw_fd(self) -> i32 {/' \
  -e 's|port\.as_ptr(),|port.as_ptr() as *const _,|' \
  "$tipc_handle"
"$sed_cmd" -i \
  -e 's|cfg\.get_path()\.as_ptr(),|cfg.get_path().as_ptr() as *const _,|' \
  "$work/trusty/user/base/lib/tipc/rust/src/raw/handle_set_wrapper.rs"
"$sed_cmd" -i \
  -e 's|cfg\.path\.as_ptr(),|cfg.path.as_ptr() as *const _,|' \
  "$work/trusty/user/base/lib/tipc/rust/src/service.rs"
service_manager="$work/trusty/user/base/lib/service_manager/client/src/lib.rs"
"$sed_cmd" -i \
  's|use std::os::fd::IntoRawFd;|use binder::fd_compat::IntoRawFd;|' \
  "$service_manager"
rpcbinder="$work/frameworks/native/libs/binder/rust/rpcbinder/src"
"$sed_cmd" -i \
  's|use std::os::fd::RawFd;|use binder::fd_compat::RawFd;|' \
  "$rpcbinder/session.rs"
"$sed_cmd" -i \
  -e "s|use std::ffi::{c_char, c_void};|use std::ffi::c_void;\
type BinderChar = $binder_char;|" \
  -e 's|c_char|BinderChar|g' \
  "$rpcbinder/server/trusty.rs"

# AuthMgr FE/BE own Binder file descriptors with the same Trusty libc ABI.
# Keep them on the single compatibility type so ParcelFileDescriptor sees
# the exact OwnedFd type used by the patched Binder crate.
"$sed_cmd" -i \
  's|use std::os::fd::{FromRawFd, OwnedFd};|use binder::fd_compat::{FromRawFd, OwnedFd};|' \
  "$work/trusty/user/app/authmgr/authmgr-fe/accessor.rs"
"$sed_cmd" -i \
  -e 's|use std::os::fd::FromRawFd;|use binder::fd_compat::FromRawFd;|' \
  -e 's|use std::os::fd::OwnedFd;|use binder::fd_compat::OwnedFd;|' \
  "$work/trusty/user/app/authmgr/authmgr-be/lib/src/authorization_service.rs"
"$sed_cmd" -i 's/use std::{os::fd::AsRawFd, ptr::NonNull};/use binder::fd_compat::AsRawFd; use std::ptr::NonNull;/' \
  "$work/trusty/user/app/sample/hwcryptohal/server/cmd_processing.rs"

