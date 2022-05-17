use core::sync::atomic::AtomicBool;

use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Signal,
    mutex::Mutex,
    time::{Duration, Timer},
    util::select,
};
use embassy_nrf::twim::{Instance, Twim};
use embedded_hal_async::i2c::I2c;
use ssd1306::{
    mode::BufferedGraphicsMode, prelude::I2CInterface, rotation::DisplayRotation,
    size::DisplaySize128x32, I2CDisplayInterface, Ssd1306,
};

type OledDisplay<'a, T> =
    Ssd1306<I2CInterface<Twim<'a, T>>, DisplaySize128x32, BufferedGraphicsMode<DisplaySize128x32>>;

pub struct Oled<'a, T: Instance> {
    status: bool,
    display: OledDisplay<'a, T>,
}

impl<'a, T: Instance> Oled<'a, T> {
    pub fn new(twim: Twim<'a, T>) -> Self {
        let i2c = I2CDisplayInterface::new(twim);
        let display = Ssd1306::new(i2c, DisplaySize128x32, DisplayRotation::Rotate0)
            .into_buffered_graphics_mode();
        Self {
            status: true,
            display,
        }
    }

    pub async fn draw(&mut self, f: impl Fn(&mut OledDisplay<'a, T>)) {
        f(&mut self.display);
        let _ = self.display.flush().await;
    }

    async fn set_on(&mut self) {
        if self.status {
            return;
        }

        let _ = self.display.set_display_on(true).await;
        self.status = true;
    }

    async fn set_off(&mut self) {
        if !self.status {
            return;
        }

        let _ = self.display.set_display_on(false).await;
        self.status = false;
    }
}

pub const OLED_TIMEOUT: Duration = Duration::from_secs(30);
pub static INTERACTED: AtomicBool = AtomicBool::new(true);
pub static INTERACTED_SIG: Signal<()> = Signal::new();

pub fn interacted() {
    INTERACTED.store(true, core::sync::atomic::Ordering::Relaxed);
    INTERACTED_SIG.signal(());
}

async fn set_noninteracted() {
    Timer::after(OLED_TIMEOUT).await;
    INTERACTED.store(false, core::sync::atomic::Ordering::SeqCst);
}

pub async fn display_timeout_task<'a, T: Instance>(oled: &Mutex<ThreadModeRawMutex, Oled<'a, T>>)
where
    Twim<'a, T>: I2c<u8>,
{
    loop {
        select(set_noninteracted(), INTERACTED_SIG.wait()).await;
        INTERACTED_SIG.reset();

        if INTERACTED.load(core::sync::atomic::Ordering::Relaxed) {
            oled.lock().await.set_on().await;
        } else {
            oled.lock().await.set_off().await;
        }
    }
}
