use alloc::sync::Arc;
use core::hash::{Hash, Hasher};
use core::sync::atomic::AtomicU16;
use embassy::blocking_mutex::raw::ThreadModeRawMutex;
use embassy::channel::{Channel, Sender, Signal};
use embassy::mutex::Mutex;
use embassy::time::{with_timeout, Duration};
use embassy_nrf::uarte::{Instance, Uarte, UarteRx, UarteTx};
use postcard::flavors::{Cobs, Slice};
use postcard::CobsAccumulator;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format, Hash, Clone)]
pub enum DomToSub {
    ResyncLeds,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Debug, defmt::Format, Hash, Clone)]
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
    Stats { keypresses: u32 },
    Log(heapless::Vec<u8, 60>),
}

#[derive(Serialize, Deserialize)]
struct Command<T> {
    uuid: u16,
    csum: u16,
    cmd: T,
}

fn csum<T: Hash>(v: T) -> u16 {
    let mut hasher = fnv::FnvHasher::default();
    v.hash(&mut hasher);
    let checksum = hasher.finish();
    let checksum = (checksum >> 32) as u32 ^ checksum as u32;
    let checksum = (checksum >> 16) as u16 ^ checksum as u16;
    checksum
}

impl<T: Hash> Command<T> {
    fn new(cmd: T) -> Self {
        static UUID_GEN: AtomicU16 = AtomicU16::new(0);
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

#[derive(Serialize, Deserialize)]
pub struct Ack {
    uuid: u16,
    csum: u16,
}

#[derive(Serialize, Deserialize)]
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

impl SubToDom {
    pub fn as_keyberon_event(&self) -> Option<keyberon::layout::Event> {
        match self {
            SubToDom::KeyPressed(x, y) => Some(keyberon::layout::Event::Press(*x, *y)),
            SubToDom::KeyReleased(x, y) => Some(keyberon::layout::Event::Release(*x, *y)),
        }
    }
}

pub struct Eventer<'a, T, U, UT: Instance> {
    tx: UarteTx<'a, UT>,
    rx: UarteRx<'a, UT>,
    mix_chan: Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    waiters: Mutex<
        ThreadModeRawMutex,
        heapless::FnvIndexMap<u16, Arc<embassy::channel::Signal<()>>, 128>,
    >,
}

pub struct EventSender<'e, T> {
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    waiters: &'e Mutex<
        ThreadModeRawMutex,
        heapless::FnvIndexMap<u16, Arc<embassy::channel::Signal<()>>, 128>,
    >,
}

pub struct EventOutProcessor<'a, 'e, T, UT: Instance> {
    tx: &'e mut UarteTx<'a, UT>,
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
}

pub struct EventInProcessor<'a, 'e, T, U, UT: Instance> {
    rx: &'e mut UarteRx<'a, UT>,
    out_chan: Sender<'a, ThreadModeRawMutex, U, 16>,
    mix_chan: &'e Channel<ThreadModeRawMutex, CmdOrAck<T>, 16>,
    waiters: &'e Mutex<
        ThreadModeRawMutex,
        heapless::FnvIndexMap<u16, Arc<embassy::channel::Signal<()>>, 128>,
    >,
}

impl<'a, 'e, T, U: DeserializeOwned + Hash, UT: Instance> EventInProcessor<'a, 'e, T, U, UT> {
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
                    postcard::FeedResult::DeserError(buf) => buf,
                    postcard::FeedResult::Success { data, remaining } => {
                        let data: CmdOrAck<U> = data;

                        match data {
                            CmdOrAck::Cmd(c) => {
                                if let Some(c) = c.validate() {
                                    self.mix_chan.send(CmdOrAck::Ack(c.ack())).await;
                                    self.out_chan.send(c.cmd).await;
                                }
                            }
                            CmdOrAck::Ack(a) => {
                                if let Some(a) = a.validate() {
                                    let mut waiters = self.waiters.lock().await;
                                    if let Some(waker) = waiters.remove(&a.uuid) {
                                        waker.signal(());
                                    }
                                }
                            }
                        }

                        remaining
                    }
                }
            }
        }
    }

    pub async fn task(&mut self) {
        loop {
            let _ = self.recv_task_inner().await;
        }
    }
}

impl<'a, 'e, T: Serialize, UT: Instance> EventOutProcessor<'a, 'e, T, UT> {
    pub async fn task(&mut self) {
        loop {
            let val = self.mix_chan.recv().await;

            let mut buf = [0u8; BUF_SIZE];
            if let Some(buf) =
                postcard::serialize_with_flavor(&val, Cobs::try_new(Slice::new(&mut buf)).unwrap())
                    .ok()
            {
                let _ = self.tx.write(buf).await.ok();
            }
        }
    }
}

impl<'a, T: Hash + Clone> EventSender<'a, T> {
    pub async fn send(&self, cmd: T) {
        loop {
            let cmd = Command::new(cmd.clone());
            let uuid = cmd.uuid;
            let waiter: Arc<Signal<()>> = self.register_waiter(uuid).await;
            self.mix_chan.send(CmdOrAck::Cmd(cmd)).await;

            match with_timeout(Duration::from_millis(100), waiter.wait()).await {
                Ok(_) => return,
                Err(_) => {
                    self.deregister_waiter(uuid).await;
                }
            }
        }
    }

    async fn register_waiter(&self, uuid: u16) -> Arc<Signal<()>> {
        let signal = Arc::new(Signal::<()>::new());
        let mut waiters = self.waiters.lock().await;
        if let Ok(_) = waiters.insert(uuid, signal.clone()) {
            signal
        } else {
            panic!("Duped waiter uuid")
        }
    }

    async fn deregister_waiter(&self, uuid: u16) {
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

    pub fn split(
        &mut self,
    ) -> (
        EventSender<T>,
        EventOutProcessor<'a, '_, T, UT>,
        EventInProcessor<'a, '_, T, U, UT>,
    ) {
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

        (sender, out_processor, in_processor)
    }
}
