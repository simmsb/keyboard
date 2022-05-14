#[macro_export]
macro_rules! build_matrix {
    ($p:ident) => {{
        use embassy_nrf::gpio::{Input, Level, OutputDrive, Pull, Pin};
        use keyberon::matrix::Matrix;
        Matrix::new(
            [
                Input::new($p.P0_31.degrade(), Pull::Up),
                Input::new($p.P0_29.degrade(), Pull::Up),
                Input::new($p.P0_02.degrade(), Pull::Up),
                Input::new($p.P1_15.degrade(), Pull::Up),
                Input::new($p.P1_13.degrade(), Pull::Up),
                Input::new($p.P1_11.degrade(), Pull::Up),
            ],
            [
                Output::new($p.P0_22.degrade(), Level::High, OutputDrive::Standard),
                Output::new($p.P0_24.degrade(), Level::High, OutputDrive::Standard),
                Output::new($p.P1_00.degrade(), Level::High, OutputDrive::Standard),
                Output::new($p.P0_11.degrade(), Level::High, OutputDrive::Standard),
            ],
        )
        .unwrap()
    }};
}
