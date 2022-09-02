use defmt::debug;
use embassy_futures::select::select;
use embassy_nrf::uarte::{self, UarteRx, UarteTx};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_usb::driver::{Driver, EndpointError};
use embassy_usb_serial::CdcAcmClass;
use futures::Future;

pub trait AsyncRead {
    type Error;
    type Fut<'a>: Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::Fut<'a>;
}

pub trait AsyncWrite {
    type Error;
    type Fut<'a>: Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::Fut<'a>;
}

impl<'d, T: uarte::Instance> AsyncRead for UarteRx<'d, T> {
    type Error = uarte::Error;

    type Fut<'a> = impl Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    #[inline]
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::Fut<'a> {
        UarteRx::read(self, buf)
    }
}

impl<'d, T: uarte::Instance> AsyncWrite for UarteTx<'d, T> {
    type Error = uarte::Error;

    type Fut<'a> = impl Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    #[inline]
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::Fut<'a> {
        UarteTx::write(self, buf)
    }
}

impl<const N: usize> AsyncRead for &Channel<ThreadModeRawMutex, u8, N> {
    type Error = ();

    type Fut<'a> = impl Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    #[inline]
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::Fut<'a> {
        async {
            for p in buf.iter_mut() {
                *p = self.recv().await;
            }
            Ok(())
        }
    }
}

impl<const N: usize> AsyncWrite for &Channel<ThreadModeRawMutex, u8, N> {
    type Error = ();

    type Fut<'a> = impl Future<Output = Result<(), Self::Error>>
    where
        Self: 'a;

    #[inline]
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::Fut<'a> {
        async move {
            for b in buf {
                self.send(*b).await;
            }
            Ok(())
        }
    }
}

pub struct UsbSerialWrapper<'a, 'd, D: Driver<'d>, const N: usize> {
    class: &'a mut CdcAcmClass<'d, D>,
    in_chan: &'static Channel<ThreadModeRawMutex, u8, N>,
    out_chan: &'static Channel<ThreadModeRawMutex, u8, N>,
}

impl<'a, 'd, D: Driver<'d>, const N: usize> UsbSerialWrapper<'a, 'd, D, N> {
    pub fn new(
        class: &'a mut CdcAcmClass<'d, D>,
        in_chan: &'static Channel<ThreadModeRawMutex, u8, N>,
        out_chan: &'static Channel<ThreadModeRawMutex, u8, N>,
    ) -> Self {
        Self {
            class,
            in_chan,
            out_chan,
        }
    }

    pub async fn run(&mut self) -> Result<(), EndpointError> {
        loop {
            let a = async {
                let mut v = heapless::Vec::<u8, 64>::new();

                v.push(self.in_chan.recv().await).unwrap();

                while let Ok(x) = self.in_chan.try_recv() {
                    v.push(x).unwrap();

                    if v.is_full() {
                        break;
                    }
                }

                debug!("Sent a serial packet of length {}", v.len());

                v
            };

            let b = async {
                let mut v = [0u8; N];

                let n = self.class.read_packet(&mut v).await?;

                Ok(heapless::Vec::<u8, N>::from_slice(&v[..n]).unwrap())
            };

            match select(a, b).await {
                embassy_futures::select::Either::First(to_pc) => {
                    self.class.write_packet(&to_pc).await?;
                    if to_pc.len() as u16 == self.class.max_packet_size() {
                        self.class.write_packet(&[]).await?;
                    }
                }
                embassy_futures::select::Either::Second(from_pc) => {
                    let from_pc = from_pc?;
                    for b in from_pc {
                        self.out_chan.send(b).await;
                    }
                }
            }
        }
    }
}
