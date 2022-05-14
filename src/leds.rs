use embassy_nrf::{gpio::Pin, peripherals::PWM0, Unborrow};
use keyberon::layout::Event;
use micromath::F32Ext;
use nrf_smartled::RGB8;
use smart_leds::{
    gamma,
    hsv::{hsv2rgb, Hsv},
    SmartLedsWrite,
};

use crate::layout::{COLS_PER_SIDE, ROWS};

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

#[derive(Clone, Copy)]
pub struct Hsl {
    pub hue: u8,
    pub sat: u8,
    pub lum: u8,
}

impl Hsl {
    pub fn to_hsv(self) -> Hsv {
        let val = self.lum + self.sat * self.lum.min(255 - self.lum);
        let sat = if val == 0 {
            0
        } else {
            2 * (255 - (self.lum / val))
        };
        Hsv {
            hue: self.hue,
            sat,
            val,
        }
    }

    pub fn from_hsv(hsv: Hsv) -> Self {
        let lum = hsv.val * (255 - (hsv.sat / 2));
        let sat = if lum == 0 || lum == 255 {
            0
        } else {
            (hsv.val - lum) / lum.min(255 - lum)
        };
        Hsl {
            hue: hsv.hue,
            sat,
            lum,
        }
    }
}

pub fn rainbow_single(x: u8, y: u8, offset: u8) -> Hsv {
    Hsv {
        hue: x
            .wrapping_mul(6)
            .wrapping_add(y.wrapping_mul(2))
            .wrapping_add(offset),
        sat: 255,
        val: 40,
    }
}

pub fn rainbow(offset: u8) -> impl Iterator<Item = RGB8> {
    colour_gen(move |x, y| hsv2rgb(rainbow_single(x, y, offset)))
}

pub struct TapWaves {
    matrix: [[u8; COLS_PER_SIDE]; ROWS],
}

impl TapWaves {
    pub fn new() -> Self {
        Self {
            matrix: Default::default(),
        }
    }

    pub fn update(&mut self, event: Event) {
        if !event.is_press() {
            return;
        }

        let (x, y) = event.coord();
        if let Some(v) = self
            .matrix
            .get_mut(y as usize)
            .and_then(|col| col.get_mut(x as usize))
        {
            *v = 1;
        }

        for v in self.matrix.iter_mut().flatten() {
            if *v == 0 {
                continue;
            }

            *v = v.wrapping_add(1);
        }
    }

    fn brightness_sums(&self, x: u8, y: u8) -> u8 {
        let x = x as f32;
        let y = y as f32;
        let mut brightness = 0f32;

        for (yy, row) in self.matrix.iter().enumerate() {
            let yy = yy as f32;
            for (xx, v) in row.iter().enumerate() {
                let xx = xx as f32;

                // percentage radius of keypress wave as [0, 1]
                let radius = 255.0 / (*v as f32);

                // percentage distance of this led from the origin [0, 1]
                let dist = ((x - xx).powi(2) + (y - yy).powi(2)).sqrt() / 8.0;

                // how close is the led to the current wavefront [0, 1]
                let delta = (radius - dist).abs();

                // we want the curve to be steeper
                let b = delta.powi(4);

                brightness += b;
            }
        }

        (brightness.min(0.0).max(1.0) * 255.0) as u8
    }

    pub fn render<'s, 'a: 's>(&'s self, below: impl Fn(u8, u8) -> Hsl + 'a) -> impl Iterator<Item = RGB8> + 's {
        colour_gen(move |x, y| {
            let mut colour = below(x, y);

            let b = self.brightness_sums(x, y);

            colour.lum = colour.lum.max(b);
            hsv2rgb(colour.to_hsv())
        })
    }
}

pub struct Leds {
    pwm: nrf_smartled::pwm::Pwm<'static, PWM0>,
}

impl Leds {
    pub fn new<P: Pin + Unborrow<Target = P>>(pwm0: PWM0, pin: P) -> Self {
        Self {
            pwm: nrf_smartled::pwm::Pwm::new(pwm0, pin),
        }
    }

    pub fn send<T, I>(&mut self, iterator: T)
    where
        T: Iterator<Item = I>,
        I: Into<RGB8>,
    {
        // critical_section::with(|_| {
        let _ = self.pwm.write(gamma(iterator.map(Into::into)));
        // });
    }
}
