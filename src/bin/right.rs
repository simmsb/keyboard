#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::{AtomicU32, AtomicU8};

use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
    executor::Spawner,
    mutex::Mutex,
    time::{Duration, Timer, Ticker},
    util::Forever,
};
use embassy_nrf::{
    gpio::{AnyPin, Input, Output},
    interrupt,
    peripherals::{TWISPI0, UARTE0},
    twim::{self, Twim},
    uarte, Peripherals,
};
use embedded_graphics::{
    mono_font::{ascii::FONT_6X13, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    prelude::Point,
    text::{Text, TextStyleBuilder},
    Drawable,
};
use futures::StreamExt;
use keyberon::{chording::Chording, debounce::Debouncer, layout::Event, matrix::Matrix};
use keyboard_thing::{
    self as _,
    leds::{rainbow_single, Leds, TapWaves},
    messages::{DomToSub, EventReader, EventSender, SubToDom},
    oled::{display_timeout_task, Oled, interacted},
};
use ufmt::uwrite;

static TOTAL_KEYPRESSES: AtomicU32 = AtomicU32::new(0);

static LED_KEY_LISTEN_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();
/// Channels that receive each debounced key press
static KEY_EVENT_CHANS: &[&Channel<ThreadModeRawMutex, Event, 16>] = &[&LED_KEY_LISTEN_CHAN];
/// Channel commands are put on to be sent to the other side
static COMMAND_CHAN: Channel<ThreadModeRawMutex, SubToDom, 4> = Channel::new();

static LED_COUNTER: AtomicU8 = AtomicU8::new(0);

#[embassy::main]
async fn main(spawner: Spawner, p: Peripherals) {
    let mut cortex_p = cortex_m::Peripherals::take().unwrap();
    cortex_p.SCB.enable_icache();

    let leds = Leds::new(p.PWM0, p.P0_06);

    let matrix = keyboard_thing::build_matrix!(p);
    let debouncer = Debouncer::new([[false; 6]; 4], [[false; 6]; 4], 30);
    let chording = Chording::new(&keyboard_thing::layout::CHORDS);

    let mut uart_config = uarte::Config::default();
    uart_config.parity = uarte::Parity::EXCLUDED;
    uart_config.baudrate = uarte::Baudrate::BAUD1M;

    let irq = interrupt::take!(UARTE0_UART0);
    let uart = uarte::Uarte::new(p.UARTE0, irq, p.P0_08, p.P1_04, uart_config);
    let (uart_out, uart_in) = uart.split();
    let events_out = EventSender::new(uart_out);
    let events_in = EventReader::new(uart_in);

    let irq = interrupt::take!(SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
    let twim = Twim::new(p.TWISPI0, irq, p.P0_17, p.P0_20, twim::Config::default());
    static OLED: Forever<Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>> = Forever::new();
    let oled = OLED.put(Mutex::new(Oled::new(twim)));

    spawner.spawn(oled_task(oled)).unwrap();
    spawner.spawn(oled_timeout_task(oled)).unwrap();
    spawner.spawn(led_task(leds)).unwrap();
    spawner
        .spawn(keyboard_poll_task(matrix, debouncer, chording))
        .unwrap();
    spawner.spawn(read_events_task(events_in)).unwrap();
    spawner.spawn(send_events_task(events_out)).unwrap();
}

fn log_e(r: Result<(), display_interface::DisplayError>) {
    use display_interface::DisplayError::*;
    match r {
        Ok(_) => {},
        Err(InvalidFormatError) => defmt::debug!("Invalid format"),
        Err(BusWriteError) => defmt::debug!("bus write error"),
        Err(DCError) => defmt::debug!("dc error"),
        Err(CSError) => defmt::debug!("cs error"),
        Err(DataFormatNotImplemented) => defmt::debug!("not impl"),
        Err(RSError) => defmt::debug!("rs error"),
        Err(OutOfBoundsError) => defmt::debug!("oob"),
        Err(_) => defmt::debug!("other error"),
    }
}

#[embassy::task]
async fn oled_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    let character_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X13)
        .text_color(BinaryColor::On)
        .build();

    let text_style = TextStyleBuilder::new()
        .alignment(embedded_graphics::text::Alignment::Left)
        .build();

    let mut buf: heapless::String<128> = heapless::String::new();
    let mut n = 0u32;

    log_e(oled.lock().await.init().await);

    loop {
        buf.clear();
        let _ = uwrite!(
            &mut buf,
            "keypresses: {}\nticks: {}",
            TOTAL_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed),
            n
        );
        let text = Text::with_text_style(&buf, Point::new(0, 15), character_style, text_style);
        defmt::debug!("about to write to oled");
        log_e(oled.lock()
            .await
            .draw(|d| {
                let _ = text.draw(d);
            })
            .await);
        defmt::debug!("written to oled");

        n += 1;
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy::task]
async fn oled_timeout_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    display_timeout_task(oled).await;
}

#[embassy::task]
async fn send_events_task(mut events_out: EventSender<'static, SubToDom, UARTE0>) {
    loop {
        let evt = COMMAND_CHAN.recv().await;
        let _ = events_out.send(&evt).await;
    }
}

#[embassy::task]
async fn read_events_task(mut events_in: EventReader<'static, DomToSub, UARTE0>) {
    let mut queue: heapless::spsc::Queue<DomToSub, 8> = heapless::spsc::Queue::new();

    loop {
        let _ = events_in.read(&mut queue).await;
        while let Some(event) = queue.dequeue() {
            match event {
                DomToSub::ResyncLeds => {
                    LED_COUNTER.store(0, core::sync::atomic::Ordering::SeqCst);
                }
            }
        }
    }
}

#[embassy::task]
async fn keyboard_poll_task(
    mut matrix: Matrix<Input<'static, AnyPin>, Output<'static, AnyPin>, 6, 4>,
    mut debouncer: Debouncer<[[bool; 6]; 4]>,
    mut chording: Chording<{ keyboard_thing::layout::NUM_CHORDS }>,
) {
    loop {
        let events = debouncer
            .events(matrix.get().unwrap())
            .map(|e| e.transform(|x, y| (x, 11 - y)))
            .collect::<heapless::Vec<_, 16>>();

        if !events.is_empty() {
            interacted();
        }

        for event in &events {
            for chan in KEY_EVENT_CHANS {
                let _ = chan.try_send(event.transform(|x, y| (x, 11 - y)));
            }
        }

        let events = chording.tick(events);

        for event in events {
            let msg = match event {
                keyberon::layout::Event::Press(x, y) => SubToDom::KeyPressed(x, y),
                keyberon::layout::Event::Release(x, y) => SubToDom::KeyReleased(x, y),
            };
            COMMAND_CHAN.send(msg).await;
            if event.is_press() {
                TOTAL_KEYPRESSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}

#[embassy::task]
async fn led_task(mut leds: Leds) {
    let fps = 30;
    let mut tapwaves = TapWaves::new();
    let mut ticker = Ticker::every(Duration::from_millis(1000 / fps));

    loop {
        while let Ok(event) = LED_KEY_LISTEN_CHAN.try_recv() {
            tapwaves.update(event);
        }

        tapwaves.tick();

        let i = LED_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Release);
        leds.send(tapwaves.render(|x, y| rainbow_single(x, y, i)));

        ticker.next().await;
    }
}
