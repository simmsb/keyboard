use std::{path::PathBuf, time::Duration};

use clap::Parser;
use color_eyre::Result;

#[derive(Debug, clap::Parser)]
struct Opts {
    #[clap(parse(from_os_str))]
    file: PathBuf,

    #[clap(long, short)]
    r#loop: bool,

    port: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    color_eyre::install()?;

    if let Some(path) = opts.port {
        let port = tokio_serial::new(path, 115_200)
            .timeout(Duration::from_millis(100))
            .open()?;
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
