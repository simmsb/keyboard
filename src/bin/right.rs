#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::AtomicU8;

use defmt::debug;
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Receiver},
    executor::Spawner,
    mutex::Mutex,
    time::{Duration, Ticker, Timer},
    util::Forever,
};
use embassy_nrf::{
    gpio::{AnyPin, Input, Output},
    interrupt,
    peripherals::{TWISPI0, UARTE0},
    twim::{self, Twim},
    uarte, Peripherals,
};
use futures::StreamExt;
use keyberon::{chording::Chording, debounce::Debouncer, layout::Event, matrix::Matrix};
use keyboard_thing::{
    self as _, init_heap,
    layout::{COLS_PER_SIDE, ROWS},
    leds::{rainbow_single, Leds, TapWaves},
    messages::{DomToSub, EventInProcessor, EventOutProcessor, EventSender, Eventer, SubToDom},
    oled::{display_timeout_task, interacted, Oled},
    rhs_display::{RHSDisplay, KEYPRESS_SIGNAL, TOTAL_KEYPRESSES, AVERAGE_KEYPRESSES},
    DEBOUNCER_TICKS, POLL_PERIOD, UART_BAUD, cpm::{Cpm, cpm_task},
};

static LED_KEY_LISTEN_CHAN: Channel<ThreadModeRawMutex, Event, 16> = Channel::new();
/// Channels that receive each debounced key press
static KEY_EVENT_CHANS: &[&Channel<ThreadModeRawMutex, Event, 16>] = &[&LED_KEY_LISTEN_CHAN];
/// Channel commands are put on to be sent to the other side
static COMMAND_CHAN: Channel<ThreadModeRawMutex, SubToDom, 4> = Channel::new();

static LED_COUNTER: AtomicU8 = AtomicU8::new(0);

#[embassy::main]
async fn main(spawner: Spawner, p: Peripherals) {
    init_heap();

    let mut cortex_p = cortex_m::Peripherals::take().unwrap();
    cortex_p.SCB.enable_icache();

    let leds = Leds::new(p.PWM0, p.P0_06);

    let matrix = keyboard_thing::build_matrix!(p);
    let debouncer = Debouncer::new(
        [[false; COLS_PER_SIDE]; ROWS],
        [[false; COLS_PER_SIDE]; ROWS],
        DEBOUNCER_TICKS,
    );
    let chording = Chording::new(&keyboard_thing::layout::CHORDS);

    let mut uart_config = uarte::Config::default();
    uart_config.parity = uarte::Parity::EXCLUDED;
    uart_config.baudrate = UART_BAUD;

    let irq = interrupt::take!(UARTE0_UART0);
    let uart = uarte::Uarte::new(p.UARTE0, irq, p.P0_08, p.P1_04, uart_config);
    static EVENTER: Forever<Eventer<SubToDom, DomToSub, UARTE0>> = Forever::new();
    static DOM_TO_SUB_CHAN: Channel<ThreadModeRawMutex, DomToSub, 16> = Channel::new();
    let eventer = EVENTER.put(Eventer::new(uart, DOM_TO_SUB_CHAN.sender()));
    let (event_sender, event_out_proc, event_in_proc) = eventer.split();

    let irq = interrupt::take!(SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
    let twim = Twim::new(p.TWISPI0, irq, p.P0_17, p.P0_20, twim::Config::default());
    static OLED: Forever<Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>> = Forever::new();
    let oled = OLED.put(Mutex::new(Oled::new(twim)));
    let cpm = Cpm::new(&TOTAL_KEYPRESSES, &AVERAGE_KEYPRESSES);

    spawner.spawn(cpm_task(cpm)).unwrap();
    spawner.spawn(oled_task(oled)).unwrap();
    spawner.spawn(oled_timeout_task(oled)).unwrap();
    spawner.spawn(led_task(leds)).unwrap();
    spawner
        .spawn(keyboard_poll_task(matrix, debouncer, chording))
        .unwrap();
    spawner
        .spawn(read_events_task(DOM_TO_SUB_CHAN.receiver()))
        .unwrap();
    spawner.spawn(send_events_task(event_sender)).unwrap();
    spawner
        .spawn(process_events_in_task(event_in_proc))
        .unwrap();
    spawner
        .spawn(process_events_out_task(event_out_proc))
        .unwrap();
}

#[embassy::task]
async fn oled_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    Timer::after(Duration::from_millis(100)).await;
    let _ = oled.lock().await.init().await;
    debug!("oled starting up");

    let mut display = RHSDisplay::new(oled);
    display.run().await;
}

#[embassy::task]
async fn oled_timeout_task(oled: &'static Mutex<ThreadModeRawMutex, Oled<'static, TWISPI0>>) {
    display_timeout_task(oled).await;
}

#[embassy::task]
async fn send_events_task(events_out: EventSender<'static, SubToDom>) {
    loop {
        let evt = COMMAND_CHAN.recv().await;
        let _ = events_out.send(evt).await;
    }
}

#[embassy::task]
async fn process_events_in_task(
    mut proc: EventInProcessor<'static, 'static, SubToDom, DomToSub, UARTE0>,
) {
    proc.task().await;
}

#[embassy::task]
async fn process_events_out_task(mut proc: EventOutProcessor<'static, 'static, SubToDom, UARTE0>) {
    proc.task().await;
}

#[embassy::task]
async fn read_events_task(events_in: Receiver<'static, ThreadModeRawMutex, DomToSub, 16>) {
    loop {
        let event = events_in.recv().await;
        match event {
            DomToSub::ResyncLeds => {
                LED_COUNTER.store(0, core::sync::atomic::Ordering::SeqCst);
            }
            DomToSub::Reset => {
                cortex_m::peripheral::SCB::sys_reset();
            }
            DomToSub::SyncKeypresses(kp) => {
                if kp != 0 {
                    TOTAL_KEYPRESSES.fetch_add(kp as u32, core::sync::atomic::Ordering::Relaxed);
                    KEYPRESS_SIGNAL.signal(());
                    interacted();
                }
            },
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
                keyberon::layout::Event::Press(x, y) => SubToDom::key_pressed(x, y),
                keyberon::layout::Event::Release(x, y) => SubToDom::key_released(x, y),
            };
            COMMAND_CHAN.send(msg).await;
            if event.is_press() {
                TOTAL_KEYPRESSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                KEYPRESS_SIGNAL.signal(());
            }
        }

        Timer::after(POLL_PERIOD).await;
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
