#![no_main]
#![no_std]

use keyboard_thing as _;

#[rtic::app(device = nrf52840_hal::pac, peripherals = true, dispatchers = [SWI0_EGU0, SWI1_EGU1])]
mod app {
    use embedded_hal::timer::CountDown;
    use fugit::ExtU32;
    use keyberon::debounce::Debouncer;
    use keyberon::hid::HidClass;
    use keyberon::keyboard::Keyboard;
    use keyberon::layout::Layout;
    use keyberon::matrix::Matrix;
    use keyboard_thing::leds::Leds;
    use keyboard_thing::messages::{DomToSub, EventReader, EventSender, SubToDom};
    use keyboard_thing::mono::MonoTimer;
    use nrf52840_hal::clocks::{ExternalOscillator, Internal, LfOscStopped};
    use nrf52840_hal::gpio::{Input, Output, Pin, PullUp, PushPull};
    use nrf52840_hal::pac::{TIMER0, TIMER1};
    use nrf52840_hal::timer::Periodic;
    use nrf52840_hal::usbd::{UsbPeripheral, Usbd};
    use nrf52840_hal::{uarte, Clocks, Timer, Uarte};
    use usb_device::class::UsbClass;
    use usb_device::prelude::*;

    #[monotonic(binds = TIMER0, default = true)]
    type Mono = MonoTimer<TIMER0>;

    #[shared]
    struct Shared {
        usb_hid_class: HidClass<'static, Usbd<UsbPeripheral<'static>>, Keyboard<()>>,
        #[lock_free]
        layout: Layout<12, 4, 1, keyboard_thing::layout::CustomEvent>,
    }

    #[local]
    struct Local {
        usb_dev: UsbDevice<'static, Usbd<UsbPeripheral<'static>>>,
        tick_timer: Timer<TIMER1, Periodic>,
        serial: usbd_serial::SerialPort<'static, Usbd<UsbPeripheral<'static>>>,
        matrix: Matrix<Pin<Input<PullUp>>, Pin<Output<PushPull>>, 6, 4>,
        debouncer: Debouncer<[[bool; 6]; 4]>,
        other_side_events: EventReader<SubToDom, nrf52840_hal::pac::UARTE0>,
        other_side_queue: heapless::spsc::Queue<SubToDom, 8>,
        event_sender: EventSender<DomToSub, nrf52840_hal::pac::UARTE0>,
        leds: Leds,
    }

    #[init]
    fn init(ctx: init::Context) -> (Shared, Local, init::Monotonics) {
        static mut CLOCKS: Option<Clocks<ExternalOscillator, Internal, LfOscStopped>> = None;
        static mut USB_BUS: Option<
            usb_device::class_prelude::UsbBusAllocator<Usbd<UsbPeripheral<'static>>>,
        > = None;

        while !ctx
            .device
            .POWER
            .usbregstatus
            .read()
            .vbusdetect()
            .is_vbus_present()
        {}

        while !ctx
            .device
            .POWER
            .events_usbpwrrdy
            .read()
            .events_usbpwrrdy()
            .bit_is_clear()
        {}

        let mono = MonoTimer::new(ctx.device.TIMER0);

        defmt::info!("Booting");

        let clocks = Clocks::new(ctx.device.CLOCK).enable_ext_hfosc();
        unsafe { CLOCKS.replace(clocks) };
        let usb_bus = Usbd::new(UsbPeripheral::new(ctx.device.USBD, unsafe {
            CLOCKS.as_ref().unwrap()
        }));
        unsafe { USB_BUS.replace(usb_bus) };
        let usb_bus = unsafe { USB_BUS.as_ref().unwrap() };

        let serial = usbd_serial::SerialPort::new(usb_bus);

        let usb_hid_class = keyberon::new_class(usb_bus, ());
        let usb_dev = UsbDeviceBuilder::new(usb_bus, UsbVidPid(0x6969, 0x0420))
            .manufacturer("Dick")
            .product("Sniffer")
            .serial_number("69420")
            .max_packet_size_0(64)
            .build();

        let gpios_p0 = nrf52840_hal::gpio::p0::Parts::new(ctx.device.P0);
        let gpios_p1 = nrf52840_hal::gpio::p1::Parts::new(ctx.device.P1);

        let matrix = keyboard_thing::build_matrix!(gpios_p0, gpios_p1);
        let debouncer = Debouncer::new([[false; 6]; 4], [[false; 6]; 4], 5);
        let layout = Layout::new(&keyboard_thing::layout::LAYERS);

        let uarte_pins = uarte::Pins {
            rxd: gpios_p0.p0_08.into_floating_input().degrade(),
            txd: gpios_p1
                .p1_04
                .into_push_pull_output(nrf52840_hal::gpio::Level::High)
                .degrade(),
            cts: None,
            rts: None,
        };
        let uarte = Uarte::new(
            ctx.device.UARTE0,
            uarte_pins,
            uarte::Parity::EXCLUDED,
            uarte::Baudrate::BAUD115200,
        );
        static mut UARTE_TX: [u8; 32] = [0; 32];
        static mut UARTE_RX: [u8; 1] = [0; 1];
        let (uarte_tx, uarte_rx) = unsafe { uarte.split(&mut UARTE_TX, &mut UARTE_RX).unwrap() };

        let event_sender = EventSender::<DomToSub, _>::new(uarte_tx);

        let other_side_queue = heapless::spsc::Queue::new();
        let other_side_events = EventReader::new(uarte_rx);

        let mut tick_timer = Timer::periodic(ctx.device.TIMER1);
        tick_timer.enable_interrupt();
        tick_timer.start(Timer::<TIMER1, Periodic>::TICKS_PER_SECOND / 1000);

        let leds = Leds::new(ctx.device.PWM0, gpios_p0.p0_06.degrade());

        let shared = Shared {
            layout,
            usb_hid_class,
        };

        let local = Local {
            leds,
            usb_dev,
            tick_timer,
            serial,
            matrix,
            debouncer,
            other_side_queue,
            other_side_events,
            event_sender,
        };

        (shared, local, init::Monotonics(mono))
    }

    #[task(binds = UARTE0_UART0, priority = 4, local = [other_side_queue, other_side_events])]
    fn rx_other_side(ctx: rx_other_side::Context) {
        let _ = ctx.local.other_side_events.read(ctx.local.other_side_queue);
        while let Some(evt) = ctx.local.other_side_queue.dequeue() {
            if let Some(evt) = evt.as_keyberon_event() {
                handle_keyberon_event::spawn(evt).ok().unwrap();
            }
        }
    }

    #[task(priority = 2, capacity = 8, shared = [layout])]
    fn handle_keyberon_event(ctx: handle_keyberon_event::Context, event: keyberon::layout::Event) {
        ctx.shared.layout.event(event);
    }

    #[task(priority = 2, shared = [layout, usb_hid_class])]
    fn tick_keyberon(mut ctx: tick_keyberon::Context) {
        let tick = ctx.shared.layout.tick();
        match tick {
            keyberon::layout::CustomEvent::NoEvent => {}
            keyberon::layout::CustomEvent::Press(_) => {}
            keyberon::layout::CustomEvent::Release(_) => {}
        }
        let report: keyberon::key_code::KbHidReport = ctx.shared.layout.keycodes().collect();
        if !ctx
            .shared
            .usb_hid_class
            .lock(|k| k.device_mut().set_keyboard_report(report.clone()))
        {
            return;
        }
        while let Ok(0) = ctx
            .shared
            .usb_hid_class
            .lock(|k| k.write(report.as_bytes()))
        {}
    }

    #[task(binds = TIMER1, priority = 2, local = [tick_timer, matrix, debouncer])]
    fn tick(ctx: tick::Context) {
        let _ = ctx.local.tick_timer.wait();

        for event in ctx.local.debouncer.events(ctx.local.matrix.get().unwrap()) {
            handle_keyberon_event::spawn(event).unwrap();
        }

        tick_keyberon::spawn().unwrap();
    }

    #[task(local = [cnt: u8 = 0, leds])]
    fn led_tick(ctx: led_tick::Context, instant: keyboard_thing::mono::Instant) {
        *ctx.local.cnt += 1;

        ctx.local
            .leds
            .send(keyboard_thing::leds::rainbow(*ctx.local.cnt));

        let next_instant = instant + 16.millis();
        led_tick::spawn_at(next_instant, next_instant).unwrap();
    }

    #[idle(local = [usb_dev, serial], shared = [usb_hid_class])]
    // #[idle(local = [usb_hid_dev], shared = [usb_hid_class])]
    fn idle(mut ctx: idle::Context) -> ! {
        loop {
            ctx.shared.usb_hid_class.lock(|usb_class| {
                if ctx.local.usb_dev.poll(&mut [ctx.local.serial, usb_class]) {
                    usb_class.poll();

                    let mut buf = [0u8; 64];

                    match ctx.local.serial.read(&mut buf[..]) {
                        Ok(c) => {
                            let _ = ctx.local.serial.write(b"cock: ");
                            let _ = ctx.local.serial.write(&buf[..c]);
                            let _ = ctx.local.serial.write(b"\r\n");
                        }
                        Err(UsbError::WouldBlock) => {}
                        Err(e) => {
                            defmt::error!("Oopsie usb {:?}", e as u8);
                        }
                    }
                }
            });
        }
    }
}
