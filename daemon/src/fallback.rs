//! The layout used until a session agent connects (login screen, early boot,
//! or the agent isn't running). Keys only, so it needs nothing from the session.

use crate::proto::*;
use input_linux::Key;
use std::collections::HashMap;

fn key(icon: &str, k: Key, w: f64) -> Item {
    Item { icon: Some(icon.into()), act: Some(Action::Key(vec![k])), w: Some(w), ..Default::default() }
}

fn esc() -> Item {
    Item {
        label: Some("esc".into()),
        act: Some(Action::Key(vec![Key::Esc])),
        w: Some(96.0),
        pin: true,
        ..Default::default()
    }
}

/// Touch Bar MacBook Pros without a physical Escape key (2016-2019).
const NO_PHYSICAL_ESC: [&str; 8] = [
    "MacBookPro13,2", "MacBookPro13,3", "MacBookPro14,2", "MacBookPro14,3",
    "MacBookPro15,1", "MacBookPro15,2", "MacBookPro15,3", "MacBookPro15,4",
];

/// True when this Mac has a real Esc key. Unknown machines get the on-bar Esc.
pub fn has_physical_esc() -> bool {
    let model = std::fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default();
    let model = model.trim();
    if model.is_empty() {
        let compat = std::fs::read("/sys/firmware/devicetree/base/compatible").unwrap_or_default();
        let has = |needle: &[u8]| compat.windows(needle.len()).any(|w| w == needle);
        return has(b"apple,t8103") || has(b"apple,t8112");
    }
    (model.starts_with("MacBookPro") || model.starts_with("Mac1")) && !NO_PHYSICAL_ESC.contains(&model)
}

pub fn layout() -> Layout {
    let control = vec![
        esc(),
        Item {
            label: Some("fn".into()),
            act: Some(Action::ToggleLayer("function".into())),
            w: Some(80.0),
            style: Style::Subtle,
            ..Default::default()
        },
        Item { kind: Kind::Flex, ..Default::default() },
        key("󰃞", Key::BrightnessDown, 96.0),
        key("󰃠", Key::BrightnessUp, 96.0),
        Item { kind: Kind::Gap, w: Some(12.0), ..Default::default() },
        key("󰌌", Key::IllumDown, 96.0),
        key("󰌌", Key::IllumUp, 96.0),
        Item { kind: Kind::Gap, w: Some(12.0), ..Default::default() },
        Item { kind: Kind::Media, w: Some(270.0), ..Default::default() },
        Item { kind: Kind::Gap, w: Some(12.0), ..Default::default() },
        key("󰖁", Key::Mute, 96.0),
        key("󰕿", Key::VolumeDown, 96.0),
        key("󰕾", Key::VolumeUp, 96.0),
        Item { kind: Kind::Gap, w: Some(150.0), ..Default::default() },
    ];
    let fkeys = [
        Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
        Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
    ];
    let mut function = vec![esc()];
    function.extend(fkeys.iter().enumerate().map(|(i, k)| Item {
        label: Some(format!("F{}", i + 1)),
        act: Some(Action::Key(vec![*k])),
        ..Default::default()
    }));
    function.push(Item {
        icon: Some("󰁍".into()),
        act: Some(Action::Layer("control".into())),
        w: Some(80.0),
        style: Style::Subtle,
        ..Default::default()
    });
    // The on-bar Esc only on Macs without a physical one.
    let (control, function) = if has_physical_esc() {
        let drop = |v: Vec<Item>| v.into_iter().filter(|i| i.act != Some(Action::Key(vec![Key::Esc]))).collect();
        (drop(control), drop(function))
    } else {
        (control, function)
    };
    let mut layers = HashMap::new();
    layers.insert("control".to_string(), control);
    layers.insert("function".to_string(), function);
    Layout {
        layers,
        default: "control".into(),
        fn_layer: Some("function".into()),
        settings: Settings::default(),
    }
}
