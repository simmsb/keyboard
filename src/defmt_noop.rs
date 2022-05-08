#[defmt::global_logger]
struct NoopLogger;

unsafe impl defmt::Logger for NoopLogger {
    fn acquire() {}

    unsafe fn flush() {}

    unsafe fn release() {}

    unsafe fn write(_bytes: &[u8]) {}
}
