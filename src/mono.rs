// Taken from: https://github.com/kalkyl/nrf-play/blob/main/src/mono.rs
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:

// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use nrf52840_hal::pac::{timer0, TIMER0, TIMER1, TIMER2};
use rtic::Monotonic;

pub struct MonoTimer<T: Instance32>(T);
pub type Instant = fugit::TimerInstantU32<1_000_000>;
pub type Duration = fugit::TimerDurationU32<1_000_000>;

impl<T: Instance32> MonoTimer<T> {
    pub fn new(timer: T) -> Self {
        timer.prescaler.write(|w| unsafe { w.prescaler().bits(4) });
        timer.bitmode.write(|w| w.bitmode()._32bit());
        Self(timer)
    }
}

impl<T: Instance32> Monotonic for MonoTimer<T> {
    type Instant = Instant;
    type Duration = Duration;

    unsafe fn reset(&mut self) {
        self.0.intenset.modify(|_, w| w.compare0().set());
        self.0.tasks_clear.write(|w| w.bits(1));
        self.0.tasks_start.write(|w| w.bits(1));
    }

    #[inline(always)]
    fn now(&mut self) -> Self::Instant {
        self.0.tasks_capture[1].write(|w| unsafe { w.bits(1) });
        Self::Instant::from_ticks(self.0.cc[1].read().bits())
    }

    fn set_compare(&mut self, instant: Self::Instant) {
        self.0.cc[0].write(|w| unsafe { w.cc().bits(instant.duration_since_epoch().ticks()) });
    }

    fn clear_compare_flag(&mut self) {
        self.0.events_compare[0].write(|w| w);
    }

    #[inline(always)]
    fn zero() -> Self::Instant {
        Self::Instant::from_ticks(0)
    }
}

pub trait Instance32: core::ops::Deref<Target = timer0::RegisterBlock> {}
impl Instance32 for TIMER0 {}
impl Instance32 for TIMER1 {}
impl Instance32 for TIMER2 {}
