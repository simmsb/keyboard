use alloc::sync::Arc;
use core::{
    hash::{Hash, Hasher},
    sync::atomic::AtomicU8,
};
use defmt::{debug, warn, Format};
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Sender},
    mutex::Mutex,
    time::{with_timeout, Duration},
    util::select3,
};
use embassy_nrf::uarte::{Instance, Uarte, UarteRx, UarteTx};
use postcard::{
    flavors::{Cobs, Slice},
    CobsAccumulator,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::event::Event;

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format, Hash, Clone)]
pub enum DomToSub {
    ResyncLeds(u16),
    Reset,
    SyncKeypresses(u16),
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, defmt::Format, Hash, Clone)]
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

#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub enum HostToKeyboard {
    RequestStats,
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub enum KeyboardToHost {
    Stats { keypresses: u32 },
    Log(heapless::Vec<u8, 60>),
}

#[derive(Serialize, Deserialize, defmt::Format)]
struct Command<T> {
    uuid: u8,
    csum: u8,
    cmd: T,
}

fn csum<T: Hash>(v: T) -> u8 {
    let mut hasher = fnv::FnvHasher::default();
    v.hash(&mut hasher);
    let checksum = hasher.finish();
    let checksum = (checksum >> 32) as u32 ^ checksum as u32;
    let checksum = (checksum >> 16) as u16 ^ checksum as u16;
    (checksum >> 8) as u8 ^ checksum as u8
}

impl<T: Hash> Command<T> {
    fn new(cmd: T) -> Self {
        static UUID_GEN: AtomicU8 = AtomicU8::new(0);
        let uuid = UUID_GEN.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        let csum = csum((&cmd, uuid));
        Self { uuid, csum, cmd }
    }

    /// validate the data of the command
    /// though the data will probably fail to deserialize if it has been corrupted, this just makes sure
    fn validate(self) -> Option<Self> {
        let csum = csum((&self.cmd, self.uuid));
        if csum == self.csum {
            Some(self)
        } else {
            None
        }
    }

    fn ack(&self) -> Ack {
        let csum = csum(self.uuid);
        Ack {
            uuid: self.uuid,
            csum,
        }
    }
}

#[derive(Serialize, Deserialize, defmt::Format)]
pub struct Ack {
    uuid: u8,
    csum: u8,
}

#[derive(Serialize, Deserialize, defmt::Format)]
enum CmdOrAck<T> {
    Cmd(Command<T>),
    Ack(Ack),
}

impl Ack {
    fn validate(self) -> Option<Self> {
        let csum = csum(self.uuid);
        if csum == self.csum {
            Some(self)
        } else {
            None
        }
    }
}

const BUF_SIZE: usize = 128;

pub struct Eventer<'a, T, U, UT: Instance> {
    tx: UarteTx<'a, UT>,
    rx: UarteRx<'a, UT>,
    mix_chan: Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    waiters: Mutex<ThreadModeRawMutex, heapless::FnvIndexMap<u8, Arc<Event>, 128>>,
}

struct EventSender<'e, T> {
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    waiters: &'e Mutex<ThreadModeRawMutex, heapless::FnvIndexMap<u8, Arc<Event>, 128>>,
}

struct EventOutProcessor<'a, 'e, T, UT: Instance> {
    tx: &'e mut UarteTx<'a, UT>,
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
}

struct EventInProcessor<'a, 'e, T, U, UT: Instance> {
    rx: &'e mut UarteRx<'a, UT>,
    out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    waiters: &'e Mutex<ThreadModeRawMutex, heapless::FnvIndexMap<u8, Arc<Event>, 128>>,
}

impl<'a, 'e, T, U, UT> EventInProcessor<'a, 'e, T, U, UT>
where
    U: DeserializeOwned + Hash + defmt::Format,
    UT: Instance,
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
                        warn!("Message decoder failed to deserialize a message");
                        buf
                    }
                    postcard::FeedResult::Success { data, remaining } => {
                        let data: CmdOrAck<U> = data;

                        match data {
                            CmdOrAck::Cmd(c) => {
                                if let Some(c) = c.validate() {
                                    debug!("Received command: {:?}", c);
                                    self.mix_chan.send(CmdOrAck::Ack(c.ack())).await;
                                    self.out_chan.send(c.cmd).await;
                                } else {
                                    warn!("Corrupted parsed command");
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

    async fn task(&mut self) {
        loop {
            let _ = self.recv_task_inner().await;
        }
    }
}

impl<'a, 'e, T, UT> EventOutProcessor<'a, 'e, T, UT>
where
    T: Serialize + defmt::Format,
    UT: Instance,
{
    async fn task(&mut self) {
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
    async fn send(&self, cmd: T) {
        loop {
            let cmd = Command::new(cmd.clone());
            let uuid = cmd.uuid;
            let waiter = self.register_waiter(uuid).await;
            self.mix_chan.send(CmdOrAck::Cmd(cmd)).await;

            match with_timeout(Duration::from_millis(4), waiter.wait()).await {
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

impl<'a, T, U, UT: Instance> Eventer<'a, T, U, UT> {
    pub fn new(uart: Uarte<'a, UT>, out_chan: Sender<'a, ThreadModeRawMutex, U, 16>) -> Self {
        let (tx, rx) = uart.split();
        Self {
            tx,
            rx,
            mix_chan: Channel::new(),
            out_chan,
            waiters: Mutex::new(heapless::FnvIndexMap::new()),
        }
    }

    pub async fn run<const N: usize>(
        &mut self,
        cmd_chan: &'static Channel<ThreadModeRawMutex, T, N>,
    ) where
        T: Hash + Clone + Serialize + Format,
        U: Hash + DeserializeOwned + Format,
    {
        let sender = EventSender {
            mix_chan: &self.mix_chan,
            waiters: &self.waiters,
        };

        let mut out_processor = EventOutProcessor {
            tx: &mut self.tx,
            mix_chan: &self.mix_chan,
        };

        let mut in_processor = EventInProcessor {
            rx: &mut self.rx,
            out_chan: self.out_chan.clone(),
            mix_chan: &self.mix_chan,
            waiters: &self.waiters,
        };

        let sender_proc = async move {
            loop {
                let evt = cmd_chan.recv().await;
                let _ = sender.send(evt).await;
            }
        };

        select3(sender_proc, out_processor.task(), in_processor.task()).await;
    }
}
