use std::time::Duration;

use color_eyre::{eyre::eyre, Result};
use keyboard_shared::{CmdOrAck, Command, HostToKeyboard, KeyboardToHost};
use once_cell::sync::Lazy;
use postcard::CobsAccumulator;
use prometheus::{register_int_counter, IntCounter, ProtobufEncoder, Encoder};
use reqwest::{Client, header::CONTENT_TYPE, StatusCode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    select,
    time::interval,
};
use tokio_serial::SerialPortBuilderExt;

static KEYPRESS_COUNTER: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!("total_keypresses", "Total number of keys pressed").unwrap()
});

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
});

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
        let mut count = 0u64;
        println!("counter: {}", KEYPRESS_COUNTER.get());

        loop {
            let buf = select! {
                _ = interval.tick() => {
                    let cmd = CmdOrAck::Cmd(Command::new(HostToKeyboard::RequestStats));
                    let send_buf = postcard::to_allocvec_cobs(&cmd).map_err(|e| eyre!("Serde error: {}", e))?;
                    let _ = port.write_all(&send_buf).await;
                    None
                },
                Ok(len) = port.read(&mut buf) => {
                    Some(&buf[..len])
                }
            };

            if let Some(mut window) = buf {
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
                                        let send_buf = postcard::to_allocvec_cobs(&ack)
                                            .map_err(|e| eyre!("Serde error: {}", e))?;
                                        let _ = port.write_all(&send_buf).await;
                                        match c.cmd {
                                            KeyboardToHost::Stats { keypresses } => {
                                                let keypresses = keypresses as u64;
                                                let delta = keypresses - count;
                                                KEYPRESS_COUNTER.inc_by(delta);
                                                count = keypresses;

                                                push_metrics(&self.prometheus_gateway).await?;
                                            }
                                        }
                                    } else {
                                    }
                                }
                                CmdOrAck::Ack(_) => {}
                            }

                            remaining
                        }
                    }
                }
            }
        }
    }
}


async fn push_metrics(url: &url::Url) -> Result<()> {
    let url = url.join("/metrics/job/keyboard_worker")?;

    let encoder = ProtobufEncoder::new();
    let mut buf = Vec::new();

    let metric_families = prometheus::gather();

    for mf in metric_families {
        for m in mf.get_metric() {
            for lp in m.get_label() {
                if lp.get_name() == "job" {
                    return Err(eyre!("Metric {} already contains job label", mf.get_name()));
                }
            }
        }

        let _ = encoder.encode(&[mf], &mut buf);
    }

    let builder = HTTP_CLIENT.request(reqwest::Method::PUT, url.clone())
        .header(CONTENT_TYPE, encoder.format_type())
        .body(buf);

    let resp = builder.send().await?;

    match resp.status() {
        StatusCode::ACCEPTED => Ok(()),
        StatusCode::OK => Ok(()),
        s => Err(eyre!("Bad status {} when pushing to {}", s, url)),
    }
}
