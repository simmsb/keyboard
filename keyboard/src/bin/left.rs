#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]

use core::sync::atomic::AtomicU32;

use defmt::debug;
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::mpmc::{Channel, Receiver},
    executor::Spawner,
    mutex::Mutex,
    time::{Duration, Ticker, Timer},
    util::select3,
};
use embassy_nrf::{
    gpio::{AnyPin, Input, Output},
    interrupt, pac,
    peripherals::{self, TWISPI0, UARTE0},
    twim::{self, Twim},
    uarte::{self, UarteRx, UarteTx},
    usb::{self, Driver, PowerUsb},
    Peripherals,
};
use embassy_usb::UsbDevice;
use embassy_usb_hid::HidWriter;
use embassy_usb_serial::CdcAcmClass;
use futures::{Future, StreamExt};
use keyberon::{chording::Chording, debounce::Debouncer, layout::Event, matrix::Matrix};
use keyboard_thing::{
    self as _,
    async_rw::UsbSerialWrapper,
    cps::{cps_task, Cps, SampleBuffer},
    forever, init_heap,
    layout::{Layout, COLS_PER_SIDE, ROWS},
    leds::{rainbow_single, Leds, TapWaves},
    lhs_display::{
        self, DisplayOverride, LHSDisplay, AVERAGE_KEYPRESSES, KEYPRESS_EVENT, TOTAL_KEYPRESSES,
    },
    messages::{DomToSub, Eventer, HostToKeyboard, KeyboardToHost, SubToDom},
    oled::{display_timeout_task, interacted, Oled},
    wrapping_id::WrappingID,
    DEBOUNCER_TICKS, POLL_PERIOD, UART_BAUD,
};
use num_enum::TryFromPrimitive;
use packed_struct::PackedStruct;
use usbd_human_interface_device::{device::keyboard::NKROBootKeyboardReport, page::Keyboard};

static TOTAL_LHS_KEYPRESSES: AtomicU32 = AtomicU32::new(0);

static LED_KEY_LISTEN_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();

/// Channels that receive each debounced key press
static KEY_EVENT_CHANS: &[&Channel<ThreadModeRawMutex, Event, 16>] = &[&LED_KEY_LISTEN_CHAN];
/// Key events that have been chorded or received from the other side
static PROCESSED_KEY_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();
/// Channel HID events are put on to be sent to the computer
static HID_CHAN: Channel<ThreadModeRawMutex, NKROBootKeyboardReport, 1> = Channel::new();
/// Channel commands are put on to be sent to the other side
static COMMAND_CHAN: Channel<ThreadModeRawMutex, (DomToSub, Duration), 4> = Channel::new();

trait StaticLen {
    const LEN: usize;
}

impl<T, const N: usize> StaticLen for [T; N] {
    const LEN: usize = N;
}

#[embassy::main]
async fn main(spawner: Spawner, p: Peripherals) {
    init_heap();

    let clock: pac::CLOCK = unsafe { core::mem::transmute(()) };
    let power: pac::POWER = unsafe { core::mem::transmute(()) };

    clock.tasks_hfclkstart.write(|w| unsafe { w.bits(1) });
    while clock.events_hfclkstarted.read().bits() != 1 {}

    while !power.usbregstatus.read().vbusdetect().is_vbus_present() {}

    let mut cortex_p = cortex_m::Peripherals::take().unwrap();
    cortex_p.SCB.enable_icache();

    let irq = interrupt::take!(USBD);
    let power_irq = interrupt::take!(POWER_CLOCK);
    let usb_driver = usb::Driver::new(p.USBD, irq, PowerUsb::new(power_irq));

    let mut config = embassy_usb::Config::new(0x6969, 0x0420);
    config
        .manufacturer
        .replace(core::option_env!("USB_MANUFACTURER").unwrap_or("Rust"));
    config
        .product
        .replace(core::option_env!("USB_PRODUCT").unwrap_or("Corne"));
    config
        .serial_number
        .replace(core::option_env!("USB_SERIAL").unwrap_or("1"));
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

    let res: &mut Resources = forever!(Resources {
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
        report_descriptor:
            usbd_human_interface_device::device::keyboard::NKRO_BOOT_KEYBOARD_REPORT_DESCRIPTOR,
        request_handler: None,
        poll_ms: 1,
        max_packet_size: 64,
    };
    let hid = HidWriter::<_, { <NKROBootKeyboardReport as PackedStruct>::ByteArray::LEN }>::new(
        &mut builder,
        &mut res.usb_state,
        hid_config,
    );

    let usb = builder.build();

    debug!("hello");

    let leds = Leds::new(p.PWM0, p.P0_06);

    let matrix = keyboard_thing::build_matrix!(p);
    let debouncer = Debouncer::new(
        [[false; COLS_PER_SIDE]; ROWS],
        [[false; COLS_PER_SIDE]; ROWS],
        DEBOUNCER_TICKS,
    );
    let chording = Chording::new(&keyboard_thing::layout::CHORDS);

    let layout = forever!(Mutex::new(Layout::new(&keyboard_thing::layout::LAYERS)));

    let mut uart_config = uarte::Config::default();
    uart_config.parity = uarte::Parity::EXCLUDED;
    uart_config.baudrate = UART_BAUD;

    let irq = interrupt::take!(UARTE0_UART0);
    let uart = uarte::Uarte::new(p.UARTE0, irq, p.P1_04, p.P0_08, uart_config);

    static SUB_TO_DOM_CHAN: Channel<ThreadModeRawMutex, SubToDom, 16> = Channel::new();
    // pain
    let eventer: &mut Eventer<
        '_,
        DomToSub,
        SubToDom,
        UarteTx<'static, UARTE0>,
        UarteRx<'static, UARTE0>,
    > = forever!(Eventer::<
        '_,
        DomToSub,
        SubToDom,
        UarteTx<'static, UARTE0>,
        UarteRx<'static, UARTE0>,
    >::new_uart(uart, SUB_TO_DOM_CHAN.sender()));
    let (e_a, e_b, e_c) = eventer.split_tasks(&COMMAND_CHAN);

    let irq = interrupt::take!(SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
    let mut config = twim::Config::default();
    config.frequency = unsafe { core::mem::transmute(159715200) };
    config.scl_high_drive = true;
    config.sda_high_drive = true;
    let twim = Twim::new(p.TWISPI0, irq, p.P0_17, p.P0_20, config);
    let oled = forever!(Mutex::new(Oled::new(twim)));

    let cps_samples = forever!(Mutex::new(SampleBuffer::default()));
    let cps = Cps::new(&TOTAL_KEYPRESSES, &AVERAGE_KEYPRESSES, cps_samples);

    spawner.spawn(cps_task(cps)).unwrap();
    spawner.spawn(usb_task(usb)).unwrap();
    spawner.spawn(usb_serial_task(serial_class)).unwrap();
    spawner.spawn(hid_task(hid)).unwrap();

    spawner.spawn(oled_task(oled)).unwrap();
    spawner.spawn(oled_timeout_task(oled)).unwrap();
    spawner.spawn(led_task(leds)).unwrap();
    spawner
        .spawn(keyboard_poll_task(matrix, debouncer, chording))
        .unwrap();
    spawner.spawn(keyboard_event_task(layout)).unwrap();
    spawner.spawn(layout_task(layout)).unwrap();
    spawner
        .spawn(read_events_task(SUB_TO_DOM_CHAN.receiver()))
        .unwrap();
    spawner.spawn(eventer_a(e_a)).unwrap();
    spawner.spawn(eventer_b(e_b)).unwrap();
    spawner.spawn(eventer_c(e_c)).unwrap();
    spawner.spawn(sync_kp_task()).unwrap();
}

#[embassy::task]
async fn oled_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    Timer::after(Duration::from_millis(100)).await;
    {
        let _ = oled.lock().await.init().await;
    }
    debug!("oled starting up");

    let mut display = LHSDisplay::new(oled);
    display.run().await;
}

#[embassy::task]
async fn oled_timeout_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    display_timeout_task(oled).await;
}

#[embassy::task]
async fn sync_kp_task() {
    Timer::after(Duration::from_millis(1000)).await;
    let mut ticker = Ticker::every(Duration::from_millis(100));
    let mut last = 0u32;

    loop {
        let current = TOTAL_LHS_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed);
        let diff = current - last;

        if diff != 0 {
            COMMAND_CHAN
                .send((
                    DomToSub::SyncKeypresses(diff as u16),
                    Duration::from_millis(5),
                ))
                .await;
        }

        last = current;

        ticker.next().await;
    }
}

type EventerA = impl Future + 'static;

#[embassy::task]
async fn eventer_a(f: EventerA) {
    f.await;
}

type EventerB = impl Future + 'static;

#[embassy::task]
async fn eventer_b(f: EventerB) {
    f.await;
}

type EventerC = impl Future + 'static;

#[embassy::task]
async fn eventer_c(f: EventerC) {
    f.await;
}

#[embassy::task]
async fn read_events_task(events_in: Receiver<'static, ThreadModeRawMutex, SubToDom, 16>) {
    loop {
        let event = events_in.recv().await;
        if let Some(event) = event.as_keyberon_event() {
            // events from the other side are already debounced and chord-resolved
            PROCESSED_KEY_CHAN.send(event).await;
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

            let collect = layout
                .keycodes()
                .filter_map(|k| Keyboard::try_from_primitive(k as u8).ok())
                .collect::<heapless::Vec<_, 24>>();

            if last_report.as_ref() != Some(&collect) {
                last_report = Some(collect.clone());
                HID_CHAN.send(NKROBootKeyboardReport::new(&collect)).await;
            }
        }

        Timer::after(Duration::from_millis(1)).await;
    }
}

#[embassy::task]
async fn keyboard_event_task(layout: &'static Mutex<ThreadModeRawMutex, Layout>) {
    loop {
        let event = PROCESSED_KEY_CHAN.recv().await;
        let mut count = if event.is_press() { 1 } else { 0 };
        if event.is_press() {
            KEYPRESS_EVENT.set();
        }
        interacted();
        {
            let mut layout = layout.lock().await;
            layout.event(event);
            debug!("evt: press: {} {:?}", event.is_press(), event.coord());
            while let Ok(event) = PROCESSED_KEY_CHAN.try_recv() {
                debug!("evt: press: {} {:?}", event.is_press(), event.coord());
                layout.event(event);
                count += if event.is_press() { 1 } else { 0 };
            }
        }
        TOTAL_KEYPRESSES.fetch_add(count, core::sync::atomic::Ordering::Relaxed);
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
            .collect::<heapless::Vec<_, 8>>();

        for event in &events {
            for chan in KEY_EVENT_CHANS {
                let _ = chan.try_send(*event);
            }
        }

        let events = chording.tick(events);

        let count = events.iter().filter(|e| e.is_press()).count() as u32;
        TOTAL_LHS_KEYPRESSES.fetch_add(count, core::sync::atomic::Ordering::Relaxed);

        for event in events {
            PROCESSED_KEY_CHAN.send(event).await;
        }

        Timer::after(POLL_PERIOD).await;
    }
}

#[embassy::task]
async fn led_task(mut leds: Leds) {
    let fps = 30;
    let mut tapwaves = TapWaves::new();
    let mut ticker = Ticker::every(Duration::from_millis(1000 / fps));
    let mut counter = WrappingID::<u16>::new(0);

    loop {
        while let Ok(event) = LED_KEY_LISTEN_CHAN.try_recv() {
            tapwaves.update(event);
        }

        tapwaves.tick();

        leds.send(tapwaves.render(|x, y| rainbow_single(x, y, counter.get() as u8)));

        counter.inc();

        if (counter.get() % 128) == 0 {
            let _ = COMMAND_CHAN.try_send((
                DomToSub::ResyncLeds(counter.get()),
                Duration::from_millis(5),
            ));
        }

        ticker.next().await;
    }
}

type UsbDriver = Driver<'static, peripherals::USBD, PowerUsb>;

#[embassy::task]
async fn hid_task(
    mut hid: HidWriter<
        'static,
        UsbDriver,
        { <NKROBootKeyboardReport as PackedStruct>::ByteArray::LEN },
    >,
) {
    loop {
        let report = HID_CHAN.recv().await;
        let _ = hid.write(&report.pack().unwrap()).await;
    }
}

#[embassy::task]
async fn usb_serial_task(mut class: CdcAcmClass<'static, UsbDriver>) {
    loop {
        let in_chan: &mut Channel<ThreadModeRawMutex, u8, 128> = forever!(Channel::new());
        let out_chan: &mut Channel<ThreadModeRawMutex, u8, 128> = forever!(Channel::new());
        let msg_out_chan: &mut Channel<ThreadModeRawMutex, HostToKeyboard, 16> =
            forever!(Channel::new());
        let msg_in_chan: &mut Channel<ThreadModeRawMutex, (KeyboardToHost, Duration), 16> =
            forever!(Channel::new());
        class.wait_connection().await;
        let mut wrapper = UsbSerialWrapper::new(&mut class, &*in_chan, &*out_chan);
        let mut eventer = Eventer::new(&*in_chan, &*out_chan, msg_out_chan.sender());

        let handle = async {
            loop {
                match msg_out_chan.recv().await {
                    HostToKeyboard::RequestStats => {
                        msg_in_chan
                            .send((
                                KeyboardToHost::Stats {
                                    keypresses: TOTAL_KEYPRESSES
                                        .load(core::sync::atomic::Ordering::Relaxed),
                                },
                                Duration::from_millis(5),
                            ))
                            .await;
                    }
                    HostToKeyboard::WritePixels {
                        side,
                        row,
                        data_0,
                        data_1,
                    } => match side {
                        keyboard_thing::messages::KeyboardSide::Left => {
                            lhs_display::OVERRIDE_CHAN
                                .send(DisplayOverride {
                                    row,
                                    data_0,
                                    data_1,
                                })
                                .await;
                            interacted();
                        }
                        keyboard_thing::messages::KeyboardSide::Right => {
                            COMMAND_CHAN
                                .send((
                                    DomToSub::WritePixels {
                                        row,
                                        data_0,
                                        data_1,
                                    },
                                    Duration::from_millis(1),
                                ))
                                .await
                        }
                    },
                }
            }
        };

        let (e_a, e_b, e_c) = eventer.split_tasks(msg_in_chan);

        select3(wrapper.run(), select3(e_a, e_b, e_c), handle).await;
    }
}

#[embassy::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver>) {
    device.run().await;
}
