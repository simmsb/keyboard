#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]

use core::sync::atomic::AtomicU8;

use embassy::blocking_mutex::raw::ThreadModeRawMutex;
use embassy::channel::Channel;
use embassy::executor::Spawner;
use embassy::time::{Duration, Timer};
use embassy_nrf::gpio::{AnyPin, Input, Output};
use embassy_nrf::peripherals::UARTE0;
use embassy_nrf::{interrupt, uarte, Peripherals};
use keyberon::chording::Chording;
use keyberon::debounce::Debouncer;
use keyberon::matrix::Matrix;
use keyboard_thing as _;
use keyboard_thing::leds::{rainbow, Leds};
use keyboard_thing::messages::{DomToSub, EventReader, EventSender, SubToDom};

static EVENT_CHAN: Channel<ThreadModeRawMutex, SubToDom, 4> = Channel::new();
static LED_COUNTER: AtomicU8 = AtomicU8::new(0);

#[embassy::main]
async fn main(spawner: Spawner, p: Peripherals) {
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

    spawner.spawn(led_task(leds)).unwrap();
    spawner
        .spawn(keyboard_poll_task(matrix, debouncer, chording))
        .unwrap();
    spawner.spawn(read_events_task(events_in)).unwrap();
    spawner.spawn(send_events_task(events_out)).unwrap();
}

#[embassy::task]
async fn send_events_task(mut events_out: EventSender<'static, SubToDom, UARTE0>) {
    loop {
        let evt = EVENT_CHAN.recv().await;
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
            .collect::<heapless::Vec<_, 16>>();

        let events = chording.tick(events);

        for event in events {
            let msg = match event {
                keyberon::layout::Event::Press(x, y) => SubToDom::KeyPressed(x, 11 - y),
                keyberon::layout::Event::Release(x, y) => SubToDom::KeyReleased(x, 11 - y),
            };
            EVENT_CHAN.send(msg).await;
        }

        Timer::after(Duration::from_millis(20)).await;
    }
}

#[embassy::task]
async fn led_task(mut leds: Leds) {
    let fps = 30;
    loop {
        let i = LED_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Release);
        Timer::after(Duration::from_millis(1000 / fps)).await;
        leds.send(rainbow(i));
    }
}
