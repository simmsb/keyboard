use alloc::sync::Arc;
use core::hash::Hash;
use defmt::{debug, warn, Format};
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Sender},
    mutex::Mutex,
    time::{with_timeout, Duration},
};
use embassy_nrf::uarte::{Instance, Uarte, UarteRx, UarteTx};
use futures::Future;
use postcard::{
    flavors::{Cobs, Slice},
    CobsAccumulator,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub use keyboard_shared::*;

use crate::{
    async_rw::{AsyncRead, AsyncWrite},
    event::Event,
};

#[derive(Serialize, Deserialize, Eq, PartialEq, Format, Hash, Clone)]
pub enum DomToSub {
    ResyncLeds(u16),
    Reset,
    SyncKeypresses(u16),
    WritePixels {
        row: u8,
        data_0: [u8; 4],
        data_1: [u8; 4],
    },
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, Format, Hash, Clone)]
pub enum SubToDom {
    KeyPressed(u8),
    KeyReleased(u8),
}

impl SubToDom {
    pub fn as_keyberon_event(&self) -> Option<keyberon::layout::Event> {
        match self {
            SubToDom::KeyPressed(v) => {
                Some(keyberon::layout::Event::Press((v >> 4) & 0xf, v & 0xf))
            }
            SubToDom::KeyReleased(v) => {
                Some(keyberon::layout::Event::Release((v >> 4) & 0xf, v & 0xf))
            }
            _ => None,
        }
    }

    pub fn key_pressed(x: u8, y: u8) -> Self {
        Self::KeyPressed(((x & 0xf) << 4) | (y & 0xf))
    }

    pub fn key_released(x: u8, y: u8) -> Self {
        Self::KeyReleased(((x & 0xf) << 4) | (y & 0xf))
    }
}

const BUF_SIZE: usize = 128;

pub struct Eventer<'a, T, U, TX, RX> {
    tx: TX,
    rx: RX,
    mix_chan: Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    waiters: Mutex<ThreadModeRawMutex, heapless::FnvIndexMap<u8, Arc<Event>, 128>>,
}

struct EventSender<'e, T> {
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    waiters: &'e Mutex<ThreadModeRawMutex, heapless::FnvIndexMap<u8, Arc<Event>, 128>>,
}

struct EventOutProcessor<'e, T, TX> {
    tx: &'e mut TX,
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
}

struct EventInProcessor<'a, 'e, T, U, RX> {
    rx: &'e mut RX,
    out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    waiters: &'e Mutex<ThreadModeRawMutex, heapless::FnvIndexMap<u8, Arc<Event>, 128>>,
}

impl<'a, 'e, T, U, RX> EventInProcessor<'a, 'e, T, U, RX>
where
    U: DeserializeOwned + Hash + Format,
    RX: AsyncRead,
{
    async fn recv_task_inner(&mut self) -> Option<()> {
        let mut accumulator = CobsAccumulator::<BUF_SIZE>::new();

        loop {
            let mut buf = [0u8; 1];
            self.rx.read(&mut buf).await.ok()?;
            let mut window = &buf[..];

            'cobs: while !window.is_empty() {
                window = match accumulator.feed(window) {
                    postcard::FeedResult::Consumed => break 'cobs,
                    postcard::FeedResult::OverFull(buf) => buf,
                    postcard::FeedResult::DeserError(buf) => {
                        warn!(
                            "Message decoder failed to deserialize a message of type {}: {:?}",
                            core::any::type_name::<CmdOrAck<U>>(),
                            buf
                        );
                        buf
                    }
                    postcard::FeedResult::Success { data, remaining } => {
                        let data: CmdOrAck<U> = data;

                        match data {
                            CmdOrAck::Cmd(c) => {
                                if c.validate() {
                                    debug!("Received command: {:?}", c);
                                    self.mix_chan.send(CmdOrAck::Ack(c.ack())).await;
                                    self.out_chan.send(c.cmd).await;
                                } else {
                                    warn!("Corrupted parsed command: {:?}", c);
                                }
                            }
                            CmdOrAck::Ack(a) => {
                                if let Some(a) = a.validate() {
                                    debug!("Received ack: {:?}", a);
                                    let mut waiters = self.waiters.lock().await;
                                    if let Some(waker) = waiters.remove(&a.uuid) {
                                        waker.set();
                                    }
                                } else {
                                    warn!("Corrupted parsed ack");
                                }
                            }
                        }

                        remaining
                    }
                }
            }
        }
    }

    async fn task(mut self) {
        loop {
            let _ = self.recv_task_inner().await;
        }
    }
}

impl<'e, T, TX> EventOutProcessor<'e, T, TX>
where
    T: Serialize + Format,
    TX: AsyncWrite,
    <TX as AsyncWrite>::Error: Format,
{
    async fn task(self) {
        loop {
            let val = self.mix_chan.recv().await;

            let mut buf = [0u8; BUF_SIZE];
            if let Ok(buf) =
                postcard::serialize_with_flavor(&val, Cobs::try_new(Slice::new(&mut buf)).unwrap())
            {
                let r = self.tx.write(buf).await;
                debug!("Transmitted {:?}, r: {:?}", val, r);
            }
        }
    }
}

impl<'a, T: Hash + Clone> EventSender<'a, T> {
    async fn send(&self, cmd: T, timeout: Duration) {
        loop {
            let cmd = Command::new(cmd.clone());
            let uuid = cmd.uuid;
            let waiter = self.register_waiter(uuid).await;
            self.mix_chan.send(CmdOrAck::Cmd(cmd)).await;

            match with_timeout(timeout, waiter.wait()).await {
                Ok(_) => {
                    debug!("Waiter for uuid {} completed", uuid);
                    return;
                }
                Err(_) => {
                    warn!("Waiter for uuid{} timing out", uuid);
                    self.deregister_waiter(uuid).await;
                }
            }
        }
    }

    async fn register_waiter(&self, uuid: u8) -> Arc<Event> {
        let signal = Arc::new(Event::new());
        let mut waiters = self.waiters.lock().await;
        if waiters.insert(uuid, signal.clone()).is_ok() {
            signal
        } else {
            panic!("Duped waiter uuid")
        }
    }

    async fn deregister_waiter(&self, uuid: u8) {
        self.waiters.lock().await.remove(&uuid);
    }
}

impl<'a, T, U, TX, RX> Eventer<'a, T, U, TX, RX> {
    pub fn new(tx: TX, rx: RX, out_chan: Sender<'a, ThreadModeRawMutex, U, 16>) -> Self {
        Self {
            tx,
            rx,
            mix_chan: Channel::new(),
            out_chan,
            waiters: Mutex::new(heapless::FnvIndexMap::new()),
        }
    }

    pub fn new_uart<UT: Instance>(
        uart: Uarte<'static, UT>,
        out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    ) -> Eventer<'a, T, U, UarteTx<'static, UT>, UarteRx<'static, UT>> {
        let (tx, rx) = uart.split();

        Eventer::new(tx, rx, out_chan)
    }

    pub fn split_tasks<'s, const N: usize>(
        &'s mut self,
        cmd_chan: &'static Channel<ThreadModeRawMutex, (T, Duration), N>,
    ) -> (impl Future + 's, impl Future + 's, impl Future + 's)
    where
        T: Hash + Clone + Serialize + Format,
        U: Hash + DeserializeOwned + Format,
        TX: AsyncWrite,
        RX: AsyncRead,
        <TX as AsyncWrite>::Error: Format,
    {
        let sender = EventSender {
            mix_chan: &self.mix_chan,
            waiters: &self.waiters,
        };

        let out_processor = EventOutProcessor {
            tx: &mut self.tx,
            mix_chan: &self.mix_chan,
        };

        let in_processor = EventInProcessor {
            rx: &mut self.rx,
            out_chan: self.out_chan.clone(),
            mix_chan: &self.mix_chan,
            waiters: &self.waiters,
        };

        let sender_proc = async move {
            loop {
                let (evt, timeout) = cmd_chan.recv().await;
                let _ = sender.send(evt, timeout).await;
            }
        };

        (sender_proc, out_processor.task(), in_processor.task())
    }
}
