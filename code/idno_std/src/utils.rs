//! Small shared helpers; the naming scheme lives in docs/PRINCIPLES.md.

pub use glam::*;

// Splines

/// Sample a centripetal Catmull–Rom spline at approximately equal arc-length
/// intervals. The dense approximation and result use caller-owned scratch
/// storage; this is the same sampling used by Bladerend's handle-driven
/// procedural decorations.
pub fn sample_spline_many_evenly_spaced<'scope, 'arena>(
    points: &[Vec3A],
    samples_per_segment_accuracy: usize,
    target_spacing: f32,
    arena: &'arena crate::arena::TempArena<'scope>,
) -> (Vec<Vec3A, &'arena crate::arena::TempArena<'scope>>, f32) {
    profiling::function_scope!();
    assert!(points.len() >= 2);
    assert!(samples_per_segment_accuracy >= 2);

    let mut dense = arena.vec::<Vec3A>(points.len() * samples_per_segment_accuracy);
    for segment in 0..points.len() - 1 {
        for sample in 0..samples_per_segment_accuracy {
            let amount = sample as f32 / (samples_per_segment_accuracy - 1) as f32;
            dense.push(sample_spline_segment(points, segment, amount, 0.5));
        }
    }

    let mut lengths = arena.vec::<f32>(dense.len());
    lengths.push(0.0);
    for index in 1..dense.len() {
        lengths.push(lengths[index - 1] + dense[index].distance(dense[index - 1]));
    }
    let total_length = *lengths.last().unwrap();
    let spacing = target_spacing.max(0.1);
    let mut result = arena.vec::<Vec3A>((total_length / spacing) as usize);
    let mut distance = 0.0;
    let mut dense_index = 0;
    loop {
        while dense_index + 1 < lengths.len() && lengths[dense_index + 1] < distance {
            dense_index += 1;
        }
        if dense_index + 1 == lengths.len() {
            break;
        }
        let first_length = lengths[dense_index];
        let second_length = lengths[dense_index + 1];
        let amount = (distance - first_length) / (second_length - first_length);
        result.push(dense[dense_index].lerp(dense[dense_index + 1], amount));
        distance += spacing;
    }
    (result, total_length)
}

#[inline]
pub fn sample_spline_segment(points: &[Vec3A], segment: usize, amount: f32, alpha: f32) -> Vec3A {
    let first = points[segment];
    let second = points[segment + 1];
    let before = if segment == 0 {
        first + (first - second)
    } else {
        points[segment - 1]
    };
    let after = if segment + 2 >= points.len() {
        second + (second - first)
    } else {
        points[segment + 2]
    };
    catmull_rom_centripetal(before, first, second, after, amount, alpha)
}

#[inline]
pub fn catmull_rom_centripetal(
    before: Vec3A,
    first: Vec3A,
    second: Vec3A,
    after: Vec3A,
    amount: f32,
    alpha: f32,
) -> Vec3A {
    let next =
        |time: f32, point: Vec3A, next_point: Vec3A| time + point.distance(next_point).powf(alpha);
    let epsilon = 1e-6;
    let time_before = 0.0;
    let time_first = next(time_before, before, first);
    let time_second = next(time_first, first, second);
    let time_after = next(time_second, second, after);
    let time = time_first + (time_second - time_first) * amount;

    let before_first = (time_first - time_before).max(epsilon);
    let first_second = (time_second - time_first).max(epsilon);
    let second_after = (time_after - time_second).max(epsilon);
    let before_second = (time_second - time_before).max(epsilon);
    let first_after = (time_after - time_first).max(epsilon);

    let a1 = before * ((time_first - time) / before_first)
        + first * ((time - time_before) / before_first);
    let a2 = first * ((time_second - time) / first_second)
        + second * ((time - time_first) / first_second);
    let a3 = second * ((time_after - time) / second_after)
        + after * ((time - time_second) / second_after);
    let b1 =
        a1 * ((time_second - time) / before_second) + a2 * ((time - time_before) / before_second);
    let b2 = a2 * ((time_after - time) / first_after) + a3 * ((time - time_first) / first_after);
    b1 * ((time_second - time) / first_second) + b2 * ((time - time_first) / first_second)
}

// Byte views

#[inline]
pub fn bytes_of<T: Copy>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) }
}

#[inline]
pub fn bytes_of_slice<T: Copy>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, size_of_val(slice)) }
}

// Bit packing (SDR: components clamped to 0..1, 8 bits each)

#[inline]
pub fn pack_f32unorm_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0) as u8
}

/// Three 0..1 components into the low 3 bytes: `0x00CCBBAA`.
#[inline]
pub fn pack_3f32unorm_to_u32(v: [f32; 3]) -> u32 {
    pack_f32unorm_to_u8(v[0]) as u32
        | (pack_f32unorm_to_u8(v[1]) as u32) << 8
        | (pack_f32unorm_to_u8(v[2]) as u32) << 16
}

/// Four 0..1 components into four bytes: `0xDDCCBBAA`.
#[inline]
pub fn pack_4f32unorm_to_u32(v: [f32; 4]) -> u32 {
    pack_3f32unorm_to_u32([v[0], v[1], v[2]]) | (pack_f32unorm_to_u8(v[3]) as u32) << 24
}

#[inline]
pub fn unpack_3f32unorm_from_u32(packed: u32) -> [f32; 3] {
    [
        (packed & 0xff) as f32 / 255.0,
        ((packed >> 8) & 0xff) as f32 / 255.0,
        ((packed >> 16) & 0xff) as f32 / 255.0,
    ]
}

#[inline]
pub fn unpack_3u8_from_u32(packed: u32) -> [u8; 3] {
    [
        (packed & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
    ]
}

// Color-space conversions (SDR, components 0..1)

#[inline]
pub fn convert_srgb_component_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn convert_linear_component_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
pub fn convert_linear_to_srgb(rgb: Vec3A) -> Vec3A {
    Vec3A::new(
        convert_linear_component_to_srgb(rgb.x),
        convert_linear_component_to_srgb(rgb.y),
        convert_linear_component_to_srgb(rgb.z),
    )
}

#[inline]
pub fn convert_srgb_to_linear(rgb: Vec3A) -> Vec3A {
    Vec3A::new(
        convert_srgb_component_to_linear(rgb.x),
        convert_srgb_component_to_linear(rgb.y),
        convert_srgb_component_to_linear(rgb.z),
    )
}

#[inline]
pub fn convert_linear_rgba_to_srgb(rgba: [f32; 4]) -> [f32; 4] {
    let rgb = convert_linear_to_srgb(Vec3A::from_array([rgba[0], rgba[1], rgba[2]]));
    [rgb.x, rgb.y, rgb.z, rgba[3]]
}

pub fn convert_hsv_to_rgb(hsv: Vec3A) -> Vec3A {
    let h = (hsv.x.fract() + 1.0).fract() * 6.0;
    let c = hsv.z * hsv.y;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    vec3a(r, g, b) + Vec3A::splat(hsv.z - c)
}

pub fn convert_rgb_to_hsv(rgb: Vec3A) -> Vec3A {
    let maximum = rgb.max_element();
    let minimum = rgb.min_element();
    let chroma = maximum - minimum;
    let hue = if chroma <= f32::EPSILON {
        0.0
    } else if maximum == rgb.x {
        ((rgb.y - rgb.z) / chroma).rem_euclid(6.0) / 6.0
    } else if maximum == rgb.y {
        ((rgb.z - rgb.x) / chroma + 2.0) / 6.0
    } else {
        ((rgb.x - rgb.y) / chroma + 4.0) / 6.0
    };
    let saturation = if maximum <= f32::EPSILON {
        0.0
    } else {
        chroma / maximum
    };
    Vec3A::new(hue, saturation, maximum)
}

pub fn convert_srgb_to_oklab(rgb: Vec3A) -> Vec3A {
    let r = convert_srgb_component_to_linear(rgb.x);
    let g = convert_srgb_component_to_linear(rgb.y);
    let b = convert_srgb_component_to_linear(rgb.z);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    vec3a(
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )
}

pub fn convert_oklab_to_srgb(lab: Vec3A) -> Vec3A {
    let l = (lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z).powi(3);
    let m = (lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z).powi(3);
    let s = (lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z).powi(3);
    let encode = |c: f32| convert_linear_component_to_srgb(c).clamp(0.0, 1.0);
    vec3a(
        encode(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        encode(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        encode(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
    )
}

// Hashing and integer encodings

/// 2^64 / phi.
pub const GOLDEN_RATIO_U64: u64 = 0x9E3779B97F4A7C15;
/// 2^32 / phi (shader-side hashes mirror this).
pub const GOLDEN_RATIO_U32: u32 = 0x9E37_79B9;
pub const GOLDEN_RATIO_F32: f32 = 0.618_034;

/// ~1ns, full 64-bit avalanche from one multiply + xorshift; NOT
/// cryptographic, NOT zero-collision — a cheap diffuser for structured
/// integer keys ahead of power-of-two bucket masks.
#[inline]
pub const fn hash_multiply_xorshift_u64(v: u64) -> u64 {
    let h = v.wrapping_mul(GOLDEN_RATIO_U64);
    h ^ (h >> 32)
}

pub const HASH_FNV1A_SEED: u64 = 0xcbf29ce484222325;

pub const fn hash_fnv1a_str(mut hash: u64, s: &str) -> u64 {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

pub const fn hash_fnv1a_usize(mut hash: u64, v: usize) -> u64 {
    hash ^= v as u64;
    hash.wrapping_mul(0x100000001b3)
}

/// Deterministic bulk-byte hash (rapidhash v3, ~bytes/cycle in the tens):
/// for content identity of frames and payloads, where FNV's byte-at-a-time
/// loop would cost milliseconds per megabyte. NOT cryptographic.
#[inline]
pub fn hash_rapid_bytes(bytes: &[u8]) -> u64 {
    rapidhash::v3::rapidhash_v3(bytes)
}

#[inline]
pub const fn encode_i32_as_zigzag_u32(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

#[inline]
pub const fn decode_zigzag_u32_as_i32(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

// Coordinate-space conversions

#[inline]
pub fn convert_window_cursor_to_ndc(cursor: Vec2, viewport: Vec2) -> Vec2 {
    Vec2::new(
        cursor.x / viewport.x * 2.0 - 1.0,
        -(cursor.y / viewport.y * 2.0 - 1.0),
    )
}

#[inline]
pub fn intersect_ray_with_plane(
    origin: Vec3A,
    direction: Vec3A,
    plane_point: Vec3A,
    plane_normal: Vec3A,
) -> Option<Vec3A> {
    let denom = direction.dot(plane_normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_point - origin).dot(plane_normal) / denom;
    (t > 0.0).then(|| origin + direction * t)
}

// Smoothing and formatting

#[inline]
pub fn exponential_smoothing_alpha(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

/*
    NOTE: `convergence_seconds` is the time to move 99% of the remaining
          distance. Bladerend authored camera and ambience timings with this
          meaning, so it is distinct from an exponential rate/time constant.
*/
#[inline]
pub fn stable_lerp_alpha(delta_seconds: f32, convergence_seconds: f32) -> f32 {
    if convergence_seconds <= 0.0 {
        return 1.0;
    }
    let half_life = -(convergence_seconds / 0.01f32.log2());
    1.0 - (-delta_seconds.max(0.0) / half_life).exp2()
}

#[inline]
pub const fn alpha_from_span(current: f32, start: f32, end: f32) -> f32 {
    ((current - start) / (end - start)).clamp(0.0, 1.0)
}

#[inline]
pub const fn alpha_from_start_mid_end(
    current: f32,
    start: f32,
    rise_end: f32,
    fall_start: f32,
    end: f32,
) -> f32 {
    if current <= rise_end {
        alpha_from_span(current, start, rise_end)
    } else {
        1.0 - alpha_from_span(current, fall_start, end)
    }
    .clamp(0.0, 1.0)
}

#[inline]
pub const fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

/// Quintic ease: zero velocity AND acceleration at both ends, where
/// `smoothstep` only zeroes velocity. The difference shows on long moves.
#[inline]
pub const fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

/// Symmetric S-curve over `[0, 1]`; `shape > 1` sharpens the midpoint.
#[inline]
pub fn sigmoid(value: f32, shape: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value == 0.0 || value == 1.0 {
        return value;
    }
    1.0 - 1.0 / (1.0 + (value / (1.0 - value)).powf(shape))
}

#[inline]
pub const fn cubic_in_out(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value < 0.5 {
        4.0 * value * value * value
    } else {
        let shifted = 2.0 * value - 2.0;
        0.5 * shifted * shifted * shifted + 1.0
    }
}

#[inline]
pub fn elastic_in(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value == 0.0 || value == 1.0 {
        return value;
    }
    let period = 2.0 * std::f32::consts::PI / 3.0;
    2.0f32.powf(-10.0 * value) * ((10.0 * value - 0.75) * period).sin() + 1.0
}

#[inline]
pub fn project_point_onto_line(point: Vec3A, line_point: Vec3A, line_direction: Vec3A) -> Vec3A {
    let direction = line_direction.normalize_or_zero();
    line_point + direction * (point - line_point).dot(direction)
}

#[inline]
pub fn ease_out_elastic(value: f32) -> f32 {
    if value == 0.0 || value == 1.0 {
        return value;
    }
    let period = core::f32::consts::TAU / 3.0;
    2.0f32.powf(-10.0 * value) * ((value * 10.0 - 0.75) * period).sin() + 1.0
}

/// Format `prefix` + decimal `number` into a caller stack buffer, no
/// allocation: built for per-row name synthesis on bulk-spawn paths.
/// Truncates the prefix if the pair cannot fit.
pub fn format_str_then_usize<'buffer>(
    buffer: &'buffer mut [u8; 64],
    prefix: &str,
    number: usize,
) -> &'buffer str {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut remaining = number;
    loop {
        at -= 1;
        digits[at] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let digit_count = digits.len() - at;
    let mut prefix_length = prefix.len().min(buffer.len() - digit_count);
    while !prefix.is_char_boundary(prefix_length) {
        prefix_length -= 1;
    }
    buffer[..prefix_length].copy_from_slice(&prefix.as_bytes()[..prefix_length]);
    buffer[prefix_length..prefix_length + digit_count].copy_from_slice(&digits[at..]);
    std::str::from_utf8(&buffer[..prefix_length + digit_count]).unwrap_or("")
}

pub fn format_usize_as_thousands(value: usize, out: &mut String) {
    let digits = value.to_string();
    out.reserve(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('\'');
        }
        out.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_and_hsv_round_trip_primary_and_desaturated_colors() {
        for rgb in [
            Vec3A::ZERO,
            Vec3A::ONE,
            Vec3A::X,
            Vec3A::Y,
            Vec3A::Z,
            Vec3A::new(0.2, 0.4, 0.7),
        ] {
            let round_trip = convert_hsv_to_rgb(convert_rgb_to_hsv(rgb));
            assert!(
                round_trip.abs_diff_eq(rgb, 1e-5),
                "{rgb:?} became {round_trip:?}"
            );
        }
    }

    #[test]
    fn linear_and_srgb_transfer_functions_are_inverse() {
        for linear in [0.0, 0.001, 0.0031308, 0.18, 0.5, 1.0] {
            let round_trip =
                convert_srgb_component_to_linear(convert_linear_component_to_srgb(linear));
            assert!(
                (round_trip - linear).abs() < 1e-6,
                "{linear} became {round_trip}"
            );
        }
    }

    #[test]
    fn thousands_groups_digits() {
        let thousands = |value| {
            let mut text = String::new();
            format_usize_as_thousands(value, &mut text);
            text
        };
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1'000");
        assert_eq!(thousands(12_345), "12'345");
        assert_eq!(thousands(1_000_000), "1'000'000");
    }
}
