//! Drawing. Everything is drawn in landscape (2008 x 60); the DRM backend
//! rotates it onto the portrait panel.

use crate::proto::*;
use crate::ui::*;
use cairo::{Context, LinearGradient};
use chrono::Local;
use std::f64::consts::PI;
use std::fs;
use std::time::Instant;

const MARGIN_Y: f64 = 3.0;
const RADIUS: f64 = 9.0;
const ICON_PX: f64 = 40.0;
const LABEL_PX: f64 = 26.0;

#[derive(Clone, Copy)]
pub struct Rgb(pub f64, pub f64, pub f64);

impl Rgb {
    pub fn parse(s: &str) -> Rgb {
        let s = s.trim().trim_start_matches('#');
        let v = u32::from_str_radix(s.get(..6).unwrap_or("000000"), 16).unwrap_or(0);
        Rgb(
            ((v >> 16) & 0xff) as f64 / 255.0,
            ((v >> 8) & 0xff) as f64 / 255.0,
            (v & 0xff) as f64 / 255.0,
        )
    }
    pub fn mix(self, o: Rgb, t: f64) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        Rgb(
            self.0 + (o.0 - self.0) * t,
            self.1 + (o.1 - self.1) * t,
            self.2 + (o.2 - self.2) * t,
        )
    }
    fn set(self, c: &Context) {
        c.set_source_rgb(self.0, self.1, self.2);
    }
    fn set_a(self, c: &Context, a: f64) {
        c.set_source_rgba(self.0, self.1, self.2, a);
    }
}

pub struct Palette {
    pub bg: Rgb,
    pub surface: Rgb,
    pub surface_hi: Rgb,
    pub fg: Rgb,
    pub fg_dim: Rgb,
    pub accent: Rgb,
    pub red: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
}

impl Palette {
    pub fn from(t: &Theme) -> Palette {
        Palette {
            bg: Rgb::parse(&t.bg),
            surface: Rgb::parse(&t.surface),
            surface_hi: Rgb::parse(&t.surface_hi),
            fg: Rgb::parse(&t.fg),
            fg_dim: Rgb::parse(&t.fg_dim),
            accent: Rgb::parse(&t.accent),
            red: Rgb::parse(&t.red),
            green: Rgb::parse(&t.green),
            yellow: Rgb::parse(&t.yellow),
        }
    }

    /// (fill, ink) for a button style. `None` fill means no pill.
    fn style(&self, s: Style) -> (Option<Rgb>, Rgb) {
        match s {
            Style::Normal => (Some(self.surface), self.fg),
            Style::Accent => (Some(self.accent), self.bg),
            Style::Danger => (Some(self.red.mix(self.surface, 0.25)), self.bg),
            Style::Subtle => (Some(self.surface.mix(self.bg, 0.55)), self.fg),
            Style::Plain => (None, self.fg),
        }
    }
}

fn rounded(c: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    c.new_sub_path();
    c.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    c.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    c.arc(x + r, y + h - r, r, PI / 2.0, PI);
    c.arc(x + r, y + r, r, PI, 1.5 * PI);
    c.close_path();
}

pub struct Painter<'a> {
    pub c: &'a Context,
    pub font: String,
    pub h: f64,
}

impl Painter<'_> {
    fn layout(&self, text: &str, px: f64, bold: bool) -> pango::Layout {
        let l = pangocairo::functions::create_layout(self.c);
        let mut fd = pango::FontDescription::from_string(&self.font);
        fd.set_absolute_size(px * pango::SCALE as f64);
        if bold {
            fd.set_weight(pango::Weight::Bold);
        }
        l.set_font_description(Some(&fd));
        l.set_text(text);
        l
    }

    fn text_width(&self, text: &str, px: f64, bold: bool) -> f64 {
        self.layout(text, px, bold).pixel_size().0 as f64
    }

    /// Draw text centred on (cx, cy), clipped to `max_w` with an ellipsis.
    fn text(&self, text: &str, px: f64, bold: bool, cx: f64, cy: f64, max_w: Option<f64>) {
        let l = self.layout(text, px, bold);
        if let Some(mw) = max_w {
            l.set_width((mw * pango::SCALE as f64) as i32);
            l.set_ellipsize(pango::EllipsizeMode::End);
        }
        let (_, logical) = l.pixel_extents();
        let w = logical.width() as f64;
        let h = logical.height() as f64;
        self.c.move_to((cx - w / 2.0).round(), (cy - h / 2.0).round());
        pangocairo::functions::show_layout(self.c, &l);
    }

    /// Icon and optional label side by side, centred in the rect.
    fn content(&self, icon: Option<&str>, label: Option<&str>, x: f64, w: f64, ink: Rgb) {
        let cy = self.h / 2.0;
        ink.set(self.c);
        match (icon, label) {
            (Some(i), Some(t)) => {
                let iw = self.text_width(i, ICON_PX, false);
                let tw = self.text_width(t, LABEL_PX, true).min(w - iw - 24.0);
                let gap = 8.0;
                let start = x + (w - iw - gap - tw) / 2.0;
                self.text(i, ICON_PX, false, start + iw / 2.0, cy, None);
                self.text(t, LABEL_PX, true, start + iw + gap + tw / 2.0, cy, Some(tw + 1.0));
            }
            (Some(i), None) => self.text(i, ICON_PX, false, x + w / 2.0, cy, None),
            (None, Some(t)) => self.text(t, LABEL_PX, true, x + w / 2.0, cy, Some(w - 12.0)),
            (None, None) => {}
        }
    }

    fn pill(&self, x: f64, w: f64, fill: Rgb) {
        fill.set(self.c);
        rounded(self.c, x, MARGIN_Y, w, self.h - 2.0 * MARGIN_Y, RADIUS);
        self.c.fill().unwrap();
    }
}

fn press_level(m: &Model, hit: Hit, now: Instant) -> f64 {
    match m.pressed.get(&hit) {
        Some((true, _)) => 1.0,
        Some((false, t)) => 1.0 - ((now - *t).as_secs_f64() / PRESS_FADE.as_secs_f64()).min(1.0),
        None => 0.0,
    }
}

fn level_icon<'a>(icons: &'a [String], v: f64) -> Option<&'a str> {
    if icons.is_empty() {
        return None;
    }
    let i = ((v.clamp(0.0, 1.0) * icons.len() as f64) as usize).min(icons.len() - 1);
    Some(icons[i].as_str())
}

fn battery() -> Option<(f64, bool)> {
    for e in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = e.path();
        if fs::read_to_string(p.join("type")).ok()?.trim() != "Battery" {
            continue;
        }
        let cap = fs::read_to_string(p.join("capacity")).ok()?.trim().parse::<f64>().ok()?;
        let status = fs::read_to_string(p.join("status")).unwrap_or_default();
        let charging = matches!(status.trim(), "Charging" | "Full");
        return Some((cap, charging));
    }
    None
}

pub fn draw(m: &Model, c: &Context, now: Instant) {
    let pal = Palette::from(&m.theme);
    let p = Painter { c, font: m.theme.font.clone(), h: m.h };
    pal.bg.set(c);
    c.paint().unwrap();

    if let Some(s) = &m.slider {
        draw_slider(m, &p, &pal, s, now);
    } else {
        let items = m.items();
        for (n, (x, w)) in m.item_rects().into_iter().enumerate() {
            draw_item(m, &p, &pal, &items[n], n, x, w, now);
        }
    }

    let a = m.finger_alpha(now);
    if a > 0.0 {
        draw_finger(m, &p, &pal, a, now);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_item(m: &Model, p: &Painter, pal: &Palette, it: &Item, n: usize, x: f64, w: f64, now: Instant) {
    let c = p.c;
    let on = it.toggle.as_deref().is_some_and(|k| m.flag(k));
    let style = if on { it.style_on.unwrap_or(it.style) } else { it.style };
    let (fill, ink) = pal.style(style);
    let pressed = press_level(m, Hit::Item(n), now);
    let with_press = |f: Rgb| f.mix(pal.accent, 0.45 * pressed);

    match it.kind {
        Kind::Gap | Kind::Flex => {}
        Kind::Button => {
            if let Some(f) = fill {
                p.pill(x, w, with_press(f));
            } else if pressed > 0.0 {
                pal.accent.set_a(c, 0.35 * pressed);
                rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                c.fill().unwrap();
            }
            let icon = if on { it.icon_on.as_deref().or(it.icon.as_deref()) } else { it.icon.as_deref() };
            let label = if on { it.label_on.as_deref().or(it.label.as_deref()) } else { it.label.as_deref() };
            p.content(icon, label, x, w, ink);
        }
        Kind::Slider => {
            let key = it.target.as_deref().unwrap_or("");
            let v = m.num(key).unwrap_or(0.0);
            let muted = it.mute_key.as_deref().is_some_and(|k| m.flag(k));
            p.pill(x, w, with_press(fill.unwrap_or(pal.surface)));
            let icon = if muted { it.mute_icon.as_deref().or(level_icon(&it.icons, 0.0)) } else { level_icon(&it.icons, v) }
                .or(it.icon.as_deref());
            // Lift the icon clear of the level line along the bottom.
            c.save().unwrap();
            c.translate(0.0, -4.0);
            p.content(icon, None, x, w, if muted { pal.fg_dim } else { ink });
            c.restore().unwrap();
            // A hairline level meter along the bottom of the pill.
            let (bx, bw, by) = (x + 16.0, w - 32.0, m.h - MARGIN_Y - 6.0);
            pal.surface_hi.set(c);
            rounded(c, bx, by, bw, 3.0, 1.5);
            c.fill().unwrap();
            if v > 0.0 {
                (if muted { pal.fg_dim } else { pal.accent }).set(c);
                rounded(c, bx, by, (bw * v.min(1.0)).max(3.0), 3.0, 1.5);
                c.fill().unwrap();
            }
        }
        Kind::Media => {
            p.pill(x, w, fill.unwrap_or(pal.surface));
            let seg = w / 3.0;
            let playing = m.flag("playing");
            let icons = ["󰒮", if playing { "󰏤" } else { "󰐊" }, "󰒭"];
            for (i, icon) in icons.iter().enumerate() {
                let sx = x + seg * i as f64;
                let pl = press_level(m, Hit::Media(n, i as u8), now);
                if pl > 0.0 {
                    c.save().unwrap();
                    rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                    c.clip();
                    pal.accent.set_a(c, 0.45 * pl);
                    c.rectangle(sx, MARGIN_Y, seg, m.h - 2.0 * MARGIN_Y);
                    c.fill().unwrap();
                    c.restore().unwrap();
                }
                p.content(Some(icon), None, sx, seg, if i == 1 && playing { pal.accent } else { ink });
            }
            // Hairline dividers between the three segments.
            pal.bg.set_a(c, 0.6);
            for i in 1..3 {
                c.rectangle((x + seg * i as f64).round() - 0.5, MARGIN_Y + 12.0, 1.0, m.h - 2.0 * MARGIN_Y - 24.0);
            }
            c.fill().unwrap();
        }
        Kind::Nowplaying => {
            let Some(title) = m.text("title") else { return };
            if pressed > 0.0 {
                pal.accent.set_a(c, 0.25 * pressed);
                rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                c.fill().unwrap();
            }
            let artist = m.text("artist");
            let cy = m.h / 2.0;
            let note = if m.flag("playing") { "󰝚" } else { "󰏤" };
            let text = match artist {
                Some(a) => format!("{title}  ·  {a}"),
                None => title.to_string(),
            };
            let nw = p.text_width(note, 30.0, false);
            let tw = p.text_width(&text, LABEL_PX, false).min(w - nw - 30.0);
            let start = x + (w - nw - 10.0 - tw) / 2.0;
            pal.accent.set(c);
            p.text(note, 30.0, false, start + nw / 2.0, cy, None);
            pal.fg.set(c);
            p.text(&text, LABEL_PX, false, start + nw + 10.0 + tw / 2.0, cy, Some(tw + 1.0));
        }
        Kind::Clock => {
            if let Some(f) = fill {
                p.pill(x, w, with_press(f));
            }
            let fmt = it.format.as_deref().unwrap_or("%a %-d %b  %H:%M");
            let s = Local::now().format(fmt).to_string();
            ink.set(c);
            p.text(&s, LABEL_PX, true, x + w / 2.0, m.h / 2.0, Some(w - 12.0));
        }
        Kind::Battery => {
            if let Some(f) = fill {
                p.pill(x, w, with_press(f));
            }
            let Some((cap, charging)) = battery() else { return };
            let glyphs = ["󰂎", "󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"];
            let icon = if charging { "󰂄" } else { glyphs[((cap / 10.0).round() as usize).min(10)] };
            let color = if charging {
                pal.green
            } else if cap <= 15.0 {
                pal.red
            } else {
                ink
            };
            p.content(Some(icon), Some(&format!("{cap:.0}%")), x, w, color);
        }
    }
}

fn draw_slider(m: &Model, p: &Painter, pal: &Palette, s: &SliderOverlay, now: Instant) {
    let c = p.c;
    let items = m.items();
    let v = m.num(&s.key).unwrap_or(0.0).clamp(0.0, 1.0);
    let muted = s.item.mute_key.as_deref().is_some_and(|k| m.flag(k));
    for (part, x, w) in m.overlay_parts() {
        let pl = press_level(m, Hit::Overlay(part), now);
        match part {
            Part::Pinned(n) => draw_item(m, p, pal, &items[n], n, x, w, now),
            Part::Close => {
                p.pill(x, w, pal.surface.mix(pal.accent, 0.45 * pl));
                p.content(Some("󰅖"), None, x, w, pal.fg);
            }
            Part::Low | Part::High => {
                if pl > 0.0 {
                    pal.accent.set_a(c, 0.35 * pl);
                    rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                    c.fill().unwrap();
                }
                let icons = &s.item.icons;
                let icon = if part == Part::Low {
                    if s.item.mute_key.is_some() { s.item.mute_icon.as_deref() } else { None }
                        .or(icons.first().map(|s| s.as_str()))
                } else {
                    icons.last().map(|s| s.as_str())
                };
                p.content(icon.or(s.item.icon.as_deref()), None, x, w, pal.fg_dim.mix(pal.fg, 0.5));
            }
            Part::Track => {
                let (tx, tw) = (x + 16.0, w - 32.0);
                let cy = m.h / 2.0;
                let th = 8.0;
                pal.surface.set(c);
                rounded(c, tx, cy - th / 2.0, tw, th, th / 2.0);
                c.fill().unwrap();
                let fill_w = (tw * v).max(th);
                let fill = if muted { pal.fg_dim } else { pal.accent };
                let g = LinearGradient::new(tx, 0.0, tx + fill_w, 0.0);
                let f0 = fill.mix(pal.bg, 0.35);
                g.add_color_stop_rgb(0.0, f0.0, f0.1, f0.2);
                g.add_color_stop_rgb(1.0, fill.0, fill.1, fill.2);
                c.set_source(&g).unwrap();
                rounded(c, tx, cy - th / 2.0, fill_w, th, th / 2.0);
                c.fill().unwrap();
                // Knob with a soft halo in the accent colour.
                let kx = tx + tw * v;
                fill.set_a(c, 0.22);
                c.arc(kx, cy, 20.0, 0.0, 2.0 * PI);
                c.fill().unwrap();
                pal.bg.set(c);
                c.arc(kx, cy, 14.0, 0.0, 2.0 * PI);
                c.fill().unwrap();
                pal.fg.set(c);
                c.arc(kx, cy, 12.0, 0.0, 2.0 * PI);
                c.fill().unwrap();
            }
            Part::Value => {
                let label = if muted { "muted".to_string() } else { format!("{:.0}%", v * 100.0) };
                (if muted { pal.fg_dim } else { pal.fg }).set(c);
                p.text(&label, 28.0, true, x + w / 2.0, m.h / 2.0, None);
            }
        }
    }
}

/// The Touch ID prompt: a pulsing fingerprint with chevrons marching right,
/// towards the sensor in the power key just past the end of the bar.
fn draw_finger(m: &Model, p: &Painter, pal: &Palette, alpha: f64, now: Instant) {
    let c = p.c;
    let f = &m.finger;
    let fw = m.layout.settings.fingerprint_width;
    let x0 = m.w - fw;
    let t = (now - f.shown_at).as_secs_f64();
    let state = if f.state == FingerState::Idle { f.shown_state } else { f.state };

    if m.layout.settings.dim_while_scanning {
        pal.bg.set_a(c, 0.6 * alpha);
        c.rectangle(0.0, 0.0, x0, m.h);
        c.fill().unwrap();
    }

    let (color, title, icon) = match state {
        FingerState::Match => (pal.green, "Unlocked", "󰄬"),
        FingerState::Fail => (pal.red, "Not recognised", "󰈷"),
        _ => {
            let retrying = f.retry_at.is_some_and(|r| (now - r).as_secs_f64() < 1.6);
            if retrying {
                (pal.yellow, "Try again", "󰈷")
            } else {
                (pal.accent, "Touch ID", "󰈷")
            }
        }
    };

    // Panel: a pill of its own, tinted with the state colour.
    c.save().unwrap();
    c.push_group();
    pal.bg.set(c);
    c.rectangle(x0 - SPACING, 0.0, fw + SPACING, m.h);
    c.fill().unwrap();
    let px = x0 + 4.0;
    let pw = m.w - px;
    let g = LinearGradient::new(px, 0.0, m.w, 0.0);
    let (a, b) = (pal.surface.mix(color, 0.08), pal.surface.mix(color, 0.30));
    g.add_color_stop_rgb(0.0, a.0, a.1, a.2);
    g.add_color_stop_rgb(1.0, b.0, b.1, b.2);
    c.set_source(&g).unwrap();
    rounded(c, px, MARGIN_Y, pw, m.h - 2.0 * MARGIN_Y, RADIUS);
    c.fill_preserve().unwrap();
    color.set_a(c, 0.55);
    c.set_line_width(1.5);
    c.stroke().unwrap();

    // Shake sideways when the enclave rejects a scan.
    let shake = f
        .retry_at
        .map(|r| (now - r).as_secs_f64())
        .filter(|d| *d < 0.45)
        .map(|d| (d * 40.0).sin() * 7.0 * (1.0 - d / 0.45))
        .unwrap_or(0.0);

    let cy = m.h / 2.0;
    let chevron_zone = 96.0;
    let icon_cx = m.w - chevron_zone - 40.0 + shake;

    // Pulse: breathing rings behind the glyph while waiting.
    if state == FingerState::Scan {
        for k in 0..2 {
            let phase = ((t * 0.9) + k as f64 * 0.5).fract();
            color.set_a(c, 0.35 * (1.0 - phase));
            c.set_line_width(2.0);
            c.arc(icon_cx, cy, 20.0 + phase * 12.0, 0.0, 2.0 * PI);
            c.stroke().unwrap();
        }
    }
    let breathe = if state == FingerState::Scan { 0.75 + 0.25 * (t * 3.2).sin().abs() } else { 1.0 };
    color.mix(pal.fg, 0.1).set_a(c, breathe);
    p.text(icon, 46.0, false, icon_cx, cy, None);

    pal.fg.set(c);
    let tw = p.text_width(title, LABEL_PX, true);
    p.text(title, LABEL_PX, true, icon_cx - 48.0 - tw / 2.0 + shake, cy, None);

    // Chevrons: a wave of brightness running towards the sensor.
    if matches!(state, FingerState::Scan) {
        let base = m.w - chevron_zone + 6.0;
        for i in 0..3 {
            let phase = (t * 1.6 - i as f64 * 0.22).rem_euclid(1.0);
            let glow = (1.0 - (phase - 0.3).abs() * 2.2).clamp(0.15, 1.0);
            let nudge = (t * 2.0 * PI * 0.8).sin() * 2.0;
            color.set_a(c, glow);
            chevron(c, base + i as f64 * 28.0 + nudge, cy, 11.0, 16.0);
        }
    } else {
        // Settled: one solid arrow so the eye still lands on the sensor.
        color.set_a(c, 0.8);
        chevron(c, m.w - chevron_zone + 32.0, cy, 11.0, 16.0);
    }

    c.pop_group_to_source().unwrap();
    c.paint_with_alpha(alpha).unwrap();
    c.restore().unwrap();
}

fn chevron(c: &Context, x: f64, cy: f64, w: f64, h: f64) {
    c.set_line_width(3.5);
    c.set_line_cap(cairo::LineCap::Round);
    c.set_line_join(cairo::LineJoin::Round);
    c.move_to(x, cy - h);
    c.line_to(x + w, cy);
    c.line_to(x, cy + h);
    c.stroke().unwrap();
}
