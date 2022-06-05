use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use bitvec::{order::Lsb0, view::BitView};
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
    mutex::Mutex,
    time::{Duration, Instant, Ticker},
    util::select,
};
use embassy_nrf::peripherals::TWISPI0;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::{Point, Primitive, Size},
    primitives::{Line, PrimitiveStyle, Rectangle},
    Drawable, Pixel,
};
use embedded_text::{style::TextBoxStyleBuilder, TextBox};
use futures::StreamExt;
use micromath::F32Ext;
use ufmt::uwriteln;

use crate::{cpm::SampleBuffer, event::Event, oled::Oled};

pub struct DisplayOverride {
    pub row: u8,
    pub data: [u8; 4],
}

pub static TOTAL_KEYPRESSES: AtomicU32 = AtomicU32::new(0);
pub static AVERAGE_KEYPRESSES: AtomicF32 = AtomicF32::new(0.0);
pub static KEYPRESS_EVENT: Event = Event::new();
pub static OVERRIDE_CHAN: Channel<ThreadModeRawMutex, DisplayOverride, 1> = Channel::new();

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
            if self.read_in_overrides().await {
                override_timeout = Some(Instant::now() + Duration::from_secs(1));
            }

            match override_timeout {
                Some(t) => {
                    if Instant::now() > t {
                        override_timeout = None;
                    }
                }
                None => self.render_normal().await,
            }

            select(Self::wait_for_signal(), self.tick_update()).await;
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

    async fn read_in_overrides(&mut self) -> bool {
        let mut did_read = false;

        while let Ok(o) = OVERRIDE_CHAN.try_recv() {
            did_read = true;

            let _ = self.oled.lock().await.draw(|d| {
                for (col, pix) in o.data.view_bits::<Lsb0>().into_iter().enumerate() {
                    let _ = Pixel(
                        Point::new(o.row as i32, col as i32),
                        BinaryColor::from(*pix),
                    )
                    .draw(d);
                }
            });
        }

        did_read
    }

    async fn render_normal(&mut self) {
        let character_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
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
