use std::{
    fs::File,
    io::{Seek, SeekFrom},
    path::PathBuf,
    time::Duration,
};

use bitvec::{bitarr, order::Lsb0};
use color_eyre::{eyre::eyre, Help, Result};
use image::{
    imageops::{dither, grayscale, resize, BiLevel, FilterType},
    AnimationDecoder,
};
use itertools::Itertools;
use keyboard_shared::{CmdOrAck, Command, HostToKeyboard};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::Instant,
};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::Instrument;

/// Render a gif to the keyboard displays
#[derive(Debug, clap::Parser)]
pub struct RenderOpts {
    #[clap(parse(from_os_str))]
    file: PathBuf,

    #[clap(long, short)]
    no_loop: bool,

    port: String,
}

impl RenderOpts {
    pub async fn execute(self) -> color_eyre::Result<()> {
        let mut port = tokio_serial::new(&self.port, 921_600)
            .timeout(Duration::from_millis(100))
            .open_native_async()?;

        let mut gif = File::open(&self.file).section("Couldn't find your gif")?;

        loop {
            let decoder =
                image::codecs::gif::GifDecoder::new(&gif).section("Are you sure this is a gif")?;

            for frame in decoder.into_frames() {
                let frame = frame.section("Some frame is borked")?;

                let next_frame = Instant::now() + frame.delay().into();

                let mut image = grayscale(&resize(frame.buffer(), 64, 128, FilterType::Lanczos3));
                dither(&mut image, &BiLevel);
                emit_image(&image, &mut port)
                    .instrument(tracing::info_span!("sending frame", frame_time = ?Duration::from(frame.delay())))
                    .await?;

                tokio::time::sleep_until(next_frame).await;
            }

            if self.no_loop {
                break;
            }

            gif.seek(SeekFrom::Start(0))?;
        }

        Ok(())
    }
}

async fn emit_image(
    image: &image::ImageBuffer<image::Luma<u8>, Vec<u8>>,
    port: &mut SerialStream,
) -> Result<()> {
    let mut lhs = [bitarr![u8, Lsb0; 1; 32]; 128];
    let mut rhs = [bitarr![u8, Lsb0; 1; 32]; 128];

    for (x, y, p) in image.enumerate_pixels() {
        let on_rhs = x > 31;
        let x = if on_rhs { x - 32 } else { x };

        let buf = if on_rhs { &mut rhs } else { &mut lhs };
        buf[y as usize].set(x as usize, p.0[0] > 127);
    }

    let mut o_buf = Vec::new();

    let lhs_iter = lhs.chunks_exact(2).enumerate().map(|(row_idx, rows)| {
        let cmd = HostToKeyboard::WritePixels {
            side: keyboard_shared::KeyboardSide::Left,
            row: (2 * row_idx) as u8,
            data_0: rows[0].data,
            data_1: rows[1].data,
        };
        CmdOrAck::Cmd(Command::new(cmd))
    });

    let rhs_iter = rhs.chunks_exact(2).enumerate().map(|(row_idx, rows)| {
        let cmd = HostToKeyboard::WritePixels {
            side: keyboard_shared::KeyboardSide::Right,
            row: (2 * row_idx) as u8,
            data_0: rows[0].data,
            data_1: rows[1].data,
        };
        CmdOrAck::Cmd(Command::new(cmd))
    });

    // let rhs_iter = std::iter::empty();

    for cmd in lhs_iter.interleave(rhs_iter) {
        let buf = postcard::to_allocvec_cobs(&cmd).map_err(|e| eyre!("Serde error: {}", e))?;
        if (o_buf.len() + buf.len()) > 64 {
            port.write_all(&o_buf).await?;
            o_buf.clear();
            let mut buf = [0u8; 128];
            let _ = tokio::time::timeout(Duration::from_micros(100), port.read(&mut buf)).await;
        }
        o_buf.extend_from_slice(&buf);
    }

    if !o_buf.is_empty() {
        let _ = port.write_all(&o_buf).await;
        port.write_all(&o_buf)
            .instrument(tracing::debug_span!("sending remainder", len = o_buf.len()))
            .await?;
        let mut buf = [0u8; 128];
        let _ = tokio::time::timeout(Duration::from_micros(100), port.read(&mut buf)).await;
    }

    Ok(())
}
