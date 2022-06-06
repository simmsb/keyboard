use bitvec::{order::Lsb0, view::BitView};
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
    mutex::Mutex,
    time::{Duration, Instant, Ticker},
    util::{select, select3},
};
use embassy_nrf::peripherals::TWISPI0;
use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point, Drawable, Pixel};
use futures::StreamExt;

use crate::{event::Event, oled::Oled};

#[derive(defmt::Format)]
pub struct DisplayOverride {
    pub row: u8,
    pub data: [u8; 4],
}

pub static KEYPRESS_EVENT: Event = Event::new();
pub static OVERRIDE_CHAN: Channel<ThreadModeRawMutex, DisplayOverride, 256> = Channel::new();

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
        let mut should_flush = initial.row == 127;
        oled.draw_no_clear_no_flush(|d| {
            for (col, pix) in initial.data.view_bits::<Lsb0>().into_iter().enumerate() {
                let _ = Pixel(
                    Point::new(col as i32, initial.row as i32),
                    BinaryColor::from(*pix),
                )
                .draw(d);
            }

            while let Ok(o) = OVERRIDE_CHAN.try_recv() {
                should_flush ^= o.row == 127;
                for (col, pix) in o.data.view_bits::<Lsb0>().into_iter().enumerate() {
                    let _ = Pixel(
                        Point::new(col as i32, o.row as i32),
                        BinaryColor::from(*pix),
                    )
                    .draw(d);
                }
            }
        });
        if should_flush {
            let _ = oled.flush().await;
        }
    }

    async fn render_normal(&mut self) {
        {
            let _ = self.oled.lock().await.draw(move |d| {}).await;
        }
    }
}
