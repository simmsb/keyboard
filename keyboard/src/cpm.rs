use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use embassy::{
    blocking_mutex::raw::ThreadModeRawMutex,
    mutex::Mutex,
    time::{Duration, Ticker},
};
use futures::StreamExt;
use heapless::HistoryBuffer;

pub const CPM_PERIOD: Duration = Duration::from_secs(3);
pub const CPM_SAMPLES: usize = 32;
pub const CPM_RATE: Duration = Duration::from_ticks(CPM_PERIOD.as_ticks() / CPM_SAMPLES as u64);

pub type SampleBuffer = HistoryBuffer<u8, CPM_SAMPLES>;

pub struct Cpm {
    total: &'static AtomicU32,
    samples: &'static Mutex<ThreadModeRawMutex, SampleBuffer>,
    avg: &'static AtomicF32,
}

impl Cpm {
    pub fn new(
        total: &'static AtomicU32,
        avg: &'static AtomicF32,
        samples: &'static Mutex<ThreadModeRawMutex, HistoryBuffer<u8, CPM_SAMPLES>>,
    ) -> Self {
        Self {
            total,
            samples,
            avg,
        }
    }

    async fn sample(&mut self, sample: u8) {
        let mut samples = self.samples.lock().await;
        samples.write(sample);
        self.avg.store(
            samples.iter().map(|s| *s as u16).sum::<u16>() as f32
                / (samples.len() as f32 / CPM_PERIOD.as_secs() as f32),
            core::sync::atomic::Ordering::Relaxed,
        );
    }
}

#[embassy::task]
pub async fn cpm_task(mut cpm: Cpm) {
    let mut ticker = Ticker::every(CPM_RATE);

    let mut last = 0u32;

    loop {
        let current = cpm.total.load(core::sync::atomic::Ordering::Relaxed);
        let diff = current - last;

        cpm.sample(diff as u8).await;

        last = current;

        ticker.next().await;
    }
}
