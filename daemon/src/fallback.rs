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
