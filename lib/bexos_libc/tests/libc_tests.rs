#[test]
fn memory_primitives_match_c_contracts() {
    let src = *b"abcdef";
    let mut dst = [0u8; 6];
    unsafe {
        bexos_libc::memcpy(dst.as_mut_ptr().cast(), src.as_ptr().cast(), src.len());
        assert_eq!(dst, src);
        bexos_libc::memmove(dst.as_mut_ptr().add(1).cast(), dst.as_ptr().cast(), 4);
        assert_eq!(&dst, b"aabcdf");
        bexos_libc::memset(dst.as_mut_ptr().cast(), b'x' as i32, 3);
        assert_eq!(&dst, b"xxxcdf");
        assert_eq!(bexos_libc::strlen(c"hello".as_ptr()), 5);
    }
}

#[test]
fn c_strings_respect_unsigned_bytes_limits_padding_and_search_boundaries() {
    use bexos_libc::*;
    unsafe {
        assert!(strcmp(c"\u{ff}".as_ptr(), c"z".as_ptr()) > 0);
        assert_eq!(strncmp(core::ptr::null(), core::ptr::null(), 0), 0);
        assert_eq!(strcasecmp(c"VeNuS".as_ptr(), c"venus".as_ptr()), 0);
        let s = c"abcb";
        assert_eq!(strchr(s.as_ptr(), 0), s.as_ptr().add(4).cast_mut());
        assert_eq!(
            strrchr(s.as_ptr(), b'b' as i32),
            s.as_ptr().add(3).cast_mut()
        );
        assert!(strchr(s.as_ptr(), b'x' as i32).is_null());
        assert_eq!(
            strstr(s.as_ptr(), c"bc".as_ptr()),
            s.as_ptr().add(1).cast_mut()
        );
        assert!(strstr(s.as_ptr(), c"bcbbb".as_ptr()).is_null());
        assert_eq!(strspn(c"123ab".as_ptr(), c"123".as_ptr()), 3);
        assert_eq!(strcspn(c"123ab".as_ptr(), c"ba".as_ptr()), 3);
        let mut out = [77u8; 7];
        strncpy(out.as_mut_ptr().cast(), c"xy".as_ptr(), 5);
        assert_eq!(out, [b'x', b'y', 0, 0, 0, 77, 77]);
        strncpy(out.as_mut_ptr().cast(), c"abcdef".as_ptr(), 3);
        assert_eq!(&out[..4], b"abc\0");
        assert_eq!(strnlen(out.as_ptr().cast(), 2), 2);
    }
}

#[test]
fn c_integer_conversion_handles_prefixes_end_pointers_and_overflow() {
    use bexos_libc::*;
    unsafe {
        assert_eq!(atoi(c"25.3.0".as_ptr()), 25);
        assert_eq!(atoi(c" \t-253.0".as_ptr()), -253);
        assert_eq!(atoi(c"invalid".as_ptr()), 0);
        for (input, base, value, end_offset, error) in [
            (c" \t-0x8000000000000000!", 0, i64::MIN, 21, 0),
            (c"9223372036854775808 rest", 10, i64::MAX, 19, 34),
            (c"-9223372036854775809", 10, i64::MIN, 20, 34),
            (c"  -xyz", 10, 0, 0, 0),
            (c"0x!", 0, 0, 1, 0),
            (c"0779", 0, 63, 3, 0),
        ] {
            *__errno_location() = 0;
            let mut end = core::ptr::null_mut();
            assert_eq!(strtoll(input.as_ptr(), &mut end, base), value, "{input:?}");
            assert_eq!(end, input.as_ptr().add(end_offset).cast_mut(), "{input:?}");
            assert_eq!(*__errno_location(), error);
        }
        let input = c"0b1012";
        let mut end = core::ptr::null_mut();
        assert_eq!(__isoc23_strtoll(input.as_ptr(), &mut end, 0), 5);
        assert_eq!(end, input.as_ptr().add(5).cast_mut());
        assert_eq!(strtoll(input.as_ptr(), &mut end, 0), 0);
        assert_eq!(end, input.as_ptr().add(1).cast_mut());
    }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[test]
fn socket_rejects_unsupported_domain() {
    assert_eq!(bexos_libc::socket(99, 1, 0), -1);
    unsafe {
        assert_eq!(*bexos_libc::__errno_location(), 97);
    }
}

#[test]
fn poll_marks_bad_fd_without_allocating() {
    let mut fds = [PollFd {
        fd: 99,
        events: 1,
        revents: 0,
    }];
    assert_eq!(bexos_libc::poll(fds.as_mut_ptr().cast(), fds.len(), 0), 1);
    assert_eq!(fds[0].revents, 8);
}

#[test]
fn pthread_specific_values_round_trip() {
    let mut key = 0u32;
    assert_eq!(bexos_libc::pthread_key_create(&mut key, None), 0);
    let value = 0x1234usize as *const core::ffi::c_void;
    assert_eq!(bexos_libc::pthread_setspecific(key, value), 0);
    assert_eq!(bexos_libc::pthread_getspecific(key), value.cast_mut());
    assert_eq!(bexos_libc::pthread_key_delete(key), 0);
}

#[test]
fn pthread_key_abi_and_reuse_do_not_expose_deleted_values() {
    #[repr(C)]
    struct GuardedKey {
        key: u32,
        guard: u32,
    }
    let mut slot = GuardedKey {
        key: 0,
        guard: 0xdeadbeef,
    };
    for _ in 0..128 {
        assert_eq!(bexos_libc::pthread_key_create(&mut slot.key, None), 0);
        assert_eq!(
            slot.guard, 0xdeadbeef,
            "pthread_key_t write exceeded four bytes"
        );
        assert!(bexos_libc::pthread_getspecific(slot.key).is_null());
        assert_eq!(
            bexos_libc::pthread_setspecific(slot.key, 1usize as *const _),
            0
        );
        assert_eq!(bexos_libc::pthread_key_delete(slot.key), 0);
        assert!(bexos_libc::pthread_getspecific(slot.key).is_null());
        assert_eq!(
            bexos_libc::pthread_setspecific(slot.key, std::ptr::null()),
            22
        );
    }
}

#[test]
fn eventfd_writes_are_drained_by_read() {
    let fd = bexos_libc::eventfd(0, 0);
    assert!(fd >= 0);

    let value = 3u64;
    assert_eq!(
        bexos_libc::write(
            fd,
            (&value as *const u64).cast(),
            core::mem::size_of::<u64>()
        ),
        8
    );

    let mut out = 0u64;
    assert_eq!(
        bexos_libc::read(
            fd,
            (&mut out as *mut u64).cast(),
            core::mem::size_of::<u64>()
        ),
        8
    );
    assert_eq!(out, value);
    assert_eq!(bexos_libc::close(fd), 0);
}

#[test]
fn duplicated_descriptors_keep_shared_state_alive_until_the_last_close() {
    let fd = bexos_libc::epoll_create1(0);
    assert!(fd >= 0);

    let duplicate = bexos_libc::fcntl(fd, 1030, 3);
    assert_eq!(duplicate, fd);
    assert_eq!(bexos_libc::close(duplicate), 0);
    assert_eq!(bexos_libc::fcntl(fd, 1, 0), 0);
    assert_eq!(bexos_libc::close(fd), 0);
    assert_eq!(bexos_libc::fcntl(fd, 1, 0), -1);
}

#[test]
fn socket_options_cover_runtime_probes() {
    let fd = bexos_libc::socket(2, 1, 0);
    assert!(fd >= 0);

    let enabled = 1i32;
    assert_eq!(
        bexos_libc::setsockopt(
            fd,
            1,
            2,
            (&enabled as *const i32).cast(),
            core::mem::size_of::<i32>() as u32,
        ),
        0
    );

    let mut value = -1i32;
    let mut len = core::mem::size_of::<i32>() as u32;
    assert_eq!(
        bexos_libc::getsockopt(fd, 1, 3, (&mut value as *mut i32).cast(), &mut len,),
        0
    );
    assert_eq!(value, 1);
    assert_eq!(bexos_libc::close(fd), 0);
}
#[test]
fn pthread_plain_mutex_enforces_exclusion_and_rejects_unsupported_attributes() {
    let mut words = [0u64; 6];
    let mutex = words.as_mut_ptr().cast();
    assert_eq!(bexos_libc::pthread_mutex_init(mutex, core::ptr::null()), 0);
    assert_eq!(bexos_libc::pthread_mutex_lock(mutex), 0);
    assert_eq!(bexos_libc::pthread_mutex_trylock(mutex), 16);
    assert_eq!(bexos_libc::pthread_mutex_destroy(mutex), 16);
    assert_eq!(bexos_libc::pthread_mutex_unlock(mutex), 0);
    assert_eq!(bexos_libc::pthread_mutex_trylock(mutex), 0);
    assert_eq!(bexos_libc::pthread_mutex_unlock(mutex), 0);
    assert_eq!(bexos_libc::pthread_mutex_destroy(mutex), 0);
    assert_eq!(bexos_libc::pthread_mutex_lock(mutex), 22);
    let unsupported = 1u32;
    assert_eq!(
        bexos_libc::pthread_mutex_init(mutex, (&unsupported as *const u32).cast()),
        95
    );
    assert_eq!(bexos_libc::pthread_mutex_lock(core::ptr::null_mut()), 22);
}

#[test]
fn pthread_condition_abi_validates_attributes_and_preserves_mutex_on_bad_deadline() {
    use bexos_libc::*;
    let mut cond = [0u64; 7];
    cond[6] = 0xdeadbeef;
    let mut mutex = [0u64; 6];
    let c = cond.as_mut_ptr().cast();
    let m = mutex.as_mut_ptr().cast();
    let mut attributes = [0u32, 0x12345678];
    let a = attributes.as_mut_ptr().cast();
    assert_eq!(pthread_condattr_init(a), 0);
    assert_eq!(pthread_condattr_setclock(a, 1), 0);
    let mut clock = -1;
    assert_eq!(pthread_condattr_getclock(a, &mut clock), 0);
    assert_eq!(clock, 1);
    assert_eq!(pthread_condattr_setclock(a, 7), 22);
    assert_eq!(pthread_condattr_setpshared(a, 1), 95);
    assert_eq!(pthread_cond_init(c, a), 0);
    assert_eq!(attributes[1], 0x12345678);
    assert_eq!(cond[6], 0xdeadbeef);
    assert_eq!(pthread_mutex_init(m, core::ptr::null()), 0);
    assert_eq!(pthread_mutex_lock(m), 0);
    let invalid_time = [0i64, 1_000_000_000];
    assert_eq!(
        pthread_cond_timedwait(c, m, invalid_time.as_ptr().cast()),
        22
    );
    assert_eq!(pthread_mutex_trylock(m), 16);
    assert_eq!(pthread_mutex_unlock(m), 0);
    assert_eq!(pthread_cond_destroy(c), 0);
    assert_eq!(pthread_cond_signal(c), 22);
    assert_eq!(pthread_condattr_destroy(a), 0);
    assert_eq!(pthread_mutex_destroy(m), 0);
}

#[test]
fn unsupported_platform_calls_do_not_report_success_or_invent_cpu_topology() {
    use bexos_libc::*;
    let mut mask = [0xa5u8; 16];
    assert_eq!(sysconf(30), 4096);
    assert_eq!(sysconf(84), -1);
    assert_eq!(mprotect(core::ptr::null_mut(), 4096, 0), -1);
    assert_eq!(sigaltstack(core::ptr::null(), core::ptr::null_mut()), -1);
    assert_eq!(
        sched_getaffinity(0, mask.len(), mask.as_mut_ptr().cast()),
        -1
    );
    assert_eq!(mask, [0xa5; 16]);
    assert_ne!(pthread_setname_np(1, c"test".as_ptr()), 0);
}
#[test]
fn dynamic_loader_failures_report_and_clear_errors() {
    use bexos_libc::*;
    use core::ptr;
    let _ = dlerror();
    assert!(dlerror().is_null());
    assert!(dlopen(c"libvulkan.so.1".as_ptr(), 2).is_null());
    unsafe {
        assert_eq!(*__errno_location(), 38);
        assert_eq!(
            core::ffi::CStr::from_ptr(dlerror()),
            c"dynamic loading is not supported"
        );
    }
    assert!(dlerror().is_null());
    assert!(dlsym(ptr::null_mut(), c"vkGetInstanceProcAddr".as_ptr()).is_null());
    assert!(!dlerror().is_null());
    assert!(dlerror().is_null());
    assert_eq!(dlclose(ptr::null_mut()), -1);
    assert!(!dlerror().is_null());
    assert!(dlerror().is_null());
}
