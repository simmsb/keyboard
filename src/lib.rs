#![no_main]
#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]
#![feature(alloc_error_handler)]

extern crate alloc;

pub mod layout;
pub mod leds;
pub mod matrix;
pub mod messages;
pub mod oled;

use core::alloc::Layout;

use alloc_cortex_m::CortexMHeap;

#[cfg(feature = "debugger")]
use defmt_rtt as _; // global logger
#[cfg(feature = "debugger")]
use panic_probe as _;

#[cfg(all(not(feature = "debugger"), feature = "log-noop"))]
mod defmt_noop;

#[cfg(not(feature = "debugger"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::asm::udf()
}

#[global_allocator]
static ALLOCATOR: CortexMHeap = CortexMHeap::empty();

pub fn init_heap() {
    {
        use core::mem::MaybeUninit;
        const HEAP_SIZE: usize = 8192;
        static mut HEAP: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { ALLOCATOR.init(HEAP.as_ptr() as usize, HEAP_SIZE) }
    }
}

#[alloc_error_handler]
fn oom(_: Layout) -> ! {
    panic!("oom");
}

// // same panicking *behavior* as `panic-probe` but doesn't print a panic message
// // this prevents the panic message being printed *twice* when `defmt::panic` is invoked
// #[cfg(feature = "debugger")]
// #[defmt::panic_handler]
// fn panic() -> ! {
//     cortex_m::asm::udf()
// }

// /// Terminates the application and makes `probe-run` exit with exit-code = 0
// pub fn exit() -> ! {
//     loop {
//         cortex_m::asm::bkpt();
//     }
// }

// // defmt-test 0.3.0 has the limitation that this `#[tests]` attribute can only be used
// // once within a crate. the module can be in any file but there can only be at most
// // one `#[tests]` module in this library crate
// #[cfg(test)]
// #[defmt_test::tests]
// mod unit_tests {
//     use defmt::assert;

//     #[test]
//     fn it_works() {
//         assert!(true)
//     }
// }
