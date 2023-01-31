#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(alloc_error_handler)]
#![feature(async_fn_in_trait)]
#![feature(impl_trait_projections)]

extern crate alloc;

pub mod async_rw;
pub mod cps;
pub mod event;
pub mod layout;
pub mod leds;
pub mod lhs_display;
pub mod matrix;
pub mod messages;
pub mod oled;
pub mod rhs_display;
pub mod wrapping_id;

use core::alloc::Layout;

use alloc_cortex_m::CortexMHeap;

#[cfg(feature = "debugger")]
use defmt_rtt as _;
use embassy_time::Duration;
use embassy_nrf::uarte;
// global logger
#[cfg(feature = "debugger")]
use panic_probe as _;

pub const UART_BAUD: uarte::Baudrate = uarte::Baudrate::BAUD460800;
pub const POLL_PERIOD: Duration = Duration::from_micros(200);
pub const DEBOUNCER_TICKS: u16 = 50;

#[cfg(all(not(feature = "debugger"), feature = "log-noop"))]
mod defmt_noop;

#[macro_export]
macro_rules! forever {
    ($val:expr) => {{
        type T = impl ::core::marker::Sized;
        static FOREVER: ::static_cell::StaticCell<T> = ::static_cell::StaticCell::new();
        FOREVER.init($val)
    }};
}

#[cfg(feature = "panic-reset")]
use panic_reset as _;

#[global_allocator]
static ALLOCATOR: CortexMHeap = CortexMHeap::empty();

pub fn init_heap() {
    use core::mem::MaybeUninit;
    const HEAP_SIZE: usize = 8192;
    static mut HEAP: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { ALLOCATOR.init(HEAP.as_ptr() as usize, HEAP_SIZE) }
}

#[alloc_error_handler]
fn oom(_: Layout) -> ! {
    panic!("oom");
}
