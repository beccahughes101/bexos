//! libm entry points used by text, vector rendering and shader translation.
macro_rules! unary {($($name:ident),*)=>{$(#[cfg_attr(bexos_guest, unsafe(no_mangle))] pub extern "C" fn $name(x:f64)->f64{libm::$name(x)})*}}
unary!(
    sin, cos, tan, log2, atan, asin, acos, exp, log, exp2, sinh, cosh, tanh, asinh, acosh, atanh,
    log1p, expm1, log10
);
macro_rules! unary32 {($($name:ident),*)=>{$(#[cfg_attr(bexos_guest, unsafe(no_mangle))] pub extern "C" fn $name(x:f32)->f32{libm::$name(x)})*}}
unary32!(
    sinf, cosf, tanf, expf, exp2f, sinhf, coshf, tanhf, asinf, acosf, atanf, asinhf, acoshf,
    atanhf, logf, log2f, log1pf, expm1f, log10f
);
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn atan2f(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn hypotf(x: f32, y: f32) -> f32 {
    libm::hypotf(x, y)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn sincosf(x: f32, s: *mut f32, c: *mut f32) {
    let (a, b) = libm::sincosf(x);
    unsafe {
        *s = a;
        *c = b;
    }
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub extern "C" fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn sincos(x: f64, s: *mut f64, c: *mut f64) {
    let (a, b) = libm::sincos(x);
    unsafe {
        *s = a;
        *c = b;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_math_handles_domains_and_extreme_values() {
        assert_eq!(super::exp2f(10.), 1024.);
        assert_eq!(super::log2f(1024.), 10.);
        assert_eq!(super::exp2(-3.), 0.125);
        assert_eq!(super::acosh(1.), 0.);
        assert!(super::acoshf(0.5).is_nan());
        assert_eq!(super::log1p(-1.), f64::NEG_INFINITY);
        assert_eq!(super::tanh(f64::INFINITY), 1.);
        assert_eq!(super::sinhf(f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert!((super::asinh(super::sinh(0.5)) - 0.5).abs() < 1e-12);
    }
    #[test]
    fn single_precision_text_color_and_segmentation_math() {
        assert_eq!(super::hypotf(3., 4.), 5.);
        assert_eq!(super::expf(0.), 1.);
        assert_eq!(super::tanhf(f32::INFINITY), 1.);
        assert!((super::tanf(core::f32::consts::FRAC_PI_4) - 1.).abs() < 1e-6);
        assert!((super::atan2f(1., 0.) - core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let (mut sine, mut cosine) = (0., 0.);
        unsafe { super::sincosf(core::f32::consts::FRAC_PI_2, &mut sine, &mut cosine) };
        assert!((sine - 1.).abs() < 1e-6);
        assert!(cosine.abs() < 1e-6);
        assert!(super::tanf(f32::NAN).is_nan());
    }
    #[test]
    fn fractional_powers_and_trigonometry_used_by_vector_rendering() {
        assert!((crate::pow(9., 0.5) - 3.).abs() < 1e-12);
        assert!((super::powf(16., 0.25) - 2.).abs() < 1e-6);
        assert!((super::hypot(3., 4.) - 5.).abs() < 1e-12);
        let (mut sine, mut cosine) = (0., 0.);
        unsafe { super::sincos(core::f64::consts::FRAC_PI_2, &mut sine, &mut cosine) };
        assert!((sine - 1.).abs() < 1e-12);
        assert!(cosine.abs() < 1e-12);
        assert!(crate::pow(-1., 0.5).is_nan());
    }
}
