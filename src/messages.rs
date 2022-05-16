use core::marker::PhantomData;
use embassy_nrf::uarte::{Instance, UarteRx, UarteTx};
use postcard::flavors::{Cobs, Slice};
use postcard::CobsAccumulator;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format)]
pub enum DomToSub {
    ResyncLeds,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, defmt::Format)]
pub enum SubToDom {
    KeyPressed(u8, u8),
    KeyReleased(u8, u8),
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub enum HostToKeyboard {
    RequestStats,
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub enum KeyboardToHost {
    Stats {
        keypresses: u32,
    },
    Log(heapless::Vec<u8, 60>)
}

const BUF_SIZE: usize = 128;

impl SubToDom {
    pub fn as_keyberon_event(&self) -> Option<keyberon::layout::Event> {
        match self {
            SubToDom::KeyPressed(x, y) => Some(keyberon::layout::Event::Press(*x, *y)),
            SubToDom::KeyReleased(x, y) => Some(keyberon::layout::Event::Release(*x, *y)),
        }
    }
}

pub struct EventSender<'a, T: Serialize, UT: Instance> {
    tx: UarteTx<'a, UT>,
    _phantom: PhantomData<fn(T)>,
}

impl<'a, T: Serialize, UT: Instance> EventSender<'a, T, UT> {
    pub fn new(tx: UarteTx<'a, UT>) -> Self {
        Self {
            tx,
            _phantom: PhantomData,
        }
    }

    pub async fn send(&mut self, val: &T) -> Option<()> {
        let mut buf = [0u8; BUF_SIZE];
        let buf =
            postcard::serialize_with_flavor(val, Cobs::try_new(Slice::new(&mut buf)).unwrap())
                .ok()?;

        self.tx.write(buf).await.ok()?;

        Some(())
    }
}

pub struct EventReader<'a, T: DeserializeOwned, UT: Instance> {
    rx: UarteRx<'a, UT>,
    accumulator: CobsAccumulator<BUF_SIZE>,
    _phantom: PhantomData<fn() -> T>,
}

impl<'a, T: DeserializeOwned, UT: Instance> EventReader<'a, T, UT> {
    pub fn new(rx: UarteRx<'a, UT>) -> Self {
        Self {
            rx,
            accumulator: CobsAccumulator::new(),
            _phantom: PhantomData,
        }
    }

    pub async fn read<const QUEUE_LEN: usize>(
        &mut self,
        dest: &mut heapless::spsc::Queue<T, QUEUE_LEN>,
    ) -> Option<()> {
        let mut buf = [0u8; 1];
        self.rx.read(&mut buf).await.ok()?;
        let mut window = &buf[..];

        'cobs: while !window.is_empty() {
            window = match self.accumulator.feed(window) {
                postcard::FeedResult::Consumed => break 'cobs,
                postcard::FeedResult::OverFull(buf) => buf,
                postcard::FeedResult::DeserError(buf) => buf,
                postcard::FeedResult::Success { data, remaining } => {
                    dest.enqueue(data).ok()?;

                    remaining
                }
            }
        }

        Some(())
    }
}
