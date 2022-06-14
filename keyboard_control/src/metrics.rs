use std::time::Duration;

use color_eyre::eyre::eyre;
use keyboard_shared::{CmdOrAck, Command, HostToKeyboard, KeyboardToHost};
use postcard::CobsAccumulator;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    time::interval,
};
use tokio_serial::SerialPortBuilderExt;

/// Extract metrics from the keyboard
#[derive(Debug, clap::Parser)]
pub struct MetricsOpts {
    #[clap(short, long, default_value = "http://127.0.0.1:9091")]
    prometheus_gateway: url::Url,

    port: String,
}

impl MetricsOpts {
    pub async fn execute(self) -> color_eyre::Result<()> {
        let mut port = tokio_serial::new(&self.port, 921_600)
            .timeout(Duration::from_millis(100))
            .open_native_async()?;

        let mut interval = interval(Duration::from_secs(5));
        let mut buf = [0u8; 64];
        let mut accumulator = CobsAccumulator::<128>::new();

        loop {
            select! {
                _ = interval.tick() => {
                    let cmd = CmdOrAck::Cmd(Command::new(HostToKeyboard::RequestStats));
                    let send_buf = postcard::to_allocvec_cobs(&cmd).map_err(|e| eyre!("Serde error: {}", e))?;
                    let _ = port.write_all(&send_buf).await;

                },
                Ok(len) = port.read(&mut buf) => {
                    let mut window = &buf[..len];
                    'cobs: while !window.is_empty() {
                        window = match accumulator.feed(window) {
                            postcard::FeedResult::Consumed => break 'cobs,
                            postcard::FeedResult::OverFull(buf) => buf,
                            postcard::FeedResult::DeserError(buf) => buf,
                            postcard::FeedResult::Success { data, remaining } => {
                                let data: CmdOrAck<KeyboardToHost> = data;

                                match data {
                                    CmdOrAck::Cmd(c) => {
                                        if c.validate() {
                                            println!("{:?}", c);
                                            let ack = CmdOrAck::<HostToKeyboard>::Ack(c.ack());
                                            let send_buf = postcard::to_allocvec_cobs(&ack).map_err(|e| eyre!("Serde error: {}", e))?;
                                            let _ = port.write_all(&send_buf).await;
                                        } else {
                                        }
                                    }
                                    CmdOrAck::Ack(_) => {
                                    }
                                }

                                remaining
                            }
                        }
                    }
                }
            }
        }
    }
}
