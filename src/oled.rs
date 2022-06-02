use defmt::debug;
use display_interface::DisplayError;
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
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

use crate::event::Event;

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
        self.display.set_brightness(Brightness::BRIGHTEST).await?;
        self.display.init().await?;
        Ok(())
    }

    pub async fn draw(
        &mut self,
        f: impl FnOnce(&mut OledDisplay<'a, T>),
    ) -> Result<(), DisplayError> {
        self.display.clear();
        f(&mut self.display);
        self.display.flush().await?;
        Ok(())
    }

    pub async fn set_on(&mut self) -> Result<(), DisplayError> {
        if self.status {
            return Ok(());
        }

        debug!("Turning display on");

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

        debug!("Turning display off");

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
static INTERACTED_EVENT: Event = Event::new();

pub fn interacted() {
    INTERACTED_EVENT.set();
}

async fn turn_off(oled: &Mutex<ThreadModeRawMutex, Oled<'_, impl Instance>>) {
    Timer::after(OLED_TIMEOUT).await;

    let _ = oled.lock().await.set_off().await;

    turn_on(oled).await;
}

async fn turn_on(oled: &Mutex<ThreadModeRawMutex, Oled<'_, impl Instance>>) {
    INTERACTED_EVENT.wait().await;

    let _ = oled.lock().await.set_on().await;
}

pub async fn display_timeout_task<'a, T: Instance>(oled: &Mutex<ThreadModeRawMutex, Oled<'a, T>>)
where
    Twim<'a, T>: I2c<u8>,
{
    loop {
        select(turn_on(oled), turn_off(oled)).await;
    }
}
