#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]

use core::sync::atomic::AtomicU32;

use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::Channel,
    executor::Spawner,
    mutex::Mutex,
    time::{Duration, Timer, Ticker},
    util::{select, Forever},
};
use embassy_nrf::{
    gpio::{AnyPin, Input, Output},
    interrupt, pac,
    peripherals::{self, TWISPI0, UARTE0},
    twim::{self, Twim},
    uarte,
    usb::{self, Driver},
    Peripherals,
};
use embassy_usb::UsbDevice;
use embassy_usb_hid::HidWriter;
use embassy_usb_serial::CdcAcmClass;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X13, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    prelude::Point,
    text::{Text, TextStyleBuilder},
    Drawable,
};
use futures::StreamExt;
use keyberon::{
    chording::Chording, debounce::Debouncer, key_code::KbHidReport, layout::Event, matrix::Matrix,
};
use keyboard_thing::{
    self as _,
    layout::{Layout, COLS_PER_SIDE, ROWS},
    leds::{rainbow_single, Leds, TapWaves},
    messages::{DomToSub, EventReader, EventSender, HostToKeyboard, KeyboardToHost, SubToDom},
    oled::{display_timeout_task, Oled},
};
use postcard::{
    flavors::{Cobs, Slice},
    CobsAccumulator,
};

use ufmt::uwrite;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

static TOTAL_KEYPRESSES: AtomicU32 = AtomicU32::new(0);

static LED_KEY_LISTEN_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();

/// Channels that receive each debounced key press
static KEY_EVENT_CHANS: &[&Channel<ThreadModeRawMutex, Event, 16>] = &[&LED_KEY_LISTEN_CHAN];
/// Channel log messages are put on to be sent to the computer
static LOG_CHAN: Channel<ThreadModeRawMutex, &'static str, 16> = Channel::new();
/// Key events that have been chorded or received from the other side
static PROCESSED_KEY_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();
/// Channel HID events are put on to be sent to the computer
static HID_CHAN: Channel<ThreadModeRawMutex, KbHidReport, 1> = Channel::new();
/// Channel commands are put on to be sent to the other side
static COMMAND_CHAN: Channel<ThreadModeRawMutex, DomToSub, 4> = Channel::new();

#[embassy::main]
async fn main(spawner: Spawner, p: Peripherals) {
    let clock: pac::CLOCK = unsafe { core::mem::transmute(()) };
    let power: pac::POWER = unsafe { core::mem::transmute(()) };

    clock.tasks_hfclkstart.write(|w| unsafe { w.bits(1) });
    while clock.events_hfclkstarted.read().bits() != 1 {}

    while !power.usbregstatus.read().vbusdetect().is_vbus_present() {}

    let mut cortex_p = cortex_m::Peripherals::take().unwrap();
    cortex_p.SCB.enable_icache();

    let irq = interrupt::take!(USBD);
    let usb_driver = usb::Driver::new(p.USBD, irq);

    let mut config = embassy_usb::Config::new(0x6969, 0x0420);
    config.manufacturer.replace("test");
    config.product.replace("test");
    config.serial_number.replace("69420");
    config.max_power = 500;
    config.max_packet_size_0 = 64;

    struct Resources {
        device_descriptor: [u8; 256],
        config_descriptor: [u8; 256],
        bos_descriptor: [u8; 256],
        control_buf: [u8; 128],
        serial_state: embassy_usb_serial::State<'static>,
        usb_state: embassy_usb_hid::State<'static>,
    }
    static RESOURCES: Forever<Resources> = Forever::new();

    let res = RESOURCES.put(Resources {
        device_descriptor: [0; 256],
        config_descriptor: [0; 256],
        bos_descriptor: [0; 256],
        control_buf: [0; 128],
        serial_state: embassy_usb_serial::State::new(),
        usb_state: embassy_usb_hid::State::new(),
    });

    let mut builder = embassy_usb::Builder::new(
        usb_driver,
        config,
        &mut res.device_descriptor,
        &mut res.config_descriptor,
        &mut res.bos_descriptor,
        &mut res.control_buf,
        None,
    );

    let serial_class = CdcAcmClass::new(&mut builder, &mut res.serial_state, 64);

    let hid_config = embassy_usb_hid::Config {
        report_descriptor: KeyboardReport::desc(),
        request_handler: None,
        poll_ms: 1,
        max_packet_size: 64,
    };
    let hid = HidWriter::<_, 8>::new(&mut builder, &mut res.usb_state, hid_config);

    let usb = builder.build();

    defmt::debug!("hello");

    let leds = Leds::new(p.PWM0, p.P0_06);

    let matrix = keyboard_thing::build_matrix!(p);
    let debouncer = Debouncer::new(
        [[false; COLS_PER_SIDE]; ROWS],
        [[false; COLS_PER_SIDE]; ROWS],
        30,
    );
    let chording = Chording::new(&keyboard_thing::layout::CHORDS);

    static LAYOUT: Forever<Mutex<ThreadModeRawMutex, Layout>> = Forever::new();
    let layout = LAYOUT.put(Mutex::new(Layout::new(&keyboard_thing::layout::LAYERS)));

    let mut uart_config = uarte::Config::default();
    uart_config.parity = uarte::Parity::EXCLUDED;
    uart_config.baudrate = uarte::Baudrate::BAUD1M;

    let irq = interrupt::take!(UARTE0_UART0);
    let uart = uarte::Uarte::new(p.UARTE0, irq, p.P1_04, p.P0_08, uart_config);
    let (uart_out, uart_in) = uart.split();
    let events_out = EventSender::new(uart_out);
    let events_in = EventReader::new(uart_in);

    // let irq = interrupt::take!(SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
    // let twim = Twim::new(p.TWISPI0, irq, p.P0_17, p.P0_20, twim::Config::default());
    // static OLED: Forever<Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>> = Forever::new();
    // let oled = OLED.put(Mutex::new(Oled::new(twim)));

    // spawner.spawn(oled_task(oled)).unwrap();
    // spawner.spawn(oled_timeout_task(oled)).unwrap();
    spawner.spawn(usb_task(usb)).unwrap();
    spawner.spawn(usb_serial_task(serial_class)).unwrap();
    spawner.spawn(hid_task(hid)).unwrap();

    spawner.spawn(led_task(leds)).unwrap();
    spawner
        .spawn(keyboard_poll_task(matrix, debouncer, chording))
        .unwrap();
    spawner.spawn(keyboard_event_task(layout)).unwrap();
    spawner.spawn(layout_task(layout)).unwrap();
    spawner.spawn(read_events_task(events_in)).unwrap();
    spawner.spawn(send_events_task(events_out)).unwrap();
    spawner.spawn(startup_task()).unwrap();
}

#[embassy::task]
async fn oled_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    let character_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X13)
        .text_color(BinaryColor::On)
        .build();

    let text_style = TextStyleBuilder::new()
        .alignment(embedded_graphics::text::Alignment::Center)
        .build();

    let mut buf: heapless::String<128> = heapless::String::new();
    let mut n = 0u32;

    loop {
        buf.clear();
        let _ = uwrite!(
            &mut buf,
            "keypresses: {}\nticks: {}",
            TOTAL_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed),
            n
        );
        let text = Text::with_text_style(&buf, Point::new(20, 30), character_style, text_style);

        oled.lock()
            .await
            .draw(|d| {
                let _ = text.draw(d);
            })
            .await;

        n += 1;
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy::task]
async fn oled_timeout_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    display_timeout_task(oled).await;
}

#[embassy::task]
async fn startup_task() {
    Timer::after(Duration::from_millis(100)).await;
    COMMAND_CHAN.send(DomToSub::ResyncLeds).await;
}

#[embassy::task]
async fn send_events_task(mut events_out: EventSender<'static, DomToSub, UARTE0>) {
    loop {
        let evt = COMMAND_CHAN.recv().await;
        let _ = events_out.send(&evt).await;
    }
}

#[embassy::task]
async fn read_events_task(mut events_in: EventReader<'static, SubToDom, UARTE0>) {
    let mut queue: heapless::spsc::Queue<SubToDom, 8> = heapless::spsc::Queue::new();

    loop {
        let _ = events_in.read(&mut queue).await;
        while let Some(event) = queue.dequeue() {
            // defmt::debug!("Got event from rhs: {:?}", event);
            if let Some(event) = event.as_keyberon_event() {
                // events from the other side are already debounced and chord-resolved
                PROCESSED_KEY_CHAN.send(event).await;
            }
        }
    }
}

#[embassy::task]
async fn layout_task(layout: &'static Mutex<ThreadModeRawMutex, Layout>) {
    let mut last_report = None;
    loop {
        {
            let mut layout = layout.lock().await;
            layout.tick();

            let collect: KbHidReport = layout.keycodes().collect();

            if last_report.as_ref() != Some(&collect) {
                defmt::debug!("hid report: {}", collect.as_bytes());
                last_report = Some(collect.clone());
                HID_CHAN.send(collect.clone()).await;
            }
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}

#[embassy::task]
async fn keyboard_event_task(layout: &'static Mutex<ThreadModeRawMutex, Layout>) {
    loop {
        let event = PROCESSED_KEY_CHAN.recv().await;
        {
            let mut layout = layout.lock().await;
            layout.event(event);
            defmt::debug!("evt: press: {} {:?}", event.is_press(), event.coord());
            let mut total = 1;
            while let Ok(event) = PROCESSED_KEY_CHAN.try_recv() {
                defmt::debug!("evt: press: {} {:?}", event.is_press(), event.coord());
                layout.event(event);
                total += 1;
            }
            TOTAL_KEYPRESSES.fetch_add(total, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[embassy::task]
async fn keyboard_poll_task(
    mut matrix: Matrix<Input<'static, AnyPin>, Output<'static, AnyPin>, COLS_PER_SIDE, ROWS>,
    mut debouncer: Debouncer<[[bool; COLS_PER_SIDE]; ROWS]>,
    mut chording: Chording<{ keyboard_thing::layout::NUM_CHORDS }>,
) {
    loop {
        let events = debouncer
            .events(matrix.get().unwrap())
            .collect::<heapless::Vec<_, 16>>();

        for event in &events {
            for chan in KEY_EVENT_CHANS {
                let _ = chan.try_send(*event);
            }
        }

        let events = chording.tick(events);

        for event in events {
            PROCESSED_KEY_CHAN.send(event).await;
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}

#[embassy::task]
async fn led_task(mut leds: Leds) {
    let fps = 30;
    let mut tapwaves = TapWaves::new();
    let mut ticker = Ticker::every(Duration::from_millis(1000 / fps));

    for i in (0..255u8).cycle() {
        while let Ok(event) = LED_KEY_LISTEN_CHAN.try_recv() {
            tapwaves.update(event);
        }

        tapwaves.tick();

        leds.send(tapwaves.render(|x, y| rainbow_single(x, y, i)));

        ticker.next().await;
    }
}

type UsbDriver = Driver<'static, peripherals::USBD>;

#[embassy::task]
async fn hid_task(mut hid: HidWriter<'static, UsbDriver, 8>) {
    loop {
        let report = HID_CHAN.recv().await;
        let _ = hid.write(report.as_bytes()).await;
    }
}

async fn handle_cmd(
    class: &mut CdcAcmClass<'static, UsbDriver>,
    cmd: HostToKeyboard,
) -> Option<()> {
    match cmd {
        HostToKeyboard::RequestStats => {
            let keypresses = TOTAL_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed);
            let msg = KeyboardToHost::Stats { keypresses };

            let mut buf = [0u8; 64];
            let buf =
                postcard::serialize_with_flavor(&msg, Cobs::try_new(Slice::new(&mut buf)).unwrap())
                    .ok()?;

            class.write_packet(&buf).await.ok()?;
        }
    }

    Some(())
}

async fn usb_serial_inner(class: &mut CdcAcmClass<'static, UsbDriver>) -> Option<()> {
    let mut recv = [0u8; 64];
    let mut accumulator = CobsAccumulator::<128>::new();
    loop {
        let r = select(LOG_CHAN.recv(), class.read_packet(&mut recv)).await;
        match r {
            embassy::util::Either::First(log) => {
                for chunk in log.as_bytes().chunks(60) {
                    if let Ok(v) = heapless::Vec::<u8, 60>::from_slice(chunk) {
                        let msg = KeyboardToHost::Log(v);
                        let mut buf = [0u8; 64];
                        let buf = postcard::serialize_with_flavor(
                            &msg,
                            Cobs::try_new(Slice::new(&mut buf)).unwrap(),
                        )
                        .ok()?;

                        class.write_packet(&buf).await.ok()?;
                    }
                }
                class.write_packet(&[]).await.ok()?;
            }
            embassy::util::Either::Second(Ok(n)) => {
                let mut window = &recv[..n];
                'cobs: while !window.is_empty() {
                    window = match accumulator.feed(window) {
                        postcard::FeedResult::Consumed => break 'cobs,
                        postcard::FeedResult::OverFull(buf) => buf,
                        postcard::FeedResult::DeserError(buf) => buf,
                        postcard::FeedResult::Success { data, remaining } => {
                            let _ = handle_cmd(class, data).await;

                            remaining
                        }
                    }
                }
            }
            embassy::util::Either::Second(_) => {}
        }
    }
}

#[embassy::task]
async fn usb_serial_task(mut class: CdcAcmClass<'static, UsbDriver>) {
    loop {
        class.wait_connection().await;
        let _ = usb_serial_inner(&mut class).await;
    }
}

#[embassy::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver>) {
    device.run().await;
}
