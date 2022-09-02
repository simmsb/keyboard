use core::sync::atomic::AtomicU32;

use atomic_float::AtomicF32;
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    mutex::Mutex,
};
use embassy_time::{Duration, Ticker};
use futures::StreamExt;
use heapless::HistoryBuffer;

pub const CPS_PERIOD: Duration = Duration::from_secs(3);
pub const CPS_SAMPLES: usize = 32;
pub const CPS_RATE: Duration = Duration::from_ticks(CPS_PERIOD.as_ticks() / CPS_SAMPLES as u64);

pub type SampleBuffer = HistoryBuffer<u8, CPS_SAMPLES>;

pub struct Cps {
    total: &'static AtomicU32,
    samples: &'static Mutex<ThreadModeRawMutex, SampleBuffer>,
    avg: &'static AtomicF32,
}

impl Cps {
    pub fn new(
        total: &'static AtomicU32,
        avg: &'static AtomicF32,
        samples: &'static Mutex<ThreadModeRawMutex, HistoryBuffer<u8, CPS_SAMPLES>>,
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
                / (samples.len() as f32 / CPS_PERIOD.as_secs() as f32),
            core::sync::atomic::Ordering::Relaxed,
        );
    }
}

#[embassy_executor::task]
pub async fn cps_task(mut cps: Cps) {
    let mut ticker = Ticker::every(CPS_RATE);

    let mut last = 0u32;

    loop {
        let current = cps.total.load(core::sync::atomic::Ordering::Relaxed);
        let diff = current - last;

        cps.sample(diff as u8).await;

        // defmt::debug!("kp: {}, tot: {}",
        //        AVERAGE_KEYPRESSES.load(core::sync::atomic::Ordering::Relaxed),
        //               current
        // );

        last = current;

        ticker.next().await;
    }
}
