use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use bitvec::{order::Lsb0, view::BitView};
use defmt::info;
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
    mutex::Mutex,
    time::{Duration, Instant, Ticker},
    util::select3,
};
use embassy_nrf::peripherals::TWISPI0;
use embedded_graphics::{
    draw_target::DrawTarget, pixelcolor::BinaryColor, prelude::Point, Drawable, Pixel,
};
use futures::StreamExt;

use crate::{event::Event, oled::Oled};

#[derive(defmt::Format)]
pub struct DisplayOverride {
    pub row: u8,
    pub data_0: [u8; 4],
    pub data_1: [u8; 4],
}

pub static TOTAL_KEYPRESSES: AtomicU32 = AtomicU32::new(0);
pub static AVERAGE_KEYPRESSES: AtomicF32 = AtomicF32::new(0.0);
pub static KEYPRESS_EVENT: Event = Event::new();
pub static OVERRIDE_CHAN: Channel<ThreadModeRawMutex, DisplayOverride, 256> = Channel::new();

type BongoImage = &'static [(u8, &'static [(u8, bool)])];

static BONGO_BASE: BongoImage = include!(concat!(env!("OUT_DIR"), "/base.rs"));
static PAW_LEFT_UP: &[(u8, &[(u8, bool)])] = include!(concat!(env!("OUT_DIR"), "/left_paw_up.rs"));
static PAW_LEFT_DOWN: &[(u8, &[(u8, bool)])] =
    include!(concat!(env!("OUT_DIR"), "/left_paw_down.rs"));
static PAW_RIGHT_UP: &[(u8, &[(u8, bool)])] =
    include!(concat!(env!("OUT_DIR"), "/right_paw_up.rs"));
static PAW_RIGHT_DOWN: &[(u8, &[(u8, bool)])] =
    include!(concat!(env!("OUT_DIR"), "/right_paw_down.rs"));

#[inline]
fn bongo_pixels(data: BongoImage) -> impl Iterator<Item = Pixel<BinaryColor>> {
    data.iter().copied().flat_map(|(y, row)| {
        row.iter()
            .copied()
            .map(move |(x, on)| Pixel(Point::new(x as i32, y as i32), BinaryColor::from(on)))
    })
}

enum BongoState {
    BothUp,
    LeftDown,
    RightDown,
    BothDown,
}

impl BongoState {
    fn next(&self, cps: f32) -> BongoState {
        if cps < 0.1 {
            match self {
                BongoState::BothUp => Self::LeftDown,
                BongoState::LeftDown => Self::RightDown,
                BongoState::RightDown => Self::BothUp,
                BongoState::BothDown => Self::BothUp,
            }
        } else if cps < 3.0 {
            match self {
                BongoState::BothUp => Self::LeftDown,
                BongoState::LeftDown => Self::RightDown,
                BongoState::RightDown => Self::LeftDown,
                BongoState::BothDown => Self::LeftDown,
            }
        } else {
            match self {
                BongoState::BothUp => Self::BothDown,
                BongoState::LeftDown => Self::BothDown,
                BongoState::RightDown => Self::BothDown,
                BongoState::BothDown => Self::BothUp,
            }
        }
    }

    fn images(&self) -> (BongoImage, BongoImage) {
        match self {
            BongoState::BothUp => (PAW_LEFT_UP, PAW_RIGHT_UP),
            BongoState::LeftDown => (PAW_LEFT_DOWN, PAW_RIGHT_UP),
            BongoState::RightDown => (PAW_LEFT_UP, PAW_RIGHT_DOWN),
            BongoState::BothDown => (PAW_LEFT_DOWN, PAW_RIGHT_DOWN),
        }
    }
}

pub struct LHSDisplay {
    oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>,
    sec_ticker: Ticker,
    // buf: heapless::String<128>,
    ticks: u32,
    bongo_state: BongoState,
}

impl LHSDisplay {
    pub fn new(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) -> Self {
        Self {
            oled,
            sec_ticker: Ticker::every(Duration::from_secs(1)),
            // buf: Default::default(),
            ticks: 0,
            bongo_state: BongoState::BothUp,
        }
    }

    pub async fn run(&mut self) {
        let mut override_timeout: Option<Instant> = None;

        loop {
            match override_timeout {
                Some(t) => {
                    if Instant::now() > t {
                        override_timeout = None;
                    }
                }
                None => self.render_normal().await,
            }

            match select3(
                Self::wait_for_signal(),
                self.tick_update(),
                OVERRIDE_CHAN.recv(),
            )
            .await
            {
                embassy::util::Either3::First(()) => {
                    self.update_bongo();
                }
                embassy::util::Either3::Second(()) => {
                    self.update_bongo();
                }
                embassy::util::Either3::Third(o) => {
                    self.read_in_overrides(o).await;
                    override_timeout = Some(Instant::now() + Duration::from_secs(1));
                }
            };
        }
    }

    fn update_bongo(&mut self) {
        self.bongo_state = self
            .bongo_state
            .next(AVERAGE_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed));
    }

    async fn wait_for_signal() {
        KEYPRESS_EVENT.wait().await;
    }

    async fn tick_update(&mut self) {
        self.sec_ticker.next().await;
        self.ticks = self.ticks.wrapping_add(1);
    }

    async fn read_in_overrides(&mut self, initial: DisplayOverride) {
        let mut oled = self.oled.lock().await;
        let mut should_flush = initial.row >= 126;
        oled.draw_no_clear_no_flush(|d| {
            for (col, pix) in initial.data_0.view_bits::<Lsb0>().into_iter().enumerate() {
                let _ = Pixel(
                    Point::new(col as i32, initial.row as i32),
                    BinaryColor::from(*pix),
                )
                .draw(d);
            }

            for (col, pix) in initial.data_1.view_bits::<Lsb0>().into_iter().enumerate() {
                let _ = Pixel(
                    Point::new(col as i32, 1 + initial.row as i32),
                    BinaryColor::from(*pix),
                )
                .draw(d);
            }

            while let Ok(o) = OVERRIDE_CHAN.try_recv() {
                should_flush ^= o.row >= 126;
                for (col, pix) in o.data_0.view_bits::<Lsb0>().into_iter().enumerate() {
                    let _ = Pixel(
                        Point::new(col as i32, o.row as i32),
                        BinaryColor::from(*pix),
                    )
                    .draw(d);
                }

                for (col, pix) in o.data_1.view_bits::<Lsb0>().into_iter().enumerate() {
                    let _ = Pixel(
                        Point::new(col as i32, 1 + o.row as i32),
                        BinaryColor::from(*pix),
                    )
                    .draw(d);
                }
            }
        });
        if should_flush {
            let now = Instant::now();
            let _ = oled.flush().await;
            let flush_time = now.elapsed();
            info!("Flush time: {}ms", flush_time.as_millis());
        }
    }

    async fn render_normal(&mut self) {
        let (left_paw, right_paw) = self.bongo_state.images();

        {
            let _ = self
                .oled
                .lock()
                .await
                .draw(move |d| {
                    let _ = d.draw_iter(bongo_pixels(BONGO_BASE));
                    let _ = d.draw_iter(bongo_pixels(left_paw));
                    let _ = d.draw_iter(bongo_pixels(right_paw));
                })
                .await;
        }
    }
}
