use core::sync::atomic::AtomicU32;

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
    prelude::{Point, Size},
    primitives::Rectangle, Drawable,
};
use embedded_text::{style::TextBoxStyleBuilder, TextBox};
use futures::StreamExt;
use ufmt::uwriteln;

use crate::oled::Oled;

pub static TOTAL_KEYPRESSES: AtomicU32 = AtomicU32::new(0);
pub static KEYPRESS_SIGNAL: Signal<()> = Signal::new();

pub struct RHSDisplay {
    oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>,
    ticker: Ticker,
    buf: heapless::String<128>,
    ticks: u32,
}

impl RHSDisplay {
    pub fn new(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) -> Self {
        Self {
            oled,
            ticker: Ticker::every(Duration::from_secs(1)),
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
            let _ = uwriteln!(&mut self.buf, "kp:");
            let _ = uwriteln!(&mut self.buf, "{}", kp);
            let _ = uwriteln!(&mut self.buf, "tick:");
            let _ = uwriteln!(&mut self.buf, "{}", self.ticks);

            let text_box =
                TextBox::with_textbox_style(&self.buf, bounds, character_style, textbox_style);

            let _ = self
                .oled
                .lock()
                .await
                .draw(|d| {
                    let _ = text_box.draw(d);
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
        self.ticker.next().await;
        self.ticks += 1;
    }
}
