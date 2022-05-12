use core::marker::PhantomData;
use cortex_m::prelude::*;
use nrf52840_hal::uarte::{Instance, UarteRx, UarteTx};
use postcard::flavors::{Cobs, Slice};
use postcard::CobsAccumulator;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format)]
pub enum DomToSub {
    ResyncLeds,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format, Debug)]
pub enum SubToDom {
    KeyPressed(u8, u8),
    KeyReleased(u8, u8),
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

pub struct EventSender<T: Serialize, UT: Instance> {
    tx: UarteTx<UT>,
    _phantom: PhantomData<fn(T)>,
}

impl<T: Serialize, UT: Instance> EventSender<T, UT> {
    pub fn new(tx: UarteTx<UT>) -> Self {
        Self {
            tx,
            _phantom: PhantomData,
        }
    }

    pub fn send(&mut self, val: &T) -> Option<()> {
        let mut buf = [0u8; BUF_SIZE];
        let buf =
            postcard::serialize_with_flavor(val, Cobs::try_new(Slice::new(&mut buf)).unwrap())
                .ok()?;

        self.tx.bwrite_all(buf).ok()?;
        self.tx.bflush().ok()?;

        Some(())
    }
}

pub struct EventReader<T: DeserializeOwned, UT: Instance> {
    rx: UarteRx<UT>,
    accumulator: CobsAccumulator<BUF_SIZE>,
    _phantom: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned, UT: Instance> EventReader<T, UT> {
    pub fn new(rx: UarteRx<UT>) -> Self {
        Self {
            rx,
            accumulator: CobsAccumulator::new(),
            _phantom: PhantomData,
        }
    }

    pub fn read<const QUEUE_LEN: usize>(
        &mut self,
        dest: &mut heapless::spsc::Queue<T, QUEUE_LEN>,
    ) -> Option<()> {
        let byte = self.rx.read().ok()?;

        let mut window = &[byte][..];

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
