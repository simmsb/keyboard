use keyberon::action::{k, l, Action};
use keyberon::key_code::KeyCode;

pub type CustomEvent = core::convert::Infallible;

const L1_SP: Action<CustomEvent> = Action::HoldTap {
    timeout: 200,
    hold: &l(1),
    tap: &k(KeyCode::Space),
    config: keyberon::action::HoldTapConfig::Default,
    tap_hold_interval: 0,
};

const L2_SP: Action<CustomEvent> = Action::HoldTap {
    timeout: 200,
    hold: &l(2),
    tap: &k(KeyCode::Space),
    config: keyberon::action::HoldTapConfig::Default,
    tap_hold_interval: 0,
};

#[rustfmt::skip]
pub static LAYERS: keyberon::layout::Layers<12, 4, 1, CustomEvent> = keyberon::layout::layout! {
    {
        ['`' Q W E R T Y U I O P '\''],
        [LShift A S D F G H J K L ; RShift],
        [LCtrl Z X C V B N M , . / RCtrl],
        [n n n LGui LAlt {L1_SP} {L2_SP} Tab BSpace n n n],
    }
};
