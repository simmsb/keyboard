use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use embassy::time::{Duration, Ticker};
use futures::StreamExt;
use heapless::HistoryBuffer;

pub const CPM_PERIOD: Duration = Duration::from_secs(3);
pub const CPM_SAMPLES: usize = 32;
pub const CPM_RATE: Duration = Duration::from_ticks(CPM_PERIOD.as_ticks() / CPM_SAMPLES as u64);

pub struct Cpm {
    total: &'static AtomicU32,
    samples: HistoryBuffer<u8, CPM_SAMPLES>,
    avg: &'static AtomicF32,
}

impl Cpm {
    pub fn new(total: &'static AtomicU32, avg: &'static AtomicF32) -> Self {
        Self {
            total,
            samples: Default::default(),
            avg,
        }
    }

    fn sample(&mut self, sample: u8) {
        self.samples.write(sample);
        self.avg.store(
            self.samples.iter().map(|s| *s as u16).sum::<u16>() as f32
                / (self.samples.len() as f32 / CPM_PERIOD.as_secs() as f32),
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

        cpm.sample(diff as u8);

        last = current;

        ticker.next().await;
    }
}
