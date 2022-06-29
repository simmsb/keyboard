use clap::Parser;
use color_eyre::Result;

mod metrics;
mod render;
pub mod util;

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

#[derive(Debug, clap::Parser)]
struct Opts {
    #[clap(subcommand)]
    command: ControlCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ControlCommand {
    /// List possible ports
    Ports,
    Render(crate::render::RenderOpts),
    Metrics(crate::metrics::MetricsOpts),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    color_eyre::install()?;

    install_tracing()?;

    match opts.command {
        ControlCommand::Ports => {
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
        ControlCommand::Render(r) => r.execute().await?,
        ControlCommand::Metrics(m) => m.execute().await?,
    }

    Ok(())
}
