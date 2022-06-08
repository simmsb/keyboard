use core::sync::atomic::AtomicU8;

use bitvec::{order::Lsb0, view::BitView};
use defmt::{debug, info};
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
    mutex::Mutex,
    time::{Duration, Instant, Ticker},
    util::{select, select3},
};
use embassy_nrf::peripherals::TWISPI0;
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Point, Primitive, Transform},
    primitives::{Line, Polyline, PrimitiveStyle},
    Drawable, Pixel,
};
use futures::StreamExt;
use micromath::F32Ext;

use crate::{event::Event, oled::Oled};

#[derive(defmt::Format)]
pub struct DisplayOverride {
    pub row: u8,
    pub data_0: [u8; 4],
    pub data_1: [u8; 4],
}

pub static KEYPRESS_EVENT: Event = Event::new();
pub static OVERRIDE_CHAN: Channel<ThreadModeRawMutex, DisplayOverride, 256> = Channel::new();

static BONGO_BASE: &[&[Point]] = include!(concat!(env!("OUT_DIR"), "/base.rs"));
static PAW_LEFT_UP: &[&[Point]] = include!(concat!(env!("OUT_DIR"), "/paw_left_up.rs"));
static PAW_LEFT_DOWN: &[&[Point]] = include!(concat!(env!("OUT_DIR"), "/paw_left_down.rs"));
static PAW_RIGHT_UP: &[&[Point]] = include!(concat!(env!("OUT_DIR"), "/paw_right_up.rs"));
static PAW_RIGHT_DOWN: &[&[Point]] = include!(concat!(env!("OUT_DIR"), "/paw_right_down.rs"));

pub struct LHSDisplay {
    oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>,
    sec_ticker: Ticker,
    upd_ticker: Ticker,
    buf: heapless::String<128>,
    ticks: u32,
}

impl LHSDisplay {
    pub fn new(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) -> Self {
        Self {
            oled,
            sec_ticker: Ticker::every(Duration::from_secs(1)),
            upd_ticker: Ticker::every(Duration::from_millis(100)),
            buf: Default::default(),
            ticks: 0,
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
                embassy::util::Either3::First(()) => {}
                embassy::util::Either3::Second(()) => {}
                embassy::util::Either3::Third(o) => {
                    self.read_in_overrides(o).await;
                    override_timeout = Some(Instant::now() + Duration::from_secs(1));
                }
            };
        }
    }

    async fn wait_for_signal() {
        KEYPRESS_EVENT.wait().await;
    }

    async fn tick_update(&mut self) {
        let a = async {
            self.sec_ticker.next().await;
            self.ticks += 1;
        };
        let b = async {
            self.upd_ticker.next().await;
        };
        select(a, b).await;
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
        static OFFSET: AtomicU8 = AtomicU8::new(0);

        let base_lines = BONGO_BASE.iter().map(|path| {
            Polyline::new(path)
                // .translate(Point::new(
                //     - (OFFSET.fetch_add(1, core::sync::atomic::Ordering::Relaxed) as i32 / 2),
                //     0,
                // ))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        });

        let offset = OFFSET.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let (left_paw, left_paw_offs, right_paw, right_paw_offs) = match offset % 3u8 {
            0 => (PAW_LEFT_UP, 0, PAW_RIGHT_UP, 0),
            1 => (PAW_LEFT_DOWN, 3, PAW_RIGHT_UP, 0),
            2 => (PAW_LEFT_UP, 0, PAW_RIGHT_DOWN, 3),
            _ => unreachable!(),
        };

        let left_paw_lines = left_paw.iter().map(|path| {
            Polyline::new(path)
                .translate(Point::new(2, 5 + left_paw_offs))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        });

        let right_paw_lines = right_paw.iter().map(|path| {
            Polyline::new(path)
                .translate(Point::new(23, 10 + right_paw_offs))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
        });

        {
            let _ = self
                .oled
                .lock()
                .await
                .draw(move |d| {
                    for line in base_lines {
                        let _ = line.draw(d);
                    }
                    for line in left_paw_lines {
                        let _ = line.draw(d);
                    }
                    for line in right_paw_lines {
                        let _ = line.draw(d);
                    }
                })
                .await;
        }
    }
}
