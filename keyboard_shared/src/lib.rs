#![no_std]

use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format, Hash, Clone)]
pub enum KeyboardSide {
    Left,
    Right,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format, Hash, Clone)]
pub enum HostToKeyboard {
    RequestStats,
    WritePixels {
        side: KeyboardSide,
        row: u8,
        data: [u8; 4],
    },
}

#[derive(Serialize, Deserialize, Eq, PartialEq, defmt::Format, Hash, Clone)]
pub enum KeyboardToHost {
    Stats { keypresses: u32 },
}
