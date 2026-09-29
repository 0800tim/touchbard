//! What is on the bar and how touches change it. No hardware here, so the
//! same model drives both the Touch Bar and `--preview` PNG renders.

use crate::proto::*;
use input_linux::Key;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const SPACING: f64 = 6.0;
const DRAG_THRESHOLD: f64 = 6.0;
const SLIDER_STEP: f64 = 1.0 / 16.0;
const STATE_HOLD: Duration = Duration::from_millis(700);
pub const PRESS_FADE: Duration = Duration::from_millis(180);
pub const FP_FADE_IN: Duration = Duration::from_millis(220);
pub const FP_FADE_OUT: Duration = Duration::from_millis(280);

/// Something the daemon has to do in response to a touch.
pub enum Effect {
    KeyDown(Vec<Key>),
    KeyUp(Vec<Key>),
    Send(Outgoing),
}

/// Parts of the expanded slider, in drawing order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Pinned(usize),
    Close,
    Low,
    Track,
    High,
    Value,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hit {
    Item(usize),
    Media(usize, u8),
    Overlay(Part),
}

enum Grab {
    /// A button: keys held, or an action that fires on release.
    Press { hit: Hit, keys: Vec<Key>, act: Option<Action>, inside: bool },
    /// Dragging straight from a collapsed slider button, macOS style.
    SliderDrag { start_x: f64, start_v: f64, moved: bool },
    /// Dragging along the expanded slider's track.
    Track,
    Ignore,
}

pub struct SliderOverlay {
    pub item: Item,
    pub key: String,
    pub last_touch: Instant,
    pub close_at: Option<Instant>,
}

pub struct Finger {
    pub state: FingerState,
    /// The last non-idle state, drawn while fading out.
    pub shown_state: FingerState,
    pub since: Instant,
    pub shown_at: Instant,
    pub hidden_at: Option<Instant>,
    pub retry_at: Option<Instant>,
}

pub struct Model {
    pub w: f64,
    pub h: f64,
    pub theme: Theme,
    pub layout: Layout,
    pub layer: String,
    pub fn_held: bool,
    pub state: HashMap<String, Value>,
    pub slider: Option<SliderOverlay>,
    pub finger: Finger,
    pub pressed: HashMap<Hit, (bool, Instant)>,
    grabs: HashMap<u32, Grab>,
    hold: HashMap<String, Instant>,
}

impl Model {
    pub fn new(w: f64, h: f64, theme: Theme, layout: Layout) -> Model {
        let now = Instant::now();
        let layer = layout.default.clone();
        Model {
            w,
            h,
            theme,
            layout,
            layer,
            fn_held: false,
            state: HashMap::new(),
            slider: None,
            finger: Finger {
                state: FingerState::Idle,
                shown_state: FingerState::Scan,
                since: now,
                shown_at: now,
                hidden_at: None,
                retry_at: None,
            },
            pressed: HashMap::new(),
            grabs: HashMap::new(),
            hold: HashMap::new(),
        }
    }

    pub fn set_layout(&mut self, layout: Layout) -> Vec<Effect> {
        let fx = self.release_all();
        if !layout.layers.contains_key(&self.layer) {
            self.layer = layout.default.clone();
        }
        self.layout = layout;
        self.slider = None;
        fx
    }

    pub fn active_layer_name(&self) -> &str {
        if self.fn_held {
            if let Some(f) = &self.layout.fn_layer {
                if self.layout.layers.contains_key(f) {
                    return f;
                }
            }
        }
        if self.layout.layers.contains_key(&self.layer) {
            &self.layer
        } else {
            &self.layout.default
        }
    }

    pub fn items(&self) -> &[Item] {
        self.layout
            .layers
            .get(self.active_layer_name())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn switch_layer(&mut self, name: &str) -> Vec<Effect> {
        let fx = self.release_all();
        if self.layout.layers.contains_key(name) {
            self.layer = name.to_string();
        }
        self.slider = None;
        fx
    }

    pub fn set_fn(&mut self, held: bool) -> Vec<Effect> {
        if self.fn_held == held {
            return vec![];
        }
        let fx = self.release_all();
        self.fn_held = held;
        fx
    }

    // ---- state ------------------------------------------------------------

    pub fn update_state(&mut self, values: HashMap<String, Value>) {
        let now = Instant::now();
        for (k, v) in values {
            // Don't let a slow echo from the agent yank the knob mid-drag.
            if self.hold.get(&k).is_some_and(|t| *t > now) {
                continue;
            }
            self.state.insert(k, v);
        }
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        self.state.get(key).and_then(|v| v.as_f64())
    }

    pub fn flag(&self, key: &str) -> bool {
        // `layer:<name>` is true while that layer is on screen.
        if let Some(layer) = key.strip_prefix("layer:") {
            return self.active_layer_name() == layer;
        }
        self.state.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.state.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
    }

    fn set_value(&mut self, key: &str, v: f64) -> Effect {
        let v = v.clamp(0.0, 1.0);
        self.state.insert(key.to_string(), Value::from(v));
        self.hold.insert(key.to_string(), Instant::now() + STATE_HOLD);
        Effect::Send(Outgoing::Set { k: key.to_string(), v })
    }

    // ---- fingerprint --------------------------------------------------------

    pub fn set_finger(&mut self, s: FingerState) {
        let now = Instant::now();
        let f = &mut self.finger;
        match s {
            FingerState::Idle => {
                if f.state != FingerState::Idle {
                    f.hidden_at = Some(now);
                }
            }
            FingerState::Retry => {
                f.retry_at = Some(now);
            }
            _ => {}
        }
        if f.state == FingerState::Idle && s != FingerState::Idle {
            // Only fade in from nothing; a re-scan after a failure stays put.
            let fading_out = f.hidden_at.is_some_and(|t| now - t < FP_FADE_OUT);
            if !fading_out {
                f.shown_at = now;
            }
            f.hidden_at = None;
        }
        if s != FingerState::Idle {
            f.shown_state = s;
        }
        if !(s == FingerState::Retry && f.state == FingerState::Scan) {
            f.since = now;
        }
        // A retry is still a scan in progress, just with a nudge.
        f.state = if s == FingerState::Retry { FingerState::Scan } else { s };
    }

    /// 0..1 visibility of the fingerprint prompt.
    pub fn finger_alpha(&self, now: Instant) -> f64 {
        let f = &self.finger;
        if f.state != FingerState::Idle {
            ((now - f.shown_at).as_secs_f64() / FP_FADE_IN.as_secs_f64()).min(1.0)
        } else if let Some(t) = f.hidden_at {
            1.0 - ((now - t).as_secs_f64() / FP_FADE_OUT.as_secs_f64()).min(1.0)
        } else {
            0.0
        }
    }

    // ---- time -------------------------------------------------------------

    /// Expire timed things. Returns true if anything changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let f = &self.finger;
        let limit = match f.state {
            FingerState::Scan => Some(Duration::from_secs(35)),
            FingerState::Match => Some(Duration::from_millis(1100)),
            FingerState::Fail => Some(Duration::from_millis(1600)),
            _ => None,
        };
        if limit.is_some_and(|l| now - f.since > l) {
            self.set_finger(FingerState::Idle);
            changed = true;
        }
        if let Some(s) = &self.slider {
            let idle = Duration::from_secs(self.layout.settings.slider_timeout.max(1));
            let dragging = self
                .grabs
                .values()
                .any(|g| matches!(g, Grab::SliderDrag { .. } | Grab::Track));
            let expired = s.close_at.is_some_and(|t| now >= t) || now - s.last_touch > idle;
            if expired && !dragging {
                self.slider = None;
                changed = true;
            }
        }
        let before = self.pressed.len();
        self.pressed.retain(|_, (down, t)| *down || now - *t < PRESS_FADE);
        changed |= before != self.pressed.len();
        self.hold.retain(|_, t| *t > now);
        changed
    }

    /// True while something on screen is moving and needs frames.
    pub fn animating(&self, now: Instant) -> bool {
        self.finger.state != FingerState::Idle
            || self.finger_alpha(now) > 0.0
            || self.pressed.values().any(|(down, _)| !*down)
    }

    /// When `tick` next has work to do, if ever.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let mut d: Option<Instant> = None;
        let mut take = |t: Instant| d = Some(d.map_or(t, |x| x.min(t)));
        if let Some(s) = &self.slider {
            take(s.close_at.unwrap_or(
                s.last_touch + Duration::from_secs(self.layout.settings.slider_timeout.max(1)),
            ));
        }
        if self.animating(now) {
            take(now + Duration::from_millis(33));
        }
        d
    }

    // ---- geometry ---------------------------------------------------------

    /// Lay out widths left to right: fixed items first, flex items share the rest.
    pub fn lay_out(&self, widths: &[(Option<f64>, f64)]) -> Vec<(f64, f64)> {
        let n = widths.len();
        if n == 0 {
            return vec![];
        }
        let fixed: f64 = widths.iter().filter_map(|(w, _)| *w).sum();
        let weight: f64 = widths.iter().filter(|(w, _)| w.is_none()).map(|(_, f)| *f).sum();
        let spare = self.w - fixed - SPACING * (n - 1) as f64;
        let (scale, per_weight) = if spare < 0.0 {
            ((self.w - SPACING * (n - 1) as f64).max(0.0) / fixed, 0.0)
        } else {
            (1.0, if weight > 0.0 { spare / weight } else { 0.0 })
        };
        let mut x = if weight == 0.0 && spare > 0.0 { spare / 2.0 } else { 0.0 };
        widths
            .iter()
            .map(|(w, f)| {
                let width = w.map(|w| w * scale).unwrap_or(f * per_weight);
                let r = (x.round(), width.round());
                x += width + SPACING;
                r
            })
            .collect()
    }

    pub fn item_rects(&self) -> Vec<(f64, f64)> {
        let widths: Vec<_> = self
            .items()
            .iter()
            .map(|i| (i.w, i.flex.unwrap_or(1.0)))
            .collect();
        self.lay_out(&widths)
    }

    pub fn overlay_parts(&self) -> Vec<(Part, f64, f64)> {
        let mut parts: Vec<(Part, Option<f64>, f64)> = self
            .items()
            .iter()
            .enumerate()
            .filter(|(_, i)| i.pin)
            .map(|(n, i)| (Part::Pinned(n), Some(i.w.unwrap_or(90.0)), 0.0))
            .collect();
        parts.extend([
            (Part::Close, Some(72.0), 0.0),
            (Part::Low, Some(64.0), 0.0),
            (Part::Track, None, 1.0),
            (Part::High, Some(64.0), 0.0),
            (Part::Value, Some(96.0), 0.0),
        ]);
        // Leave room at the right for the fingerprint prompt's arrow target.
        let widths: Vec<_> = parts.iter().map(|(_, w, f)| (*w, *f)).collect();
        self.lay_out(&widths)
            .into_iter()
            .zip(parts)
            .map(|((x, w), (p, _, _))| (p, x, w))
            .collect()
    }

    fn track_rect(&self) -> (f64, f64) {
        self.overlay_parts()
            .into_iter()
            .find(|(p, _, _)| *p == Part::Track)
            .map(|(_, x, w)| (x + 16.0, (w - 32.0).max(1.0)))
            .unwrap_or((0.0, self.w))
    }

    pub fn hit(&self, x: f64) -> Option<Hit> {
        let half = SPACING / 2.0;
        if self.slider.is_some() {
            return self
                .overlay_parts()
                .into_iter()
                .find(|(_, px, pw)| x >= px - half && x < px + pw + half)
                .map(|(p, _, _)| match p {
                    Part::Pinned(n) => Hit::Item(n),
                    p => Hit::Overlay(p),
                });
        }
        let items = self.items();
        let rects = self.item_rects();
        let (n, (rx, rw)) = rects
            .into_iter()
            .enumerate()
            .find(|(_, (rx, rw))| x >= rx - half && x < rx + rw + half)?;
        match items[n].kind {
            Kind::Gap | Kind::Flex => None,
            Kind::Media => {
                let part = (((x - rx) / rw) * 3.0).clamp(0.0, 2.0) as u8;
                Some(Hit::Media(n, part))
            }
            _ => Some(Hit::Item(n)),
        }
    }

    fn hit_rect(&self, hit: Hit) -> Option<(f64, f64)> {
        match hit {
            Hit::Item(n) | Hit::Media(n, _) => {
                if self.slider.is_some() {
                    self.overlay_parts()
                        .into_iter()
                        .find(|(p, _, _)| *p == Part::Pinned(n))
                        .map(|(_, x, w)| (x, w))
                } else {
                    self.item_rects().get(n).copied()
                }
            }
            Hit::Overlay(p) => self
                .overlay_parts()
                .into_iter()
                .find(|(q, _, _)| *q == p)
                .map(|(_, x, w)| (x, w)),
        }
    }

    // ---- touches ----------------------------------------------------------

    fn press(&mut self, hit: Hit, down: bool) {
        self.pressed.insert(hit, (down, Instant::now()));
    }

    pub fn touch_down(&mut self, slot: u32, x: f64) -> Vec<Effect> {
        let mut fx = vec![];
        let now = Instant::now();
        if let Some(s) = &mut self.slider {
            s.last_touch = now;
            s.close_at = None;
        }
        // The fingerprint prompt covers the right end; touches there do nothing.
        if self.finger.state != FingerState::Idle
            && x > self.w - self.layout.settings.fingerprint_width
        {
            self.grabs.insert(slot, Grab::Ignore);
            return fx;
        }
        let Some(hit) = self.hit(x) else {
            self.grabs.insert(slot, Grab::Ignore);
            return fx;
        };
        let grab = match hit {
            Hit::Overlay(Part::Track) => {
                let (tx, tw) = self.track_rect();
                if let Some(key) = self.slider.as_ref().map(|s| s.key.clone()) {
                    fx.push(self.set_value(&key, (x - tx) / tw));
                }
                Grab::Track
            }
            Hit::Overlay(p @ (Part::Low | Part::High)) => {
                self.press(hit, true);
                if let Some(key) = self.slider.as_ref().map(|s| s.key.clone()) {
                    let cur = self.num(&key).unwrap_or(0.0);
                    let step = if p == Part::Low { -SLIDER_STEP } else { SLIDER_STEP };
                    fx.push(self.set_value(&key, cur + step));
                }
                Grab::Press { hit, keys: vec![], act: None, inside: true }
            }
            Hit::Overlay(Part::Close) => {
                self.press(hit, true);
                Grab::Press { hit, keys: vec![], act: None, inside: true }
            }
            Hit::Overlay(_) => Grab::Ignore,
            Hit::Media(_, part) => {
                self.press(hit, true);
                let key = [Key::PreviousSong, Key::PlayPause, Key::NextSong][part as usize];
                fx.push(Effect::KeyDown(vec![key]));
                Grab::Press { hit, keys: vec![key], act: None, inside: true }
            }
            Hit::Item(n) => {
                let item = self.items()[n].clone();
                if item.kind == Kind::Slider && self.slider.is_none() {
                    let key = item.target.clone().unwrap_or_default();
                    let start_v = self.num(&key).unwrap_or(0.0);
                    self.slider = Some(SliderOverlay {
                        item,
                        key,
                        last_touch: now,
                        close_at: None,
                    });
                    Grab::SliderDrag { start_x: x, start_v, moved: false }
                } else {
                    self.press(hit, true);
                    match item.act {
                        Some(Action::Key(keys)) => {
                            fx.push(Effect::KeyDown(keys.clone()));
                            Grab::Press { hit, keys, act: None, inside: true }
                        }
                        act => Grab::Press { hit, keys: vec![], act, inside: true },
                    }
                }
            }
        };
        self.grabs.insert(slot, grab);
        fx
    }

    pub fn touch_motion(&mut self, slot: u32, x: f64) -> Vec<Effect> {
        let mut fx = vec![];
        if let Some(s) = &mut self.slider {
            s.last_touch = Instant::now();
        }
        let Some(mut grab) = self.grabs.remove(&slot) else {
            return fx;
        };
        match &mut grab {
            Grab::Press { hit, keys, inside, .. } => {
                if *inside {
                    let (rx, rw) = self.hit_rect(*hit).unwrap_or((0.0, 0.0));
                    let slop = 24.0;
                    if x < rx - slop || x > rx + rw + slop {
                        *inside = false;
                        self.press(*hit, false);
                        if !keys.is_empty() {
                            fx.push(Effect::KeyUp(keys.clone()));
                        }
                    }
                }
            }
            Grab::SliderDrag { start_x, start_v, moved } => {
                if (x - *start_x).abs() > DRAG_THRESHOLD {
                    *moved = true;
                }
                if *moved {
                    let (_, tw) = self.track_rect();
                    let v = *start_v + (x - *start_x) / tw;
                    if let Some(key) = self.slider.as_ref().map(|s| s.key.clone()) {
                        fx.push(self.set_value(&key, v));
                    }
                }
            }
            Grab::Track => {
                let (tx, tw) = self.track_rect();
                if let Some(key) = self.slider.as_ref().map(|s| s.key.clone()) {
                    fx.push(self.set_value(&key, (x - tx) / tw));
                }
            }
            Grab::Ignore => {}
        }
        self.grabs.insert(slot, grab);
        fx
    }

    pub fn touch_up(&mut self, slot: u32) -> Vec<Effect> {
        let mut fx = vec![];
        let now = Instant::now();
        if let Some(s) = &mut self.slider {
            s.last_touch = now;
        }
        match self.grabs.remove(&slot) {
            Some(Grab::Press { hit, keys, act, inside }) => {
                self.press(hit, false);
                if !keys.is_empty() && inside {
                    fx.push(Effect::KeyUp(keys));
                }
                if inside {
                    if hit == Hit::Overlay(Part::Close) {
                        self.slider = None;
                    }
                    match act {
                        Some(Action::Cmd(cmd)) => fx.push(Effect::Send(Outgoing::Run { cmd })),
                        Some(Action::Layer(l)) => fx.extend(self.switch_layer(&l)),
                        Some(Action::ToggleLayer(l)) => {
                            let target = if self.layer == l { self.layout.default.clone() } else { l };
                            fx.extend(self.switch_layer(&target));
                        }
                        _ => {}
                    }
                }
            }
            Some(Grab::SliderDrag { moved: true, .. }) => {
                // A quick drag from the button: tidy away shortly after.
                if let Some(s) = &mut self.slider {
                    s.close_at = Some(now + Duration::from_millis(1200));
                }
            }
            _ => {}
        }
        fx
    }

    /// Let go of everything, e.g. before the layer under the fingers changes.
    pub fn release_all(&mut self) -> Vec<Effect> {
        let mut fx = vec![];
        for (_, g) in self.grabs.drain() {
            if let Grab::Press { keys, inside: true, .. } = g {
                if !keys.is_empty() {
                    fx.push(Effect::KeyUp(keys));
                }
            }
        }
        let now = Instant::now();
        for v in self.pressed.values_mut() {
            if v.0 {
                *v = (false, now);
            }
        }
        fx
    }
}
