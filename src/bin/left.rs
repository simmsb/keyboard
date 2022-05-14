#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]

use embassy::blocking_mutex::raw::ThreadModeRawMutex;
use embassy::channel::Channel;
use embassy::executor::Spawner;
use embassy::time::{Duration, Timer};
use embassy::util::Forever;
use embassy_nrf::gpio::{AnyPin, Input, Output};
use embassy_nrf::peripherals::UARTE0;
use embassy_nrf::usb::Driver;
use embassy_nrf::{interrupt, pac, peripherals, uarte, usb, Peripherals};
use embassy_usb::UsbDevice;
use embassy_usb_hid::HidWriter;
use embassy_usb_serial::CdcAcmClass;
use keyberon::chording::Chording;
use keyberon::debounce::Debouncer;
use keyberon::key_code::KbHidReport;
use keyberon::layout::{Event, Layout};
use keyberon::matrix::Matrix;
use keyboard_thing as _;
use keyboard_thing::leds::{rainbow, Leds};
use keyboard_thing::messages::{DomToSub, EventReader, EventSender, SubToDom};
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

static LOG_CHAN: Channel<ThreadModeRawMutex, &'static str, 16> = Channel::new();
static KEY_EVENT_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();
static HID_CHAN: Channel<ThreadModeRawMutex, KbHidReport, 1> = Channel::new();
static EVENT_CHAN: Channel<ThreadModeRawMutex, DomToSub, 4> = Channel::new();

#[embassy::main]
async fn main(spawner: Spawner, p: Peripherals) {
    let clock: pac::CLOCK = unsafe { core::mem::transmute(()) };
    let power: pac::POWER = unsafe { core::mem::transmute(()) };

    clock.tasks_hfclkstart.write(|w| unsafe { w.bits(1) });
    while clock.events_hfclkstarted.read().bits() != 1 {}

    while !power.usbregstatus.read().vbusdetect().is_vbus_present() {}

    let irq = interrupt::take!(USBD);
    let usb_driver = usb::Driver::new(p.USBD, irq);

    let mut config = embassy_usb::Config::new(0x6969, 0x0420);
    config.manufacturer.replace("Dick");
    config.product.replace("Sniffer");
    config.serial_number.replace("69420");
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
        poll_ms: 20,
        max_packet_size: 64,
    };
    let hid = HidWriter::<_, 8>::new(&mut builder, &mut res.usb_state, hid_config);

    let usb = builder.build();

    let leds = Leds::new(p.PWM0, p.P0_06);

    let matrix = keyboard_thing::build_matrix!(p);
    let debouncer = Debouncer::new([[false; 6]; 4], [[false; 6]; 4], 30);
    let chording = Chording::new(&keyboard_thing::layout::CHORDS);
    let layout = Layout::new(&keyboard_thing::layout::LAYERS);

    let mut uart_config = uarte::Config::default();
    uart_config.parity = uarte::Parity::EXCLUDED;
    uart_config.baudrate = uarte::Baudrate::BAUD1M;

    let irq = interrupt::take!(UARTE0_UART0);
    let uart = uarte::Uarte::new(p.UARTE0, irq, p.P1_04, p.P0_08, uart_config);
    let (uart_out, uart_in) = uart.split();
    let events_out = EventSender::new(uart_out);
    let events_in = EventReader::new(uart_in);

    spawner.spawn(usb_task(usb)).unwrap();
    spawner.spawn(log_task(serial_class)).unwrap();
    spawner.spawn(hid_task(hid)).unwrap();
    spawner.spawn(led_task(leds)).unwrap();
    spawner
        .spawn(keyboard_poll_task(matrix, debouncer, chording))
        .unwrap();
    spawner.spawn(keyboard_event_task(layout)).unwrap();
    spawner.spawn(read_events_task(events_in)).unwrap();
    spawner.spawn(send_events_task(events_out)).unwrap();
}

#[embassy::task]
async fn send_events_task(mut events_out: EventSender<'static, DomToSub, UARTE0>) {
    loop {
        let evt = EVENT_CHAN.recv().await;
        let _ = events_out.send(&evt).await;
    }
}

#[embassy::task]
async fn read_events_task(mut events_in: EventReader<'static, SubToDom, UARTE0>) {
    let mut queue: heapless::spsc::Queue<SubToDom, 8> = heapless::spsc::Queue::new();

    loop {
        let _ = events_in.read(&mut queue).await;
        while let Some(event) = queue.dequeue() {
            if let Some(event) = event.as_keyberon_event() {
                // events from the other side are already debounced and chord-resolved
                KEY_EVENT_CHAN.send(event).await;
            }
        }
    }
}

#[embassy::task]
async fn keyboard_event_task(mut layout: Layout<12, 5, 3, keyboard_thing::layout::CustomEvent>) {
    loop {
        let event = KEY_EVENT_CHAN.recv().await;
        layout.event(event);
        while let Ok(event) = KEY_EVENT_CHAN.try_recv() {
            layout.event(event);
        }
        layout.tick();
        HID_CHAN.send(layout.keycodes().collect()).await;
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
            .collect::<heapless::Vec<_, 16>>();

        let events = chording.tick(events);

        for event in events {
            KEY_EVENT_CHAN.send(event).await;
        }

        Timer::after(Duration::from_millis(20)).await;
    }
}

#[embassy::task]
async fn led_task(mut leds: Leds) {
    let fps = 30;
    for i in (0..255u8).cycle() {
        Timer::after(Duration::from_millis(1000 / fps)).await;
        leds.send(rainbow(i));
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

async fn log_inner(class: &mut CdcAcmClass<'static, UsbDriver>) -> Option<()> {
    loop {
        let msg = LOG_CHAN.recv().await;
        for chunk in msg.as_bytes().chunks(64) {
            class.write_packet(chunk).await.ok()?;
        }
    }
}

#[embassy::task]
async fn log_task(mut class: CdcAcmClass<'static, UsbDriver>) {
    loop {
        class.wait_connection().await;
        let _ = log_inner(&mut class).await;
    }
}

#[embassy::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDriver>) {
    device.run().await;
}
