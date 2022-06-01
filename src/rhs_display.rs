use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Signal,
    mutex::Mutex,
    time::{Duration, Ticker},
    util::select,
};
use embassy_nrf::peripherals::TWISPI0;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::{Point, Primitive, Size},
    primitives::{Line, PrimitiveStyle, Rectangle},
    Drawable,
};
use embedded_text::{style::TextBoxStyleBuilder, TextBox};
use futures::StreamExt;
use micromath::F32Ext;
use ufmt::uwriteln;

use crate::{cpm::SampleBuffer, oled::Oled};

pub static TOTAL_KEYPRESSES: AtomicU32 = AtomicU32::new(0);
pub static AVERAGE_KEYPRESSES: AtomicF32 = AtomicF32::new(0.0);
pub static KEYPRESS_SIGNAL: Signal<()> = Signal::new();

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
            upd_ticker: Ticker::every(Duration::from_millis(32)),
            buf: Default::default(),
            ticks: 0,
        }
    }

    pub async fn run(&mut self) {
        let character_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        let textbox_style = TextBoxStyleBuilder::new()
            .height_mode(embedded_text::style::HeightMode::FitToText)
            .alignment(embedded_text::alignment::HorizontalAlignment::Justified)
            .paragraph_spacing(6)
            .build();

        let bounds = Rectangle::new(Point::zero(), Size::new(32, 0));

        loop {
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

            select(Self::wait_for_signal(), self.tick_update()).await;
        }
    }

    async fn wait_for_signal() {
        KEYPRESS_SIGNAL.wait().await;
        KEYPRESS_SIGNAL.reset();
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
}
