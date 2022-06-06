use std::{fs::File, io::Seek, path::PathBuf, time::Duration};

use bitvec::{bitarr, order::Lsb0};
use clap::Parser;
use color_eyre::{eyre::eyre, Help, Result};
use image::{
    imageops::{dither, grayscale, resize, BiLevel, FilterType},
    AnimationDecoder,
};
use keyboard_shared::{CmdOrAck, Command, HostToKeyboard};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::Instrument;

#[derive(Debug, clap::Parser)]
struct Opts {
    #[clap(parse(from_os_str))]
    file: PathBuf,

    #[clap(long, short)]
    no_loop: bool,

    port: Option<String>,
}
fn install_tracing() -> color_eyre::Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    use tracing_subscriber::fmt::format::FmtSpan;
    let fmt_layer = tracing_subscriber::fmt::layer().with_span_events(FmtSpan::CLOSE);
    // .pretty();
    let filter_layer = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::default()
            .add_directive("keyboard_control=info".parse().unwrap())
    });

    // let (flame_layer, guard) =
    // tracing_flame::FlameLayer::with_file("./tracing.folded").unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(tracing_error::ErrorLayer::default())
        .with(fmt_layer)
        // .with(flame_layer)
        .init();

    // return Ok(Box::new(guard));

    Ok(())
    //Ok(Box::new(()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    color_eyre::install()?;

    install_tracing()?;

    if let Some(path) = opts.port {
        let mut port = tokio_serial::new(path, 921_600)
            .timeout(Duration::from_millis(100))
            .open_native_async()?;

        let gif = File::open(&opts.file).section("Couldn't find your gif")?;

        let mut frames = Vec::new();

        let decoder =
            image::codecs::gif::GifDecoder::new(&gif).section("Are you sure this is a gif")?;
        for frame in decoder.into_frames() {
            let frame = frame.section("Some frame is borked")?;

            let mut image = grayscale(&resize(frame.buffer(), 64, 128, FilterType::Lanczos3));
            dither(&mut image, &BiLevel);
            emit_image(&image, &mut port).await?;
            frames.push(image);
        }

        loop {
            for frame in &frames {
                emit_image(frame, &mut port).await?;
            }

            if opts.no_loop {
                break;
            }
        }
    } else {
        let ports = tokio_serial::available_ports()?;

        if ports.is_empty() {
            println!("No ports found");
        } else {
            println!("The following ports were found:");
            for port in ports {
                println!("{}: {:?}", port.port_name, port.port_type);
            }
        }
    }

    Ok(())
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

    for (row_idx, row) in lhs.iter().enumerate() {
        let cmd = HostToKeyboard::WritePixels {
            side: keyboard_shared::KeyboardSide::Left,
            row: row_idx as u8,
            data: row.data,
        };
        let cmd = CmdOrAck::Cmd(Command::new(cmd));

        let buf = postcard::to_allocvec_cobs(&cmd).map_err(|e| eyre!("Serde error: {}", e))?;
        if (o_buf.len() + buf.len()) > 64 {
            port.write_all(&o_buf)
                .instrument(tracing::info_span!("sending row", row_idx, len = o_buf.len()))
                .await?;
            o_buf.clear();
            let mut buf = [0u8; 128];
            let _ = tokio::time::timeout(Duration::from_micros(100), port.read(&mut buf)).await;
        }
        o_buf.extend_from_slice(&buf);
    }

    for (row_idx, row) in rhs.iter().enumerate() {
        let cmd = HostToKeyboard::WritePixels {
            side: keyboard_shared::KeyboardSide::Right,
            row: row_idx as u8,
            data: row.data,
        };
        let cmd = CmdOrAck::Cmd(Command::new(cmd));

        let buf = postcard::to_allocvec_cobs(&cmd).map_err(|e| eyre!("Serde error: {}", e))?;
        if (o_buf.len() + buf.len()) > 64 {
            port.write_all(&o_buf)
                .instrument(tracing::info_span!("sending row", row_idx, len = o_buf.len()))
                .await?;
            o_buf.clear();
            let mut buf = [0u8; 128];
            let _ = tokio::time::timeout(Duration::from_micros(100), port.read(&mut buf)).await;
        }
        o_buf.extend_from_slice(&buf);
    }

    if !o_buf.is_empty() {
        let _ = port.write_all(&o_buf).await;
        port.write_all(&o_buf)
            .instrument(tracing::info_span!("sending remainder", len = o_buf.len()))
            .await?;
        let mut buf = [0u8; 128];
        let _ = tokio::time::timeout(Duration::from_micros(100), port.read(&mut buf)).await;
    }

    Ok(())
}
