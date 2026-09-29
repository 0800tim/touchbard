//! Messages exchanged with the session agent over the Unix socket, one JSON
//! object per line. The agent owns everything that lives in the user's
//! session (theme, config, volume, fingerprint); the daemon owns the hardware.

use input_linux::Key;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct Theme {
    pub bg: String,
    pub surface: String,
    pub surface_hi: String,
    pub fg: String,
    pub fg_dim: String,
    pub accent: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    /// Second colour for gradients (the theme's magenta).
    pub accent2: String,
    pub font: String,
}

impl Default for Theme {
    // Tokyo Night, so the bar looks right before the agent has connected.
    fn default() -> Self {
        Theme {
            bg: "#0e0e14".into(),
            surface: "#24283b".into(),
            surface_hi: "#414868".into(),
            fg: "#c0caf5".into(),
            fg_dim: "#565f89".into(),
            accent: "#7aa2f7".into(),
            red: "#f7768e".into(),
            green: "#9ece6a".into(),
            yellow: "#e0af68".into(),
            accent2: "#bb9af7".into(),
            font: "JetBrainsMono Nerd Font".into(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Button,
    Slider,
    Media,
    Nowplaying,
    Clock,
    Battery,
    Workspaces,
    /// A surface drawn by a plugin process.
    Plugin,
    Gap,
    Flex,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    #[default]
    Normal,
    Accent,
    Danger,
    Subtle,
    Plain,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Press these keys while touched (a chord when more than one).
    Key(Vec<Key>),
    /// Ask the agent to run a shell command in the user's session.
    Cmd(String),
    /// Show another layer until something switches back.
    Layer(String),
    /// Switch to a layer, or back to the default one if it is already shown.
    ToggleLayer(String),
    /// Expand the music visualiser over the bar.
    Visualizer,
    /// Flip a boolean state key (e.g. "eq") and tell the agent.
    ToggleFlag(String),
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Item {
    pub kind: Kind,
    pub icon: Option<String>,
    pub label: Option<String>,
    /// Fixed width in pixels. Items without it share the leftover space.
    pub w: Option<f64>,
    pub flex: Option<f64>,
    pub act: Option<Action>,
    pub style: Style,
    /// A boolean state key; when true the `*_on` variants are drawn.
    pub toggle: Option<String>,
    pub icon_on: Option<String>,
    pub label_on: Option<String>,
    pub style_on: Option<Style>,
    /// Slider: the state key it shows and sets (volume, brightness, keyboard).
    pub target: Option<String>,
    /// Slider: icons from lowest to highest level.
    pub icons: Vec<String>,
    /// Slider: a boolean state key that means "muted".
    pub mute_key: Option<String>,
    pub mute_icon: Option<String>,
    /// Clock: strftime format.
    pub format: Option<String>,
    /// Stays visible when a slider is expanded over the layer.
    pub pin: bool,
    /// Plugin: its name, and options passed through to it.
    pub plugin: Option<String>,
    pub options: Option<Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub dim_after: u64,
    pub off_after: u64,
    pub max_brightness: u32,
    pub slider_timeout: u64,
    pub fingerprint_width: f64,
    pub dim_while_scanning: bool,
    /// What the expanded visualiser can show, in the order its mode button
    /// cycles: "bars" (built in) or plugin names.
    pub visualizers: Vec<String>,
    /// Behind the now-playing title: "bars" (live equaliser), "art" (album
    /// artwork beside the title) or "off".
    pub nowplaying: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dim_after: 30,
            off_after: 60,
            max_brightness: 160,
            slider_timeout: 5,
            fingerprint_width: 330.0,
            dim_while_scanning: true,
            visualizers: vec!["bars".into()],
            nowplaying: "bars".into(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Layout {
    pub layers: HashMap<String, Vec<Item>>,
    pub default: String,
    /// Shown while the physical fn key is held.
    pub fn_layer: Option<String>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Incoming {
    Theme(Theme),
    Layout(Layout),
    State {
        #[serde(flatten)]
        values: HashMap<String, Value>,
    },
    Fingerprint {
        s: FingerState,
    },
    Layer {
        name: String,
    },
    /// Spectrum from cava, 0..1 per band, low to high.
    Bars {
        v: Vec<f32>,
    },
    /// Album art as base64 PNG; empty clears it.
    Art {
        png: String,
    },
    /// Header for a plugin frame: `len` bytes of BGRA follow the newline.
    Pixels {
        id: String,
        w: u32,
        h: u32,
        len: usize,
    },
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FingerState {
    Idle,
    Scan,
    Retry,
    Match,
    Fail,
}

#[derive(Serialize, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Outgoing {
    Hello,
    Run { cmd: String },
    Set { k: String, v: f64 },
    /// The plugin surfaces now on screen; the agent runs exactly these.
    Surfaces { list: Vec<SurfaceSpec> },
    /// A tap or a command for a plugin (e.g. "next" for the next preset).
    PluginCmd { id: String, cmd: String, x: f64 },
    Seek { pos: f64 },
    /// The bar went dark or came back; the agent pauses cava meanwhile.
    Power { on: bool },
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SurfaceSpec {
    pub id: String,
    pub w: u32,
    pub h: u32,
    pub options: Option<Value>,
}
