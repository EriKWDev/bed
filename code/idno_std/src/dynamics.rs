//! Stable second-order dynamics shared by camera and gameplay motion.

pub trait DynamicsValue:
    Copy
    + std::ops::Add<Output = Self>
    + std::ops::Sub<Output = Self>
    + std::ops::Mul<f32, Output = Self>
    + std::ops::Div<f32, Output = Self>
{
    const ZERO: Self;
    fn is_finite(self) -> bool;
}

impl DynamicsValue for f32 {
    const ZERO: Self = 0.0;

    #[inline]
    fn is_finite(self) -> bool {
        self.is_finite()
    }
}

impl DynamicsValue for glam::Vec3A {
    const ZERO: Self = Self::ZERO;

    #[inline]
    fn is_finite(self) -> bool {
        self.is_finite()
    }
}

#[inline]
pub fn second_order_coefficients(frequency: f32, damping: f32, response: f32) -> (f32, f32, f32) {
    let angular_frequency = core::f32::consts::TAU * frequency;
    (
        damping / (core::f32::consts::PI * frequency),
        1.0 / (angular_frequency * angular_frequency),
        response * damping / angular_frequency,
    )
}

#[inline]
pub fn second_order_step<T: DynamicsValue>(
    previous_target: &mut T,
    value: &mut T,
    velocity: &mut T,
    target: T,
    target_velocity: T,
    frequency: f32,
    damping: f32,
    response: f32,
    delta_seconds: f32,
) {
    let (first, second, third) = second_order_coefficients(frequency, damping, response);
    let stable_second = second
        .max(delta_seconds * delta_seconds * 0.5 + delta_seconds * first * 0.5)
        .max(delta_seconds * first);
    *value = *value + *velocity * delta_seconds;
    *velocity = *velocity
        + (target + target_velocity * third - *value - *velocity * first) * delta_seconds
            / stable_second;

    if !value.is_finite() {
        *previous_target = target;
        *value = target;
        *velocity = T::ZERO;
    }
}

pub fn second_order_advance<T: DynamicsValue>(
    previous_target: &mut T,
    value: &mut T,
    velocity: &mut T,
    lag_seconds: &mut f32,
    target: T,
    frequency: f32,
    damping: f32,
    response: f32,
    delta_seconds: f32,
) -> T {
    const FIXED_DELTA_SECONDS: f32 = 1.0 / (60.0 * 4.0);
    if delta_seconds <= 0.0 {
        return *value;
    }

    if *lag_seconds + delta_seconds <= FIXED_DELTA_SECONDS {
        let target_velocity = (target - *previous_target) / delta_seconds;
        *previous_target = target;
        *lag_seconds = 0.0;
        second_order_step(
            previous_target,
            value,
            velocity,
            target,
            target_velocity,
            frequency,
            damping,
            response,
            delta_seconds,
        );
        return *value;
    }

    *lag_seconds += delta_seconds;
    while *lag_seconds >= FIXED_DELTA_SECONDS {
        let target_velocity = (target - *previous_target) / FIXED_DELTA_SECONDS;
        *previous_target = target;
        *lag_seconds -= FIXED_DELTA_SECONDS;
        second_order_step(
            previous_target,
            value,
            velocity,
            target,
            target_velocity,
            frequency,
            damping,
            response,
            FIXED_DELTA_SECONDS,
        );
    }
    *value
}
