//! Four-pixel source-over for matching premultiplied channel orders.
//! Both supported 64-bit architectures provide these SIMD instructions at base.

pub(crate) fn row(input: &[u8], output: &mut [u8], opacity: u8) {
    debug_assert_eq!(input.len(), output.len());
    debug_assert_eq!(input.len() % 4, 0);
    let mut offset = 0;
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    {
        let end = input.len() / 16 * 16;
        // Every load/store covers exactly four pixels within the borrowed row.
        unsafe {
            while offset < end {
                four(
                    input.as_ptr().add(offset),
                    output.as_mut_ptr().add(offset),
                    opacity,
                );
                offset += 16;
            }
        }
    }
    for (src, dst) in input[offset..]
        .chunks_exact(4)
        .zip(output[offset..].chunks_exact_mut(4))
    {
        let alpha = (u32::from(src[3]) * u32::from(opacity) + 127) / 255;
        for channel in 0..4 {
            dst[channel] = ((u32::from(src[channel]) * u32::from(opacity) + 127) / 255
                + (u32::from(dst[channel]) * (255 - alpha) + 127) / 255)
                .min(255) as u8;
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn four(input: *const u8, output: *mut u8, opacity: u8) {
    use core::arch::aarch64::*;
    unsafe {
        let src = vld1q_u8(input);
        let dst = vld1q_u8(output);
        let alpha_indices =
            vld1q_u8([3, 3, 3, 3, 7, 7, 7, 7, 11, 11, 11, 11, 15, 15, 15, 15].as_ptr());
        let alpha = vqtbl1q_u8(src, alpha_indices);
        let opacity = vdupq_n_u16(u16::from(opacity));
        // For 0..=65025, (x+128 + ((x+128)>>8))>>8 equals (x+127)/255.
        // The intermediate fits u16, including the extra carry term.
        let round = |x| {
            let x = vaddq_u16(x, vdupq_n_u16(128));
            vshrq_n_u16::<8>(vaddq_u16(x, vshrq_n_u16::<8>(x)))
        };
        let half = |src, dst, alpha| {
            let src = round(vmulq_u16(vmovl_u8(src), opacity));
            let alpha = round(vmulq_u16(vmovl_u8(alpha), opacity));
            let inverse = vsubq_u16(vdupq_n_u16(255), alpha);
            vqmovn_u16(vaddq_u16(src, round(vmulq_u16(vmovl_u8(dst), inverse))))
        };
        let low = half(vget_low_u8(src), vget_low_u8(dst), vget_low_u8(alpha));
        let high = half(vget_high_u8(src), vget_high_u8(dst), vget_high_u8(alpha));
        vst1q_u8(output, vcombine_u8(low, high));
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn four(input: *const u8, output: *mut u8, opacity: u8) {
    use core::arch::x86_64::*;
    unsafe {
        let src = _mm_loadu_si128(input.cast());
        let dst = _mm_loadu_si128(output.cast());
        let zero = _mm_setzero_si128();
        let opacity = _mm_set1_epi16(i16::from(opacity));
        let round = |x| {
            let x = _mm_add_epi16(x, _mm_set1_epi16(128));
            _mm_srli_epi16::<8>(_mm_add_epi16(x, _mm_srli_epi16::<8>(x)))
        };
        let half = |src, dst| {
            let alpha = _mm_shufflehi_epi16::<255>(_mm_shufflelo_epi16::<255>(src));
            let alpha = round(_mm_mullo_epi16(alpha, opacity));
            let inverse = _mm_sub_epi16(_mm_set1_epi16(255), alpha);
            _mm_add_epi16(
                round(_mm_mullo_epi16(src, opacity)),
                round(_mm_mullo_epi16(dst, inverse)),
            )
        };
        let low = half(_mm_unpacklo_epi8(src, zero), _mm_unpacklo_epi8(dst, zero));
        let high = half(_mm_unpackhi_epi8(src, zero), _mm_unpackhi_epi8(dst, zero));
        _mm_storeu_si128(output.cast(), _mm_packus_epi16(low, high));
    }
}
