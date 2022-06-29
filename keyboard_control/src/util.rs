use std::{path::Path, time::Duration};

use color_eyre::Result;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::info;

pub fn open_port(port: Option<&str>) -> Result<SerialStream> {
    if let Some(name) = port {
        return tokio_serial::new(name, 921_600)
            .timeout(Duration::from_millis(100))
            .open_native_async()
            .map_err(Into::into);
    } else {
        let ports = tokio_serial::available_ports()?;

        for port in ports {
            if port.port_name.contains("ttyACM") {
                let name = Path::new(&port.port_name).file_name().ok_or_else(|| {
                    color_eyre::eyre::eyre!("Couldn't get name of port {}", &port.port_name)
                })?;
                let path = Path::new("/dev")
                    .join(name)
                    .into_os_string()
                    .into_string()
                    .unwrap();
                info!("Selected port: {}", path);

                return tokio_serial::new(path, 921_600)
                    .timeout(Duration::from_millis(100))
                    .open_native_async()
                    .map_err(Into::into);
            }
        }
    }

    Err(color_eyre::eyre::eyre!("No ports!"))
}
