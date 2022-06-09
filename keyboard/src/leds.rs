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
    (3, 5), (2, 5), (1, 5), (0, 5),
    // second column: 11, 12, 13, 14
    (0, 4), (1, 4), (2, 4), (3, 4),
    // third column: 15, 16, 17, 18
    (3, 3), (2, 3), (1, 3), (0, 3),
    // fourth column: 19, 20, 21
    (0, 2), (1, 2), (2, 2),
    // fifth column: 22, 23, 24
    (2, 1), (1, 1), (0, 1),
    // sixth column: 25, 26, 27
    (0, 0), (1, 0), (2, 0)
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

pub fn rainbow_single(x: u8, y: u8, offset: u8) -> Hsv {
    Hsv {
        hue: x
            .wrapping_mul(6)
            .wrapping_add(y.wrapping_mul(2))
            .wrapping_add(offset),
        sat: 255,
        val: 127,
    }
}

pub fn rainbow(offset: u8) -> impl Iterator<Item = RGB8> {
    colour_gen(move |x, y| hsv2rgb(rainbow_single(x, y, offset)))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    (1.0 - t) * a + t * b
}

fn dmod(a: f32, m: f32) -> f32 {
    a - m * (a / m).floor()
}

fn lerp_wrap(a: f32, b: f32, m: f32, t: f32) -> f32 {
    let b_prime = [b - m, b, b + m]
        .into_iter()
        .min_by(|x, y| (a - *x).abs().total_cmp(&(a - *y).abs()))
        .unwrap();
    dmod(lerp(a, b_prime, t), m)
}

fn c_f(x: f32) -> u8 {
    (x * 255.0) as u8
}

fn c_b(x: u8) -> f32 {
    (x as f32) / 255.0
}

fn blend_hsv(a: Hsv, b: Hsv, t: f32) -> Hsv {
    Hsv {
        hue: c_f(lerp_wrap(c_b(a.hue), c_b(b.hue), 1.0, t)),
        sat: c_f(lerp(c_b(a.sat), c_b(b.sat), t)),
        val: c_f(lerp(c_b(a.val), c_b(b.val), t)),
    }
}

#[derive(Default)]
pub struct TapWaves {
    matrix: [[u8; ROWS]; COLS_PER_SIDE],
}

impl TapWaves {
    pub fn new() -> Self {
        Self {
            matrix: Default::default(),
        }
    }

    pub fn tick(&mut self) {
        for v in self.matrix.iter_mut().flatten() {
            if *v == 255 {
                *v = 0;
            }

            if *v == 0 {
                continue;
            }

            *v = v.saturating_add(15);
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
    }

    fn brightness_sums(&self, x: u8, y: u8) -> f32 {
        let x = x as f32;
        let y = y as f32;
        let mut brightness = 0f32;

        for (yy, row) in self.matrix.iter().enumerate() {
            let yy = yy as f32;
            for (xx, v) in row.iter().enumerate() {
                let xx = xx as f32;
                if *v == 0 {
                    continue;
                }

                // percentage radius of keypress wave as [0, 2]
                let radius = (*v as f32) / 127.0;

                // percentage distance of this led from the origin [0, 1]
                let dist = ((x - xx).powi(2) + (y - yy).powi(2)).sqrt() / 8.0;

                // how close is the led to the current wavefront [0, 1]
                let delta = (dist - radius).abs();

                // calculate the brightness
                let b = (1.0 - delta).clamp(0.0, 1.0).powi(4);

                brightness += b;
            }
        }

        brightness.clamp(0.0, 1.0)
    }

    pub fn render<'s, 'a: 's>(
        &'s self,
        below: impl Fn(u8, u8) -> Hsv + 'a,
    ) -> impl Iterator<Item = RGB8> + 's {
        colour_gen(move |x, y| {
            let colour = below(x, y);

            let b = self.brightness_sums(x, y);

            let white = Hsv {
                hue: 0,
                sat: 0,
                val: 255,
            };
            let colour_out = blend_hsv(colour, white, b);
            // defmt::debug!("in: {:?}, out: {:?}, b: {}", components(colour), components(colour_out), b);

            hsv2rgb(colour_out)
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
        critical_section::with(|_| {
            let _ = self.pwm.write(gamma(iterator.map(Into::into)));
        });
    }
}
