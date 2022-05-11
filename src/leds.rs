use nrf52840_hal::gpio::{Disconnected, Pin};
use nrf52840_hal::pac::PWM0;
use nrf_smartled::RGB8;
use smart_leds::hsv::hsv2rgb;
use smart_leds::SmartLedsWrite;

pub const UNDERGLOW_LEDS: usize = 6;
pub const SWITCH_LEDS: usize = 21;
pub const TOTAL_LEDS: usize = UNDERGLOW_LEDS + SWITCH_LEDS;

// underglow LEDs are left to right
#[rustfmt::skip]
pub const UNDERGLOW_LED_POSITIONS: [(u8, u8); UNDERGLOW_LEDS] = [
    // top row: 1, 2, 3
    (0, 1), (2, 1), (4, 1),
    // bottom row: 4, 5, 6
    (4, 2), (2, 3), (0, 3),
];

// switch leds are bottom to top
#[rustfmt::skip]
pub const SWITCH_LED_POSITIONS: [(u8, u8); SWITCH_LEDS] = [
    // first column: 7, 8, 9, 10
    (0, 3), (0, 2), (0, 1), (0, 0),
    // second column: 11, 12, 13, 14
    (1, 1), (1, 2), (1, 3), (1, 4),
    // third column: 15, 16, 17, 18
    (2, 3), (2, 2), (2, 1), (2, 0),
    // fourth column: 19, 20, 21
    (3, 0), (3, 1), (3, 2),
    // fifth column: 22, 23, 24
    (4, 2), (4, 1), (4, 0),
    // sixth column: 25, 26, 27
    (5, 0), (5, 1), (5, 2)
];

pub fn colour_gen<F, U>(f: F) -> impl Iterator<Item = U>
where
    F: Fn(u8, u8) -> U,
{
    let buf_a = UNDERGLOW_LED_POSITIONS.map(|(x, y)| f(x, y));
    let buf_b = SWITCH_LED_POSITIONS.map(|(x, y)| f(x, y));
    buf_a.into_iter().chain(buf_b.into_iter())
}

pub fn split_colour_gen<FU, FS, U>(underglow: FU, switches: FS) -> impl Iterator<Item = U>
where
    FU: Fn(u8, u8) -> U,
    FS: Fn(u8, u8) -> U,
{
    let buf_a = UNDERGLOW_LED_POSITIONS.map(|(x, y)| underglow(x, y));
    let buf_b = SWITCH_LED_POSITIONS.map(|(x, y)| switches(x, y));
    buf_a.into_iter().chain(buf_b.into_iter())
}

pub fn rainbow(offset: u8) -> impl Iterator<Item = RGB8> {
    colour_gen(move |x, y| {
        hsv2rgb(smart_leds::hsv::Hsv {
            hue: x.wrapping_mul(6).wrapping_add(y.wrapping_mul(2)).wrapping_add(offset),
            sat: 255,
            val: 25,
        })
    })
}

pub struct Leds {
    pwm: nrf_smartled::pwm::Pwm<PWM0>,
}

impl Leds {
    pub fn new(pwm0: PWM0, pin: Pin<Disconnected>) -> Self {
        Self {
            pwm: nrf_smartled::pwm::Pwm::new(pwm0, pin),
        }
    }

    pub fn send<T, I>(&mut self, iterator: T)
    where
        T: Iterator<Item = I>,
        I: Into<RGB8>,
    {
        let _ = self.pwm.write(iterator);
    }
}
