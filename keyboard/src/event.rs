use embassy_sync::signal::Signal;

pub struct Event(Signal<embassy_sync::blocking_mutex::raw::ThreadModeRawMutex, ()>);

impl Event {
    pub const fn new() -> Self {
        Self(Signal::new())
    }

    pub async fn wait(&self) {
        self.0.wait().await;
        self.0.reset();
    }

    pub fn set(&self) {
        self.0.signal(());
    }
}
