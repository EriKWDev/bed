#[derive(Clone, Copy)]
pub struct SeededRng {
    pub state: u64,
}

pub fn seeded_rng(seed: u64) -> SeededRng {
    SeededRng { state: seed }
}

impl SeededRng {
    #[inline]
    pub fn fork(&mut self) -> Self {
        seeded_rng(self.u64())
    }

    #[inline]
    pub fn u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(crate::utils::GOLDEN_RATIO_U64);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    #[inline]
    pub fn f32(&mut self) -> f32 {
        (self.u64() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }
}

pub fn random_direction(random: &mut SeededRng) -> glam::Vec3A {
    loop {
        let direction = glam::Vec3A::new(
            random.f32() * 2.0 - 1.0,
            random.f32() * 2.0 - 1.0,
            random.f32() * 2.0 - 1.0,
        );
        let length_squared = direction.length_squared();
        if length_squared > f32::EPSILON && length_squared <= 1.0 {
            return direction / length_squared.sqrt();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn seeded_stream_is_repeatable_and_seed_sensitive() {
        let mut first = super::seeded_rng(7);
        let mut same = super::seeded_rng(7);
        let mut different = super::seeded_rng(8);

        for _ in 0..16 {
            assert_eq!(first.u64(), same.u64());
        }
        assert_ne!(super::seeded_rng(7).u64(), different.u64());
    }

    #[test]
    fn random_directions_are_normalized() {
        let mut random = super::seeded_rng(11);
        for _ in 0..128 {
            let direction = super::random_direction(&mut random);
            assert!(direction.is_finite());
            assert!((direction.length_squared() - 1.0).abs() < 1e-5);
        }
    }
}
