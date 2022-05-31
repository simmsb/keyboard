use core::sync::atomic::AtomicBool;

use display_interface::DisplayError;
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
    mode::{BufferedGraphicsMode, DisplayConfig},
    prelude::{Brightness, I2CInterface},
    rotation::DisplayRotation,
    size::DisplaySize128x32,
    I2CDisplayInterface, Ssd1306,
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

    pub async fn init(&mut self) -> Result<(), DisplayError> {
        self.display.set_rotation(DisplayRotation::Rotate90).await?;
        self.display.init().await?;
        Ok(())
    }

    pub async fn draw(&mut self, f: impl Fn(&mut OledDisplay<'a, T>)) -> Result<(), DisplayError> {
        self.display.clear();
        f(&mut self.display);
        self.display.flush().await?;
        Ok(())
    }

    pub async fn set_on(&mut self) -> Result<(), DisplayError> {
        if self.status {
            return Ok(());
        }

        defmt::debug!("Turning display on");

        self.display.set_brightness(Brightness::DIMMEST).await?;
        self.display.set_display_on(true).await?;

        for brightness in [
            Brightness::DIM,
            Brightness::NORMAL,
            Brightness::BRIGHT,
            Brightness::BRIGHTEST,
        ] {
            Timer::after(Duration::from_millis(100)).await;
            self.display.set_brightness(brightness).await?;
        }

        self.status = true;

        Ok(())
    }

    pub async fn set_off(&mut self) -> Result<(), DisplayError> {
        if !self.status {
            return Ok(());
        }

        defmt::debug!("Turning display off");

        self.display.set_brightness(Brightness::BRIGHTEST).await?;

        for brightness in [
            Brightness::BRIGHT,
            Brightness::NORMAL,
            Brightness::DIM,
            Brightness::DIMMEST,
        ] {
            Timer::after(Duration::from_millis(100)).await;
            self.display.set_brightness(brightness).await?;
        }

        self.display.set_display_on(false).await?;

        self.status = false;

        Ok(())
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
            let _ = oled.lock().await.set_on().await;
        } else {
            let _ = oled.lock().await.set_off().await;
        }
    }
}
