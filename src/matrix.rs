#[macro_export]
macro_rules! build_matrix {
    ($gpios_p0:ident, $gpios_p1:ident) => {{
        use keyberon::matrix::Matrix;
        use nrf52840_hal::gpio::{p0, p1};
        Matrix::new(
            [
                $gpios_p0.p0_31.into_pullup_input().degrade(),
                $gpios_p0.p0_29.into_pullup_input().degrade(),
                $gpios_p0.p0_02.into_pullup_input().degrade(),
                $gpios_p1.p1_15.into_pullup_input().degrade(),
                $gpios_p1.p1_13.into_pullup_input().degrade(),
                $gpios_p1.p1_11.into_pullup_input().degrade(),
            ],
            [
                $gpios_p0
                    .p0_22
                    .into_push_pull_output(nrf52840_hal::gpio::Level::High)
                    .degrade(),
                $gpios_p0
                    .p0_24
                    .into_push_pull_output(nrf52840_hal::gpio::Level::High)
                    .degrade(),
                $gpios_p1
                    .p1_00
                    .into_push_pull_output(nrf52840_hal::gpio::Level::High)
                    .degrade(),
                $gpios_p0
                    .p0_11
                    .into_push_pull_output(nrf52840_hal::gpio::Level::High)
                    .degrade(),
            ],
        )
        .unwrap()
    }};
}
