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
/// Spectrum frames older than this count as silence.
const BARS_STALE: Duration = Duration::from_millis(400);
/// Plugin frames older than this are treated as gone.
pub const PIXELS_STALE: Duration = Duration::from_millis(1500);
pub const BAR_STYLES: u8 = 3;

/// Something the daemon has to do in response to a touch.
pub enum Effect {
    KeyDown(Vec<Key>),
    KeyUp(Vec<Key>),
    Send(Outgoing),
}

/// Parts of an expanded overlay (slider or visualiser), in drawing order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Pinned(usize),
    Close,
    // slider
    Low,
    Track,
    High,
    Value,
    // visualiser
    Art,
    // weather
    WNow,
    WHours,
    Viz,
    Preset,
    Mode,
    Prev,
    Play,
    Next,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hit {
    Item(usize),
    /// A segment inside an item: media prev/play/next, or a workspace.
    Sub(usize, u8),
    Overlay(Part),
}

enum Grab {
    /// A button: keys held, or an action that fires on release.
    Press { hit: Hit, keys: Vec<Key>, act: Option<Action>, inside: bool, tap: Option<(String, f64)> },
    /// Dragging straight from a collapsed slider button, macOS style.
    SliderDrag { start_x: f64, start_v: f64, moved: bool },
    /// Dragging along the expanded slider's track.
    Track,
    /// Touching the visualiser: a tap plays/pauses, a drag scrubs.
    Scrub { start_x: f64, moved: bool },
    Ignore,
}

pub struct SliderOverlay {
    pub item: Item,
    pub key: String,
    pub last_touch: Instant,
    pub close_at: Option<Instant>,
}

pub struct VizOverlay {
    /// "bars" or the name of the plugin filling the area.
    pub mode: String,
    /// While scrubbing: the position (seconds) under the finger.
    pub scrub: Option<f64>,
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
    pub viz: Option<VizOverlay>,
    /// The expanded weather view, with when it was last touched.
    pub weather: Option<Instant>,
    pub finger: Finger,
    pub pressed: HashMap<Hit, (bool, Instant)>,
    pub bars: Vec<f32>,
    pub peaks: Vec<f32>,
    pub bars_at: Instant,
    pub bar_style: u8,
    pub art: Option<cairo::ImageSurface>,
    pub surfaces: HashMap<String, (cairo::ImageSurface, Instant)>,
    pub position_at: Instant,
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
            viz: None,
            weather: None,
            finger: Finger {
                state: FingerState::Idle,
                shown_state: FingerState::Scan,
                since: now,
                shown_at: now,
                hidden_at: None,
                retry_at: None,
            },
            pressed: HashMap::new(),
            bars: vec![],
            peaks: vec![],
            bars_at: now - BARS_STALE * 2,
            bar_style: 0,
            art: None,
            surfaces: HashMap::new(),
            position_at: now,
            grabs: HashMap::new(),
            hold: HashMap::new(),
        }
    }

    pub fn set_layout(&mut self, layout: Layout) -> Vec<Effect> {
        let mut fx = self.release_all();
        if !layout.layers.contains_key(&self.layer) {
            self.layer = layout.default.clone();
        }
        self.layout = layout;
        self.slider = None;
        self.weather = None;
        self.weather = None;
        fx.extend(self.close_viz());
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
        let mut fx = self.release_all();
        if self.layout.layers.contains_key(name) {
            self.layer = name.to_string();
        }
        self.slider = None;
        fx.extend(self.close_viz());
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
            if k == "position" {
                self.position_at = now;
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

    /// Track position and length in seconds, running on while playing.
    pub fn position(&self, now: Instant) -> Option<(f64, f64)> {
        let len = self.num("length").filter(|l| *l > 0.0)?;
        let mut pos = self.num("position").unwrap_or(0.0);
        if self.flag("playing") {
            pos += (now - self.position_at).as_secs_f64();
        }
        Some((pos.clamp(0.0, len), len))
    }

    /// Hyprland workspaces to show: (id, window count), plus the active id.
    pub fn workspaces(&self) -> (Vec<(i64, i64)>, i64) {
        let active = self.num("workspace").map(|v| v as i64).unwrap_or(1);
        let mut list: Vec<(i64, i64)> = self
            .state
            .get("workspaces")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let p = p.as_array()?;
                        Some((p.first()?.as_i64()?, p.get(1).and_then(|w| w.as_i64()).unwrap_or(0)))
                    })
                    .filter(|(id, _)| *id > 0)
                    .collect()
            })
            .unwrap_or_default();
        if !list.iter().any(|(id, _)| *id == active) {
            list.push((active, 0));
        }
        list.sort();
        (list, active)
    }

    // ---- spectrum and frames -----------------------------------------------

    pub fn set_bars(&mut self, v: Vec<f32>) {
        let now = Instant::now();
        let dt = (now - self.bars_at).as_secs_f32().min(0.5);
        if self.peaks.len() != v.len() {
            self.peaks = v.clone();
        }
        for (p, b) in self.peaks.iter_mut().zip(&v) {
            // Peaks hang for a moment, then fall.
            *p = (*p - 0.7 * dt).max(*b);
        }
        self.bars = v;
        self.bars_at = now;
    }

    pub fn bars_live(&self, now: Instant) -> bool {
        !self.bars.is_empty() && now - self.bars_at < BARS_STALE
    }

    pub fn surface(&self, id: &str, now: Instant) -> Option<&cairo::ImageSurface> {
        self.surfaces.get(id).filter(|(_, t)| now - *t < PIXELS_STALE).map(|(s, _)| s)
    }

    /// The plugin surfaces currently on screen, with their sizes.
    pub fn surface_specs(&self) -> Vec<SurfaceSpec> {
        let h = self.h as u32;
        if let Some(v) = &self.viz {
            if v.mode == "bars" {
                return vec![];
            }
            let (_, w) = self.viz_area();
            return vec![SurfaceSpec { id: v.mode.clone(), w: w as u32, h, options: None }];
        }
        if self.slider.is_some() {
            return vec![];
        }
        self.items()
            .iter()
            .zip(self.item_rects())
            .filter(|(i, _)| i.kind == Kind::Plugin)
            .filter_map(|(i, (_, w))| {
                Some(SurfaceSpec { id: i.plugin.clone()?, w: w as u32, h, options: i.options.clone() })
            })
            .collect()
    }

    // ---- visualiser overlay ------------------------------------------------

    pub fn viz_area(&self) -> (f64, f64) {
        self.overlay_parts()
            .into_iter()
            .find(|(p, _, _)| *p == Part::Viz)
            .map(|(_, x, w)| (x, w))
            .unwrap_or((0.0, self.w))
    }

    fn visualizers(&self) -> Vec<String> {
        let v = &self.layout.settings.visualizers;
        if v.is_empty() { vec!["bars".into()] } else { v.clone() }
    }

    pub fn open_viz(&mut self) -> Vec<Effect> {
        self.slider = None;
        if self.viz.is_none() {
            let list = self.visualizers();
            // A music video playing? Start on the video plugin.
            let mode = if self.flag("has_video") && list.iter().any(|m| m == "fmvideo") {
                "fmvideo".to_string()
            } else {
                list[0].clone()
            };
            self.viz = Some(VizOverlay { mode, scrub: None });
        }
        // The agent keeps the spectrum running while this is open, even with the eq off.
        vec![Effect::Send(Outgoing::Set { k: "viz".into(), v: 1.0 })]
    }

    pub fn close_viz(&mut self) -> Vec<Effect> {
        if self.viz.take().is_some() {
            vec![Effect::Send(Outgoing::Set { k: "viz".into(), v: 0.0 })]
        } else {
            vec![]
        }
    }

    /// The equaliser behind the title is on unless switched off.
    pub fn eq_on(&self) -> bool {
        self.state.get("eq").and_then(|v| v.as_bool()).unwrap_or(true)
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
        if self.weather.is_some_and(|t| now - t > Duration::from_secs(10)) {
            self.weather = None;
            changed = true;
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
            || (self.bars_live(now) && self.shows_spectrum())
            || (self.viz.is_some() && self.flag("playing"))
            || self.surfaces.values().any(|(_, t)| now - *t < PIXELS_STALE)
    }

    /// Whether anything on screen draws the spectrum right now.
    fn shows_spectrum(&self) -> bool {
        self.viz.is_some()
            || (self.slider.is_none() && self.eq_on() && self.items().iter().any(|i| i.kind == Kind::Nowplaying))
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
        if self.weather.is_some() {
            parts.extend([
                (Part::Close, Some(72.0), 0.0),
                (Part::WNow, Some(560.0), 0.0),
                (Part::WHours, None, 1.0),
            ]);
        } else if self.viz.is_some() {
            parts.extend([
                (Part::Close, Some(72.0), 0.0),
                (Part::Art, Some(54.0), 0.0),
                (Part::Viz, None, 1.0),
                (Part::Preset, Some(72.0), 0.0),
            ]);
            // Only worth a mode button when there's more than one thing to show.
            if self.visualizers().len() > 1 {
                parts.push((Part::Mode, Some(72.0), 0.0));
            }
            parts.extend([
                (Part::Prev, Some(80.0), 0.0),
                (Part::Play, Some(80.0), 0.0),
                (Part::Next, Some(80.0), 0.0),
            ]);
        } else {
            parts.extend([
                (Part::Close, Some(72.0), 0.0),
                (Part::Low, Some(64.0), 0.0),
                (Part::Track, None, 1.0),
                (Part::High, Some(64.0), 0.0),
                (Part::Value, Some(96.0), 0.0),
            ]);
        }
        let widths: Vec<_> = parts.iter().map(|(_, w, f)| (*w, *f)).collect();
        self.lay_out(&widths)
            .into_iter()
            .zip(parts)
            .map(|((x, w), (p, _, _))| (p, x, w))
            .collect()
    }

    fn overlay_open(&self) -> bool {
        self.slider.is_some() || self.viz.is_some() || self.weather.is_some()
    }

    fn track_rect(&self) -> (f64, f64) {
        self.overlay_parts()
            .into_iter()
            .find(|(p, _, _)| *p == Part::Track)
            .map(|(_, x, w)| (x + 16.0, (w - 32.0).max(1.0)))
            .unwrap_or((0.0, self.w))
    }

    /// Sub-segment rects for media (3 equal parts) and workspaces (one per id).
    pub fn sub_rects(&self, item: &Item, x: f64, w: f64) -> Vec<(f64, f64)> {
        let n = match item.kind {
            Kind::Media => 3,
            Kind::Workspaces => self.workspaces().0.len().max(1),
            _ => 1,
        };
        let seg = w / n as f64;
        (0..n).map(|i| (x + seg * i as f64, seg)).collect()
    }

    pub fn hit(&self, x: f64) -> Option<Hit> {
        let half = SPACING / 2.0;
        if self.overlay_open() {
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
            Kind::Media | Kind::Workspaces => {
                let subs = self.sub_rects(&items[n], rx, rw);
                let i = subs.iter().position(|(sx, sw)| x < sx + sw).unwrap_or(subs.len() - 1);
                Some(Hit::Sub(n, i.min(255) as u8))
            }
            _ => Some(Hit::Item(n)),
        }
    }

    fn hit_rect(&self, hit: Hit) -> Option<(f64, f64)> {
        match hit {
            Hit::Item(n) | Hit::Sub(n, _) => {
                if self.overlay_open() {
                    self.overlay_parts()
                        .into_iter()
                        .find(|(p, _, _)| *p == Part::Pinned(n))
                        .map(|(_, x, w)| (x, w))
                } else {
                    let (x, w) = *self.item_rects().get(n)?;
                    match hit {
                        Hit::Sub(_, i) => self.sub_rects(&self.items()[n], x, w).get(i as usize).copied(),
                        _ => Some((x, w)),
                    }
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

    fn key_press(&mut self, hit: Hit, key: Key, fx: &mut Vec<Effect>) -> Grab {
        self.press(hit, true);
        fx.push(Effect::KeyDown(vec![key]));
        Grab::Press { hit, keys: vec![key], act: None, inside: true, tap: None }
    }

    pub fn touch_down(&mut self, slot: u32, x: f64) -> Vec<Effect> {
        let mut fx = vec![];
        let now = Instant::now();
        if let Some(s) = &mut self.slider {
            s.last_touch = now;
            s.close_at = None;
        }
        if let Some(t) = &mut self.weather {
            *t = now;
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
                Grab::Press { hit, keys: vec![], act: None, inside: true, tap: None }
            }
            Hit::Overlay(Part::Viz) => Grab::Scrub { start_x: x, moved: false },
            Hit::Overlay(Part::Prev) => self.key_press(hit, Key::PreviousSong, &mut fx),
            Hit::Overlay(Part::Play) => self.key_press(hit, Key::PlayPause, &mut fx),
            Hit::Overlay(Part::Next) => self.key_press(hit, Key::NextSong, &mut fx),
            Hit::Overlay(Part::Close | Part::Preset | Part::Mode) => {
                self.press(hit, true);
                Grab::Press { hit, keys: vec![], act: None, inside: true, tap: None }
            }
            Hit::Overlay(_) => Grab::Ignore,
            Hit::Sub(n, part) => {
                let item = self.items()[n].clone();
                if item.kind == Kind::Media {
                    let key = [Key::PreviousSong, Key::PlayPause, Key::NextSong][part.min(2) as usize];
                    self.key_press(hit, key, &mut fx)
                } else {
                    // A workspace: fill `{id}` into the item's command.
                    self.press(hit, true);
                    let (list, _) = self.workspaces();
                    let id = list.get(part as usize).map(|(id, _)| *id).unwrap_or(1);
                    let act = match item.act {
                        Some(Action::Cmd(c)) => Some(Action::Cmd(c.replace("{id}", &id.to_string()))),
                        other => other,
                    };
                    Grab::Press { hit, keys: vec![], act, inside: true, tap: None }
                }
            }
            Hit::Item(n) => {
                let item = self.items()[n].clone();
                if item.kind == Kind::Slider && !self.overlay_open() {
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
                    // A plugin with no action of its own gets the tap, in its own coordinates.
                    let tap = match (&item.kind, &item.plugin, &item.act) {
                        (Kind::Plugin, Some(id), None) => {
                            let rx = self.item_rects().get(n).map(|r| r.0).unwrap_or(0.0);
                            Some((id.clone(), x - rx))
                        }
                        _ => None,
                    };
                    match item.act {
                        Some(Action::Key(keys)) => {
                            fx.push(Effect::KeyDown(keys.clone()));
                            Grab::Press { hit, keys, act: None, inside: true, tap }
                        }
                        act => Grab::Press { hit, keys: vec![], act, inside: true, tap },
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
            Grab::Scrub { start_x, moved } => {
                if (x - *start_x).abs() > DRAG_THRESHOLD * 2.0 {
                    *moved = true;
                }
                if *moved {
                    let (vx, vw) = self.viz_area();
                    let len = self.position(Instant::now()).map(|(_, l)| l);
                    if let (Some(len), Some(v)) = (len, self.viz.as_mut()) {
                        v.scrub = Some(((x - vx) / vw).clamp(0.0, 1.0) * len);
                    }
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
            Some(Grab::Press { hit, keys, act, inside, tap }) => {
                self.press(hit, false);
                if !keys.is_empty() && inside {
                    fx.push(Effect::KeyUp(keys));
                }
                if inside {
                    match hit {
                        Hit::Overlay(Part::Close) => {
                            self.slider = None;
                            self.weather = None;
                            fx.extend(self.close_viz());
                        }
                        Hit::Overlay(Part::Preset) => match self.viz.as_ref().map(|v| v.mode.clone()) {
                            Some(m) if m != "bars" => {
                                fx.push(Effect::Send(Outgoing::PluginCmd { id: m, cmd: "next".into(), x: 0.0 }))
                            }
                            _ => self.bar_style = (self.bar_style + 1) % BAR_STYLES,
                        },
                        Hit::Overlay(Part::Mode) => {
                            let list = self.visualizers();
                            if let Some(v) = &mut self.viz {
                                let i = list.iter().position(|m| *m == v.mode).map_or(0, |i| (i + 1) % list.len());
                                v.mode = list[i].clone();
                            }
                        }
                        _ => {}
                    }
                    match act {
                        Some(Action::Cmd(cmd)) => fx.push(Effect::Send(Outgoing::Run { cmd })),
                        Some(Action::Layer(l)) => fx.extend(self.switch_layer(&l)),
                        Some(Action::ToggleLayer(l)) => {
                            let target = if self.layer == l { self.layout.default.clone() } else { l };
                            fx.extend(self.switch_layer(&target));
                        }
                        Some(Action::Visualizer) => fx.extend(self.open_viz()),
                        Some(Action::Weather) => {
                            self.slider = None;
                            fx.extend(self.close_viz());
                            self.weather = Some(now);
                        }
                        Some(Action::ToggleFlag(k)) => {
                            let on = !self.flag(&k);
                            self.state.insert(k.clone(), Value::from(on));
                            self.hold.insert(k.clone(), now + STATE_HOLD);
                            fx.push(Effect::Send(Outgoing::Set { k, v: on as u8 as f64 }));
                        }
                        _ => {}
                    }
                    if let Some((id, x)) = tap {
                        fx.push(Effect::Send(Outgoing::PluginCmd { id, cmd: "tap".into(), x }));
                    }
                }
            }
            Some(Grab::SliderDrag { moved: true, .. }) => {
                // A quick drag from the button: tidy away shortly after.
                if let Some(s) = &mut self.slider {
                    s.close_at = Some(now + Duration::from_millis(1200));
                }
            }
            Some(Grab::Scrub { moved, .. }) => {
                let target = self.viz.as_mut().and_then(|v| v.scrub.take());
                match (moved, target) {
                    (true, Some(pos)) => {
                        self.state.insert("position".into(), Value::from(pos));
                        self.position_at = now;
                        self.hold.insert("position".into(), now + Duration::from_millis(1500));
                        fx.push(Effect::Send(Outgoing::Seek { pos }));
                    }
                    (false, _) => {
                        // A tap on the visualiser plays or pauses.
                        fx.push(Effect::KeyDown(vec![Key::PlayPause]));
                        fx.push(Effect::KeyUp(vec![Key::PlayPause]));
                    }
                    _ => {}
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
