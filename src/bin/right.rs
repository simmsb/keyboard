#![no_main]
#![no_std]

use keyboard_thing as _;

#[rtic::app(device = nrf52840_hal::pac, peripherals = true, dispatchers = [SWI0_EGU0, SWI1_EGU1])]
mod app {
    use core::sync::atomic::AtomicU16;

    use embedded_hal::timer::CountDown;
    use fugit::ExtU32;
    use keyberon::debounce::Debouncer;
    use keyberon::matrix::Matrix;
    use keyboard_thing::leds::Leds;
    use keyboard_thing::messages::{DomToSub, EventReader, EventSender, SubToDom};
    use keyboard_thing::mono::MonoTimer;
    use nrf52840_hal::gpio::{Input, Output, Pin, PullUp, PushPull};
    use nrf52840_hal::pac::{TIMER0, TIMER1};
    use nrf52840_hal::timer::Periodic;
    use nrf52840_hal::{uarte, Timer, Uarte};

    #[monotonic(binds = TIMER0, default = true)]
    type Mono = MonoTimer<TIMER0>;

    static LED_CNT: AtomicU16 = AtomicU16::new(0);

    #[shared]
    struct Shared {}

    #[local]
    struct Local {
        tick_timer: Timer<TIMER1, Periodic>,
        matrix: Matrix<Pin<Input<PullUp>>, Pin<Output<PushPull>>, 6, 4>,
        debouncer: Debouncer<[[bool; 6]; 4]>,
        other_side_events: EventReader<DomToSub, nrf52840_hal::pac::UARTE0>,
        other_side_queue: heapless::spsc::Queue<DomToSub, 8>,
        event_sender: EventSender<SubToDom, nrf52840_hal::pac::UARTE0>,
        leds: Leds,
    }

    #[init]
    fn init(ctx: init::Context) -> (Shared, Local, init::Monotonics) {
        let mono = MonoTimer::new(ctx.device.TIMER0);

        defmt::info!("Booting");

        let gpios_p0 = nrf52840_hal::gpio::p0::Parts::new(ctx.device.P0);
        let gpios_p1 = nrf52840_hal::gpio::p1::Parts::new(ctx.device.P1);

        let matrix = keyboard_thing::build_matrix!(gpios_p0, gpios_p1);
        let debouncer = Debouncer::new([[false; 6]; 4], [[false; 6]; 4], 5);

        // TODO: not sure if we need to flip these, check schematic
        let uarte_pins = uarte::Pins {
            rxd: gpios_p0.p0_08.into_floating_input().degrade(),
            txd: gpios_p1
                .p1_04
                .into_push_pull_output(nrf52840_hal::gpio::Level::High)
                .degrade(),
            cts: None,
            rts: None,
        };

        // TODO: do we need this
        ctx.device
            .UARTE0
            .intenset
            .modify(|_, w| w.endrx().set_bit());

        let uarte = Uarte::new(
            ctx.device.UARTE0,
            uarte_pins,
            uarte::Parity::EXCLUDED,
            uarte::Baudrate::BAUD1M,
        );
        static mut UARTE_TX: [u8; 1] = [0; 1];
        static mut UARTE_RX: [u8; 1] = [0; 1];
        let (uarte_tx, uarte_rx) = unsafe { uarte.split(&mut UARTE_TX, &mut UARTE_RX).unwrap() };

        let event_sender = EventSender::<SubToDom, _>::new(uarte_tx);

        let other_side_queue = heapless::spsc::Queue::new();
        let other_side_events = EventReader::new(uarte_rx);

        let mut tick_timer = Timer::periodic(ctx.device.TIMER1);
        tick_timer.enable_interrupt();
        tick_timer.start(Timer::<TIMER1, Periodic>::TICKS_PER_SECOND / 1000);

        let leds = Leds::new(ctx.device.PWM0, gpios_p0.p0_06.degrade());
        let _ = led_tick::spawn_after(100.millis());

        rtic::pend(nrf52840_hal::pac::Interrupt::UARTE0_UART0);

        let shared = Shared {};

        let local = Local {
            leds,
            tick_timer,
            matrix,
            debouncer,
            other_side_queue,
            other_side_events,
            event_sender,
        };

        (shared, local, init::Monotonics(mono))
    }

    // #[task(binds = UARTE0_UART0, priority = 4, local = [other_side_queue, other_side_events])]
    // fn rx_other_side(ctx: rx_other_side::Context) {
    // }

    #[task(capacity = 8)]
    fn handle_event(_ctx: handle_event::Context, event: DomToSub) {
        match event {
            DomToSub::ResyncLeds => LED_CNT.store(0, core::sync::atomic::Ordering::SeqCst),
        }
    }

    #[task(binds = TIMER1, priority = 2, local = [tick_timer, matrix, debouncer, event_sender])]
    fn tick(ctx: tick::Context) {
        let _ = ctx.local.tick_timer.wait();

        for event in ctx.local.debouncer.events(ctx.local.matrix.get().unwrap()) {
            let msg = match event {
                keyberon::layout::Event::Press(x, y) => SubToDom::KeyPressed(x, 11 - y),
                keyberon::layout::Event::Release(x, y) => SubToDom::KeyReleased(x, 11 - y),
            };

            ctx.local.event_sender.send(&msg);
        }
    }

    #[task(priority = 1, local = [leds])]
    fn led_tick(ctx: led_tick::Context) {
        let led_cnt = LED_CNT.fetch_add(1, core::sync::atomic::Ordering::SeqCst);

        ctx.local
            .leds
            .send(keyboard_thing::leds::rainbow(led_cnt as u8));

        let fps = 30;
        let fps_interval = 1u32.secs() / fps;
        let _ = led_tick::spawn_after(fps_interval);
    }

    #[idle(local = [other_side_queue, other_side_events])]
    fn idle(ctx: idle::Context) -> ! {
        loop {
            let _ = ctx.local.other_side_events.read(ctx.local.other_side_queue);
            while let Some(evt) = ctx.local.other_side_queue.dequeue() {
                let _ = handle_event::spawn(evt);
            }
        }
    }
}
