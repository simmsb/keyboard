use core::ops::Neg;

use num_traits::{
    Bounded, CheckedNeg, FromPrimitive, Num, NumCast, SaturatingAdd, SaturatingSub, Signed,
    ToPrimitive, Unsigned, WrappingAdd, WrappingSub, Zero,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Copy, Clone, defmt::Format)]
pub struct WrappingID<T>(T);

impl<T> WrappingID<T>
where
    T: Copy
        + Unsigned
        + SaturatingAdd
        + SaturatingSub
        + WrappingAdd
        + WrappingSub
        + Bounded
        + FromPrimitive
        + ToPrimitive
        + PartialOrd
        + HasSigned
        + NumCast,
    <T as HasSigned>::Signed:
        Neg<Output = <T as HasSigned>::Signed> + Signed + CheckedNeg + Bounded + Zero,
{
    pub fn new(val: T) -> Self {
        Self(val)
    }

    pub fn get(self) -> T {
        self.0
    }

    pub fn add(&mut self, other: T::Signed) {
        self.0 = self.0.wrapping_add_signed_(other);
    }

    pub fn inc(&mut self) {
        self.0 = self.0.wrapping_add(&T::one());
    }

    pub fn delta(self, rhs: Self) -> T::Signed {
        let half = T::max_value() / T::from_u8(2).unwrap();

        let lhs = self.0;
        let rhs = rhs.0;

        let d = lhs.wrapping_sub(&rhs);

        if d <= half {
            NumCast::from(d).unwrap()
        } else {
            let x: T::Signed =
                NumCast::from(T::max_value() - (d - T::one())).unwrap_or(T::Signed::zero());
            -x
        }
    }
}

pub trait HasSigned {
    type Signed: NumCast;

    fn wrapping_add_signed_(self, rhs: Self::Signed) -> Self;
}

impl HasSigned for u16 {
    type Signed = i16;

    fn wrapping_add_signed_(self, rhs: Self::Signed) -> Self {
        u16::wrapping_add_signed(self, rhs)
    }
}
