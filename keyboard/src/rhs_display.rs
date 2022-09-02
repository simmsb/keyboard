use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use bitvec::{order::Lsb0, view::BitView};
use embassy_futures::select::{select, select3, Either3};
use embassy_nrf::peripherals::TWISPI0;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel, mutex::Mutex};
use embassy_time::{Duration, Instant, Ticker};
use embedded_graphics::{
    mono_font::MonoTextStyle,
    pixelcolor::BinaryColor,
    prelude::{Point, Primitive, Size},
    primitives::{Line, PrimitiveStyle, Rectangle},
    Drawable, Pixel,
};
use embedded_text::{style::TextBoxStyleBuilder, TextBox};
use futures::StreamExt;
use micromath::F32Ext;
use profont::PROFONT_9_POINT;
use ufmt::uwriteln;

use crate::{cps::SampleBuffer, event::Event, oled::Oled};

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

pub struct RHSDisplay {
    oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>,
    sample_buffer: &'static Mutex<ThreadModeRawMutex, SampleBuffer>,
    sec_ticker: Ticker,
    upd_ticker: Ticker,
    buf: heapless::String<128>,
    ticks: u32,
}

impl RHSDisplay {
    pub fn new(
        oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>,
        sample_buffer: &'static Mutex<ThreadModeRawMutex, SampleBuffer>,
    ) -> Self {
        Self {
            oled,
            sample_buffer,
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
                Either3::First(()) => {}
                Either3::Second(()) => {}
                Either3::Third(o) => {
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
            let _ = oled.flush().await;
        }
    }

    async fn render_normal(&mut self) {
        let character_style = MonoTextStyle::new(&PROFONT_9_POINT, BinaryColor::On);
        let textbox_style = TextBoxStyleBuilder::new()
            .height_mode(embedded_text::style::HeightMode::FitToText)
            .alignment(embedded_text::alignment::HorizontalAlignment::Justified)
            .paragraph_spacing(6)
            .build();

        let bounds = Rectangle::new(Point::zero(), Size::new(32, 0));

        self.buf.clear();

        let kp = TOTAL_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed);
        let cps = AVERAGE_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed);
        let cps = f32::trunc(cps * 10.0) / 10.0;
        let mut fp_buf = dtoa::Buffer::new();
        let cps = fp_buf.format_finite(cps);

        let _ = uwriteln!(&mut self.buf, "kp:");
        let _ = uwriteln!(&mut self.buf, "{}", kp);
        let _ = uwriteln!(&mut self.buf, "cps:");
        let _ = uwriteln!(&mut self.buf, "{}/s", cps);
        let _ = uwriteln!(&mut self.buf, "tick:");
        let _ = uwriteln!(&mut self.buf, "{}", self.ticks);

        let text_box =
            TextBox::with_textbox_style(&self.buf, bounds, character_style, textbox_style);

        let lines = {
            let samples = self.sample_buffer.lock().await;
            samples
                .oldest_ordered()
                .enumerate()
                .map(|(idx, height)| {
                    Line::new(
                        Point::new(idx as i32, 128 - (*height as i32).clamp(0, 16)),
                        Point::new(idx as i32, 128),
                    )
                    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                })
                .collect::<heapless::Vec<_, 32>>()
        };

        {
            let _ = self
                .oled
                .lock()
                .await
                .draw(move |d| {
                    let _ = text_box.draw(d);
                    for line in lines {
                        let _ = line.draw(d);
                    }
                })
                .await;
        }
    }
}
