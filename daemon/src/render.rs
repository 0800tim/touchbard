//! Drawing. Everything is drawn in landscape (2008 x 60); the DRM backend
//! rotates it onto the portrait panel.

use crate::proto::*;
use crate::ui::*;
use cairo::{Context, LinearGradient};
use chrono::Local;
use std::f64::consts::PI;
use std::fs;
use std::time::Instant;

const MARGIN_Y: f64 = 2.0;
const RADIUS: f64 = 9.0;
const ICON_PX: f64 = 50.0;
const LABEL_PX: f64 = 32.0;

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
    pub accent2: Rgb,
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
            accent2: Rgb::parse(&t.accent2),
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

    if m.weather.is_some() {
        draw_weather(m, &p, &pal, now);
    } else if let Some(v) = &m.viz {
        draw_viz(m, &p, &pal, v, now);
    } else if let Some(s) = &m.slider {
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
                let pl = press_level(m, Hit::Sub(n, i as u8), now);
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
            let bg_mode = m.layout.settings.nowplaying.as_str();
            // The live spectrum runs behind the title, quietly.
            if bg_mode == "bars" && m.eq_on() && m.bars_live(now) {
                c.save().unwrap();
                rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                c.clip();
                draw_bars(m, c, pal, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, 0, 0.38);
                c.restore().unwrap();
            }
            if let Some((pos, len)) = m.position(now) {
                pal.surface_hi.set_a(c, 0.8);
                c.rectangle(x + 12.0, m.h - MARGIN_Y - 2.0, w - 24.0, 2.0);
                c.fill().unwrap();
                pal.accent.set(c);
                c.rectangle(x + 12.0, m.h - MARGIN_Y - 2.0, (w - 24.0) * pos / len, 2.0);
                c.fill().unwrap();
            }
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
            // With artwork on, the cover takes the note's place beside the title.
            let art = if bg_mode == "art" { m.art.as_ref() } else { None };
            let side = m.h - 2.0 * MARGIN_Y - 6.0;
            let nw = if art.is_some() { side } else { p.text_width(note, 30.0, false) };
            let full = p.text_width(&text, LABEL_PX, false);
            let avail = (w - nw - 30.0).max(20.0);
            let scrolling = full > avail;
            m.marquee.set(scrolling && m.flag("playing"));
            let tw = full.min(avail);
            let start = x + (w - nw - 10.0 - tw) / 2.0;
            if let Some(img) = art {
                c.save().unwrap();
                rounded(c, start, MARGIN_Y + 3.0, side, side, 6.0);
                c.clip();
                let s = side / img.width().min(img.height()) as f64;
                c.translate(start, MARGIN_Y + 3.0);
                c.scale(s, s);
                c.set_source_surface(img, 0.0, 0.0).unwrap();
                c.paint().unwrap();
                c.restore().unwrap();
            } else {
                pal.accent.set(c);
                p.text(note, 30.0, false, start + nw / 2.0, cy, None);
            }
            let tx = start + nw + 10.0;
            if scrolling {
                draw_marquee(m, p, pal, &text, full, tx, avail, now);
            } else {
                pal.fg.set(c);
                p.text(&text, LABEL_PX, false, tx + tw / 2.0, cy, Some(tw + 1.0));
            }
        }
        Kind::Workspaces => {
            let (list, active) = m.workspaces();
            if let Some(f) = fill {
                p.pill(x, w, f);
            }
            for (i, (sx, sw)) in m.sub_rects(it, x, w).into_iter().enumerate() {
                let Some(&(id, windows)) = list.get(i) else { break };
                let pl = press_level(m, Hit::Sub(n, i as u8), now);
                let is_active = id == active;
                let (dw, dh) = ((sw - 10.0).min(52.0), m.h - 2.0 * MARGIN_Y - 12.0);
                let dx = sx + (sw - dw) / 2.0;
                let dot = if is_active { pal.accent } else { pal.surface_hi.mix(pal.accent, 0.5 * pl) };
                if is_active || windows > 0 || pl > 0.0 {
                    dot.set_a(c, if is_active { 1.0 } else { 0.55 });
                    rounded(c, dx, MARGIN_Y + 6.0, dw, dh, 7.0);
                    c.fill().unwrap();
                }
                (if is_active { pal.bg } else if windows > 0 { pal.fg } else { pal.fg_dim }).set(c);
                p.text(&id.to_string(), LABEL_PX, true, sx + sw / 2.0, m.h / 2.0, None);
            }
        }
        Kind::Weather => {
            if let Some(f) = fill {
                p.pill(x, w, with_press(f));
            }
            let (icon, temp) = match (m.text("weather_icon"), m.num("weather_temp")) {
                (Some(i), Some(t)) => (i.to_string(), format!("{t:.0}°")),
                _ => ("󰼯".to_string(), "--°".to_string()),
            };
            let tint = weather_tint(pal, m.text("weather_kind").unwrap_or(""));
            let stale = m.flag("weather_stale");
            // Wind: an arrow pointing where it blows, then the speed in the chosen unit.
            let wind = m.num("weather_wind").map(|v| {
                let unit = m.text("weather_wind_unit").unwrap_or("km/h");
                (format!("{v:.0} {unit}"), m.num("weather_wind_dir"))
            });
            let iw = p.text_width(&icon, ICON_PX, false);
            let tw = p.text_width(&temp, LABEL_PX, true);
            let gap = 8.0;
            let arrow = 26.0;
            let ww = wind.as_ref().map_or(0.0, |(t, _)| 14.0 + arrow + 4.0 + p.text_width(t, 22.0, true));
            let mut cx = x + (w - iw - gap - tw - ww) / 2.0;
            let cy = m.h / 2.0;
            (if stale { tint.mix(pal.fg_dim, 0.6) } else { tint }).set(c);
            p.text(&icon, ICON_PX, false, cx + iw / 2.0, cy, None);
            cx += iw + gap;
            (if stale { pal.fg_dim } else { ink }).set(c);
            p.text(&temp, LABEL_PX, true, cx + tw / 2.0, cy, None);
            cx += tw + 14.0;
            if let Some((speed, dir)) = wind {
                let ink2 = if stale { pal.fg_dim } else { pal.fg.mix(pal.fg_dim, 0.25) };
                if let Some(from) = dir {
                    wind_arrow(c, cx + arrow / 2.0, cy, from, 1.25, ink2);
                }
                ink2.set(c);
                let sw = p.text_width(&speed, 22.0, true);
                p.text(&speed, 22.0, true, cx + arrow + 4.0 + sw / 2.0, cy, None);
            }
        }
        Kind::Plugin => {
            let id = it.plugin.as_deref().unwrap_or("");
            draw_surface(m, p, pal, id, x, w, now);
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
            _ => {}
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
        FingerState::Error => (pal.red, "Use password", "\u{F033E}"),
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
    // Its own size: the prompt's width is fixed, the button labels aren't.
    let tw = p.text_width(title, 26.0, true);
    p.text(title, 26.0, true, icon_cx - 48.0 - tw / 2.0 + shake, cy, None);

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

/// The spectrum. Styles: 0 mirrored about the middle, 1 rising from the
/// floor with falling peak caps, 2 dots.
#[allow(clippy::too_many_arguments)]
pub fn draw_bars(m: &Model, c: &Context, pal: &Palette, x: f64, y: f64, w: f64, h: f64, style: u8, alpha: f64) {
    let src = &m.bars;
    if src.is_empty() {
        return;
    }
    let t = (Instant::now() - m.epoch).as_secs_f64();
    match style {
        3 => return draw_ripple(m, c, x, y, w, h, t, alpha),
        4 => return draw_aurora(m, c, x, y, w, h, t, alpha),
        5 => return draw_pixels(m, c, x, y, w, h, t, alpha),
        6 => return draw_smoke_bars(m, c, x, y, w, h, t, alpha),
        _ => {}
    }
    // Fewer, fatter bars on narrow areas.
    let n = ((w / 14.0) as usize).clamp(8, src.len());
    let bw = w / n as f64;
    let sample = |v: &[f32], i: usize| -> f64 {
        let a = i * v.len() / n;
        let b = (((i + 1) * v.len()) / n).max(a + 1).min(v.len());
        v[a..b].iter().copied().fold(0.0f32, f32::max) as f64
    };
    for i in 0..n {
        let v = sample(src, i).clamp(0.0, 1.0);
        let t = i as f64 / (n - 1).max(1) as f64;
        let col = pal.accent.mix(pal.accent2, t);
        let bx = x + i as f64 * bw + bw * 0.18;
        let bwid = bw * 0.64;
        match style {
            0 => {
                let bh = (v * h).max(2.0);
                col.set_a(c, alpha);
                rounded(c, bx, y + (h - bh) / 2.0, bwid, bh, bwid / 2.0);
                c.fill().unwrap();
            }
            1 => {
                let bh = (v * (h - 4.0)).max(2.0);
                col.set_a(c, alpha);
                rounded(c, bx, y + h - bh, bwid, bh, 2.0);
                c.fill().unwrap();
                let pk = sample(&m.peaks, i).clamp(0.0, 1.0);
                pal.fg.set_a(c, alpha * 0.9);
                c.rectangle(bx, y + h - pk * (h - 4.0) - 3.0, bwid, 2.5);
                c.fill().unwrap();
            }
            _ => {
                let rows = 8;
                let lit = (v * rows as f64).round() as usize;
                let cell = h / rows as f64;
                for r in 0..rows {
                    let on = r < lit;
                    col.mix(pal.bg, if on { 0.0 } else { 0.8 }).set_a(c, alpha * if on { 1.0 } else { 0.35 });
                    c.arc(bx + bwid / 2.0, y + h - cell * (r as f64 + 0.5), (cell * 0.36).min(bwid / 2.0), 0.0, 2.0 * PI);
                    c.fill().unwrap();
                }
            }
        }
    }
}

/// A plugin's latest frame, scaled into a rounded pill; a placeholder until one arrives.
fn draw_surface(m: &Model, p: &Painter, pal: &Palette, id: &str, x: f64, w: f64, now: Instant) {
    let c = p.c;
    let (y, h) = (MARGIN_Y, m.h - 2.0 * MARGIN_Y);
    c.save().unwrap();
    rounded(c, x, y, w, h, RADIUS);
    c.clip();
    match m.surface(id, now) {
        Some(surf) => {
            let sx = w / surf.width() as f64;
            let sy = m.h / surf.height() as f64;
            c.translate(x, 0.0);
            c.scale(sx, sy);
            c.set_source_surface(surf, 0.0, 0.0).unwrap();
            c.source().set_filter(cairo::Filter::Good);
            c.paint().unwrap();
        }
        None => {
            pal.surface.mix(pal.bg, 0.4).set(c);
            c.paint().unwrap();
            pal.fg_dim.set(c);
            p.text(id, LABEL_PX * 0.8, false, x + w / 2.0, m.h / 2.0, Some(w - 12.0));
        }
    }
    c.restore().unwrap();
}

fn fmt_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn draw_viz(m: &Model, p: &Painter, pal: &Palette, v: &VizOverlay, now: Instant) {
    let c = p.c;
    let items = m.items();
    let playing = m.flag("playing");
    for (part, x, w) in m.overlay_parts() {
        let pl = press_level(m, Hit::Overlay(part), now);
        let button = |icon: &str, ink: Rgb| {
            p.pill(x, w, pal.surface.mix(pal.accent, 0.45 * pl));
            p.content(Some(icon), None, x, w, ink);
        };
        match part {
            Part::Pinned(n) => draw_item(m, p, pal, &items[n], n, x, w, now),
            Part::Close => button("󰅖", pal.fg),
            Part::Prev => button("󰒮", pal.fg),
            Part::Play => button(if playing { "󰏤" } else { "󰐊" }, pal.accent),
            Part::Next => button("󰒭", pal.fg),
            Part::Preset => button("󰑓", pal.fg),
            Part::Mode => {
                let icon = match v.mode.as_str() {
                    "bars" => "󰺢",
                    "milkdrop" => "󰸉",
                    "fmvideo" => "󰕧",
                    _ => "󰐱",
                };
                button(icon, pal.accent2)
            }
            Part::Art => {
                c.save().unwrap();
                rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                c.clip();
                match &m.art {
                    Some(img) => {
                        let s = (m.h - 2.0 * MARGIN_Y) / img.height() as f64;
                        c.translate(x + (w - img.width() as f64 * s) / 2.0, MARGIN_Y);
                        c.scale(s, s);
                        c.set_source_surface(img, 0.0, 0.0).unwrap();
                        c.paint().unwrap();
                    }
                    None => {
                        pal.surface.set(c);
                        c.paint().unwrap();
                        pal.accent.set(c);
                        p.text("󰝚", 30.0, false, x + w / 2.0, m.h / 2.0, None);
                    }
                }
                c.restore().unwrap();
            }
            Part::Viz => {
                let (y, h) = (MARGIN_Y, m.h - 2.0 * MARGIN_Y);
                if v.mode == "bars" {
                    pal.surface.mix(pal.bg, 0.5).set(c);
                    rounded(c, x, y, w, h, RADIUS);
                    c.fill().unwrap();
                    draw_bars(m, c, pal, x + 8.0, y + 4.0, w - 16.0, h - 8.0, m.bar_style, 0.95);
                } else {
                    draw_surface(m, p, pal, &v.mode, x, w, now);
                }
                // Title and time sit over the picture on a soft scrim.
                let title = m.text("title").unwrap_or("");
                let label = match m.text("artist") {
                    Some(a) if !title.is_empty() => format!("{title}  ·  {a}"),
                    _ => title.to_string(),
                };
                let pos = m.position(now);
                let shown = v.scrub.zip(pos.map(|(_, l)| l)).or(pos);
                if !label.is_empty() {
                    let tw = p.text_width(&label, 20.0, true).min(w * 0.6);
                    pal.bg.set_a(c, 0.55);
                    rounded(c, x + 8.0, y + 5.0, tw + 20.0, 28.0, 8.0);
                    c.fill().unwrap();
                    pal.fg.set(c);
                    p.text(&label, 20.0, true, x + 18.0 + tw / 2.0, y + 19.0, Some(tw + 1.0));
                }
                if let Some((pos, len)) = shown {
                    let t = format!("{} / {}", fmt_time(pos), fmt_time(len));
                    let tw = p.text_width(&t, 18.0, true);
                    pal.bg.set_a(c, 0.55);
                    rounded(c, x + w - tw - 28.0, y + 5.0, tw + 20.0, 28.0, 8.0);
                    c.fill().unwrap();
                    (if v.scrub.is_some() { pal.accent } else { pal.fg }).set(c);
                    p.text(&t, 18.0, true, x + w - 18.0 - tw / 2.0, y + 19.0, None);
                    // Progress line along the bottom edge; thicker while scrubbing.
                    let th = if v.scrub.is_some() { 5.0 } else { 3.0 };
                    pal.bg.set_a(c, 0.6);
                    c.rectangle(x + 10.0, y + h - th - 3.0, w - 20.0, th);
                    c.fill().unwrap();
                    pal.accent.set(c);
                    c.rectangle(x + 10.0, y + h - th - 3.0, (w - 20.0) * pos / len.max(1.0), th);
                    c.fill().unwrap();
                }
            }
            _ => {}
        }
    }
}

/// Colour the sky: sun warm, moon in the second accent, rain and storms in
/// the accent, the rest quiet.
fn weather_tint(pal: &Palette, kind: &str) -> Rgb {
    match kind {
        "sun" => pal.yellow,
        "moon" => pal.accent2,
        "rain" | "storm" => pal.accent,
        "snow" => pal.fg,
        "wind" => pal.fg,
        _ => pal.fg.mix(pal.fg_dim, 0.35),
    }
}

/// Meteorological direction is where the wind comes FROM; point downwind.
fn wind_arrow(c: &Context, x: f64, y: f64, from_deg: f64, scale: f64, ink: Rgb) {
    c.save().unwrap();
    c.translate(x, y);
    c.rotate((from_deg + 180.0).to_radians());
    c.scale(scale, scale);
    ink.set(c);
    c.move_to(0.0, -9.0);
    c.line_to(6.0, 5.0);
    c.line_to(0.0, 2.0);
    c.line_to(-6.0, 5.0);
    c.close_path();
    c.fill().unwrap();
    c.restore().unwrap();
}

fn draw_weather(m: &Model, p: &Painter, pal: &Palette, now: Instant) {
    let c = p.c;
    let items = m.items();
    let cy = m.h / 2.0;
    for (part, x, w) in m.overlay_parts() {
        let pl = press_level(m, Hit::Overlay(part), now);
        match part {
            Part::Pinned(n) => draw_item(m, p, pal, &items[n], n, x, w, now),
            Part::Close => {
                p.pill(x, w, pal.surface.mix(pal.accent, 0.45 * pl));
                p.content(Some("󰅖"), None, x, w, pal.fg);
            }
            Part::WNow => {
                p.pill(x, w, pal.surface);
                let Some(temp) = m.num("weather_temp") else {
                    pal.fg_dim.set(c);
                    p.text("Weather unavailable", LABEL_PX, true, x + w / 2.0, cy, None);
                    continue;
                };
                let icon = m.text("weather_icon").unwrap_or("󰼯");
                let tint = weather_tint(pal, m.text("weather_kind").unwrap_or(""));
                let mut cx = x + 18.0;
                tint.set(c);
                let iw = p.text_width(icon, 44.0, false);
                p.text(icon, 44.0, false, cx + iw / 2.0, cy, None);
                cx += iw + 12.0;
                let t = format!("{temp:.0}°");
                let tw = p.text_width(&t, 34.0, true);
                pal.fg.set(c);
                p.text(&t, 34.0, true, cx + tw / 2.0, cy, None);
                cx += tw + 16.0;
                // Two lines: what it's doing, then feels-like and today's range.
                let desc = m.text("weather_desc").unwrap_or("");
                let mut detail = vec![];
                if let Some(f) = m.num("weather_feels") {
                    detail.push(format!("feels {f:.0}°"));
                }
                if let (Some(hi), Some(lo)) = (m.num("weather_hi"), m.num("weather_lo")) {
                    detail.push(format!("↑{hi:.0}° ↓{lo:.0}°"));
                }
                let detail = detail.join("  ·  ");
                let right_w = 150.0;
                let col_w = (x + w - right_w - cx - 8.0).max(40.0);
                pal.fg.set(c);
                let dw = p.text_width(desc, 18.0, true).min(col_w);
                p.text(desc, 18.0, true, cx + dw / 2.0, cy - 11.0, Some(col_w));
                pal.fg_dim.mix(pal.fg, 0.35).set(c);
                let lw = p.text_width(&detail, 15.0, true).min(col_w);
                p.text(&detail, 15.0, true, cx + lw / 2.0, cy + 12.0, Some(col_w));
                // Wind and place on the right.
                let rx = x + w - right_w;
                if let Some(speed) = m.num("weather_wind") {
                    let unit = m.text("weather_wind_unit").unwrap_or("km/h");
                    let s = format!("{speed:.0} {unit}");
                    if let Some(dir) = m.num("weather_wind_dir") {
                        wind_arrow(c, rx + 12.0, cy - 11.0, dir, 1.0, pal.fg);
                    }
                    pal.fg.set(c);
                    let sw = p.text_width(&s, 17.0, true);
                    p.text(&s, 17.0, true, rx + 28.0 + sw / 2.0, cy - 11.0, None);
                }
                if let Some(place) = m.text("weather_place") {
                    pal.fg_dim.set(c);
                    let pw = p.text_width(place, 14.0, false).min(right_w - 8.0);
                    p.text(place, 14.0, false, rx + pw / 2.0, cy + 12.0, Some(right_w - 8.0));
                }
            }
            Part::WHours => {
                let hours: Vec<&serde_json::Value> = m
                    .state
                    .get("weather_hours")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().collect())
                    .unwrap_or_default();
                if hours.is_empty() {
                    continue;
                }
                let min_col = 96.0;
                let n = ((w / min_col) as usize).clamp(1, hours.len());
                let col = w / n as f64;
                for (i, h) in hours.iter().take(n).enumerate() {
                    let Some(h) = h.as_array() else { continue };
                    let get_s = |k: usize| h.get(k).and_then(|v| v.as_str()).unwrap_or("");
                    let (label, icon, kind) = (get_s(0), get_s(1), get_s(4));
                    let temp = h.get(2).and_then(|v| v.as_f64());
                    let pop = h.get(3).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let cx = x + col * (i as f64 + 0.5);
                    // Alternate faint panels so the hours read as columns.
                    (if i % 2 == 0 { pal.surface } else { pal.surface.mix(pal.bg, 0.4) }).set(c);
                    rounded(c, x + col * i as f64 + 2.0, MARGIN_Y, col - 4.0, m.h - 2.0 * MARGIN_Y, RADIUS);
                    c.fill().unwrap();
                    pal.fg_dim.mix(pal.fg, 0.3).set(c);
                    let top = if pop >= 20.0 { format!("{label}  {pop:.0}%") } else { label.to_string() };
                    p.text(&top, 14.0, true, cx, cy - 13.0, Some(col - 8.0));
                    let t = temp.map(|t| format!("{t:.0}°")).unwrap_or_default();
                    let iw = p.text_width(icon, 24.0, false);
                    let tw = p.text_width(&t, 18.0, true);
                    let sx = cx - (iw + 6.0 + tw) / 2.0;
                    weather_tint(pal, kind).set(c);
                    p.text(icon, 24.0, false, sx + iw / 2.0, cy + 10.0, None);
                    pal.fg.set(c);
                    p.text(&t, 18.0, true, sx + iw + 6.0 + tw / 2.0, cy + 10.0, None);
                }
            }
            _ => {}
        }
    }
}

fn hsv(h: f64, s: f64, v: f64) -> Rgb {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i64 % 6 {
        0 => Rgb(v, t, p),
        1 => Rgb(q, v, p),
        2 => Rgb(p, v, t),
        3 => Rgb(p, q, v),
        4 => Rgb(t, p, v),
        _ => Rgb(v, p, q),
    }
}

const MARQUEE_SPEED: f64 = 38.0; // px per second
const MARQUEE_REST: f64 = 1.5; // seconds to read the start before it moves
const COMET_W: f64 = 180.0;

/// A long title glides left and loops, trailed by a rainbow spectrum comet.
#[allow(clippy::too_many_arguments)]
fn draw_marquee(m: &Model, p: &Painter, pal: &Palette, text: &str, full: f64, tx: f64, avail: f64, now: Instant) {
    let c = p.c;
    let cy = m.h / 2.0;
    let t = (now - m.title_since).as_secs_f64();
    let moving = if m.flag("playing") { (t - MARQUEE_REST).max(0.0) } else { 0.0 };
    let (gap_a, gap_b) = (16.0, 48.0);
    let cycle = full + gap_a + COMET_W + gap_b;
    let off = (moving * MARQUEE_SPEED) % cycle;

    c.save().unwrap();
    c.rectangle(tx, MARGIN_Y, avail, m.h - 2.0 * MARGIN_Y);
    c.clip();
    for k in 0..2 {
        let bx = tx - off + k as f64 * cycle;
        if bx > tx + avail || bx + cycle < tx {
            continue;
        }
        pal.fg.set(c);
        p.text(text, LABEL_PX, false, bx + full / 2.0, cy, None);
        draw_comet(m, c, bx + full + gap_a, cy, t);
    }
    c.restore().unwrap();

    // Soft edges instead of a hard cut; the left one only once it has moved.
    let fade = 26.0;
    let edge = |x0: f64, x1: f64| {
        let g = LinearGradient::new(x0, 0.0, x1, 0.0);
        g.add_color_stop_rgba(0.0, pal.bg.0, pal.bg.1, pal.bg.2, 0.95);
        g.add_color_stop_rgba(1.0, pal.bg.0, pal.bg.1, pal.bg.2, 0.0);
        c.set_source(&g).unwrap();
        c.rectangle(x0.min(x1), MARGIN_Y, fade, m.h - 2.0 * MARGIN_Y);
        c.fill().unwrap();
    };
    if off > 0.0 {
        edge(tx, tx + fade);
    }
    edge(tx + avail, tx + avail - fade);
}

/// Rainbow bars fading into drifting coloured smoke. `x0` is the comet's head.
fn draw_comet(m: &Model, c: &Context, x0: f64, cy: f64, t: f64) {
    let h = m.h - 2.0 * MARGIN_Y - 8.0;
    // Smoke first, so the bars sit on top of it.
    for j in 0..8 {
        let fj = j as f64 / 7.0;
        let px = x0 + COMET_W * (0.12 + 0.88 * fj) + (t * 1.3 + j as f64 * 1.7).sin() * 7.0;
        let py = cy + (t * 0.9 + j as f64 * 2.3).sin() * 8.0 * fj;
        let r = 10.0 + 24.0 * fj;
        let col = hsv((fj * 0.7 + t * 0.07 + 0.55).fract(), 0.7, 1.0);
        let g = cairo::RadialGradient::new(px, py, 0.0, px, py, r);
        g.add_color_stop_rgba(0.0, col.0, col.1, col.2, 0.42 * (1.0 - fj * 0.6));
        g.add_color_stop_rgba(1.0, col.0, col.1, col.2, 0.0);
        c.set_source(&g).unwrap();
        c.arc(px, py, r, 0.0, 2.0 * PI);
        c.fill().unwrap();
    }
    // The spectrum: live when audio is flowing, a gentle idle wave otherwise.
    let n = 14;
    let step = COMET_W * 0.62 / n as f64;
    let live = m.bars_live(Instant::now()) && m.eq_on();
    for i in 0..n {
        let frac = i as f64 / n as f64;
        let level = if live {
            let k = (i * m.bars.len() / (n * 2)).min(m.bars.len().saturating_sub(1));
            m.bars.get(k).copied().unwrap_or(0.0) as f64
        } else {
            0.35 + 0.3 * (t * 5.0 + i as f64 * 0.7).sin()
        };
        let bh = ((0.2 + 0.8 * level.clamp(0.0, 1.0)) * h * (1.0 - frac * 0.5)).max(3.0);
        let col = hsv((frac * 0.85 + t * 0.12).fract(), 0.78, 1.0);
        c.set_source_rgba(col.0, col.1, col.2, 0.95 * (1.0 - frac).powf(1.3));
        let bw = step * 0.72;
        rounded(c, x0 + i as f64 * step, cy - bh / 2.0, bw, bh, bw / 2.0);
        c.fill().unwrap();
    }
}

/// Spectrum level at `f` (0..1 across the bands), linearly interpolated and
/// lightly smoothed, for visuals that want a continuous curve.
fn level_at(bars: &[f32], f: f64) -> f64 {
    if bars.is_empty() {
        return 0.0;
    }
    let n = bars.len();
    let pos = f.clamp(0.0, 1.0) * (n - 1) as f64;
    let i = pos.floor() as usize;
    let frac = pos - i as f64;
    let get = |k: isize| bars[k.clamp(0, n as isize - 1) as usize] as f64;
    let a = (get(i as isize - 1) + 2.0 * get(i as isize) + get(i as isize + 1)) / 4.0;
    let b = (get(i as isize) + 2.0 * get(i as isize + 1) + get(i as isize + 2)) / 4.0;
    (a + (b - a) * frac).clamp(0.0, 1.0)
}

/// Average energy over a band range.
fn band(bars: &[f32], from: f64, to: f64) -> f64 {
    if bars.is_empty() {
        return 0.0;
    }
    let n = bars.len();
    let (a, b) = ((from * n as f64) as usize, ((to * n as f64) as usize).max((from * n as f64) as usize + 1).min(n));
    bars[a..b].iter().map(|v| *v as f64).sum::<f64>() / (b - a) as f64
}

/// Rainbow rings spreading like ripples on water, each source pulsing with
/// its slice of the spectrum.
#[allow(clippy::too_many_arguments)]
fn draw_ripple(m: &Model, c: &Context, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
    let sources = 5;
    let cy = y + h / 2.0;
    c.save().unwrap();
    c.rectangle(x, y, w, h);
    c.clip();
    for i in 0..sources {
        let fi = i as f64 / (sources - 1) as f64;
        let e = band(&m.bars, fi * 0.8, fi * 0.8 + 0.2);
        let sx = x + w * (0.1 + 0.8 * fi) + (t * 0.4 + i as f64).sin() * 18.0;
        let reach = w / sources as f64 * 1.3;
        for k in 0..5 {
            let phase = (t * (0.45 + 0.1 * fi) + k as f64 / 5.0 + i as f64 * 0.17).fract();
            let r = 6.0 + phase * reach * (0.5 + 0.8 * e);
            let a = (1.0 - phase).powf(1.6) * (0.25 + 0.9 * e) * alpha;
            let col = hsv((fi * 0.8 + t * 0.05 + phase * 0.35).fract(), 0.75, 1.0);
            c.set_source_rgba(col.0, col.1, col.2, a.min(1.0));
            c.set_line_width(1.5 + 4.0 * e * (1.0 - phase));
            c.save().unwrap();
            c.translate(sx, cy);
            c.scale(1.0, 0.34); // water seen at a low angle
            c.arc(0.0, 0.0, r, 0.0, 2.0 * PI);
            c.restore().unwrap();
            c.stroke().unwrap();
        }
        // A bright drop at each source.
        let col = hsv((fi * 0.8 + t * 0.05).fract(), 0.6, 1.0);
        let g = cairo::RadialGradient::new(sx, cy, 0.0, sx, cy, 6.0 + 16.0 * e);
        g.add_color_stop_rgba(0.0, col.0, col.1, col.2, 0.9 * alpha);
        g.add_color_stop_rgba(1.0, col.0, col.1, col.2, 0.0);
        c.set_source(&g).unwrap();
        c.arc(sx, cy, 6.0 + 16.0 * e, 0.0, 2.0 * PI);
        c.fill().unwrap();
    }
    c.restore().unwrap();
}

/// Layers of smooth, translucent rainbow waves flowing across the bar.
#[allow(clippy::too_many_arguments)]
fn draw_aurora(m: &Model, c: &Context, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
    let cy = y + h / 2.0;
    let steps = (w / 8.0) as usize;
    c.save().unwrap();
    c.rectangle(x, y, w, h);
    c.clip();
    for layer in 0..3 {
        let lf = layer as f64;
        let speed = 0.35 + 0.25 * lf;
        let amp = h * (0.48 - 0.1 * lf);
        let curve = |i: usize| -> (f64, f64) {
            let f = i as f64 / steps as f64;
            let spectrum = level_at(&m.bars, f * 0.86 + lf * 0.07);
            let wave = (f * (5.0 + lf * 2.0) * PI + t * speed * 2.0 * PI).sin() * 0.18
                + (f * 13.0 * PI - t * 1.1).sin() * 0.07;
            (x + f * w, (spectrum * 0.75 + 0.28 + wave).clamp(0.06, 1.0) * amp)
        };
        // A closed band: the upper edge left to right, the mirrored lower edge back.
        c.new_path();
        for i in 0..=steps {
            let (px, a) = curve(i);
            c.line_to(px, cy - a);
        }
        for i in (0..=steps).rev() {
            let (px, a) = curve(i);
            c.line_to(px, cy + a * 0.8);
        }
        c.close_path();
        let g = LinearGradient::new(x, 0.0, x + w, 0.0);
        for s in 0..=6 {
            let f = s as f64 / 6.0;
            let col = hsv((f * 0.9 + t * 0.04 + lf * 0.23).fract(), 0.7, 1.0);
            g.add_color_stop_rgba(f, col.0, col.1, col.2, (0.42 - 0.08 * lf) * alpha);
        }
        c.set_source(&g).unwrap();
        c.fill().unwrap();
    }
    c.restore().unwrap();
}

/// An LED matrix in the style of Omarchy's screensaver: square pixels lit in
/// rainbow gradients, with falling peak pixels over a faint grid.
#[allow(clippy::too_many_arguments)]
fn draw_pixels(m: &Model, c: &Context, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
    let cell = 7.0;
    let rows = (h / cell).floor().max(1.0) as usize;
    let cols = (w / cell).floor().max(1.0) as usize;
    let oy = y + (h - rows as f64 * cell) / 2.0;
    for col in 0..cols {
        let f = col as f64 / cols as f64;
        let level = level_at(&m.bars, f);
        let peak = level_at(&m.peaks, f);
        let lit = (level * rows as f64).round() as usize;
        let peak_row = ((peak * rows as f64).round() as usize).min(rows);
        let px = x + col as f64 * cell;
        for r in 0..rows {
            let py = oy + (rows - 1 - r) as f64 * cell;
            let (rgb, a) = if r < lit {
                let v = 0.55 + 0.45 * (r as f64 / rows as f64);
                (hsv((f * 0.9 + t * 0.06 - r as f64 * 0.02).fract(), 0.8, v), 1.0)
            } else if r + 1 == peak_row && peak_row > lit {
                (hsv((f * 0.9 + t * 0.06).fract(), 0.25, 1.0), 0.85)
            } else {
                (Rgb(1.0, 1.0, 1.0), 0.05)
            };
            c.set_source_rgba(rgb.0, rgb.1, rgb.2, a * alpha);
            c.rectangle(px + 0.5, py + 0.5, cell - 1.5, cell - 1.5);
            c.fill().unwrap();
        }
    }
}

/// The marquee's comet, full width: rainbow bars over billowing smoke.
#[allow(clippy::too_many_arguments)]
fn draw_smoke_bars(m: &Model, c: &Context, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
    let cy = y + h / 2.0;
    c.save().unwrap();
    c.rectangle(x, y, w, h);
    c.clip();
    let puffs = (w / 46.0) as usize;
    for j in 0..puffs {
        let f = (j as f64 + 0.5) / puffs as f64;
        let e = level_at(&m.bars, f);
        let px = x + f * w + (t * 0.8 + j as f64 * 1.9).sin() * 12.0;
        let py = cy + (t * 0.6 + j as f64 * 2.7).sin() * 7.0;
        let r = 14.0 + 34.0 * e;
        let col = hsv((f * 0.85 + t * 0.05 + 0.5).fract(), 0.7, 1.0);
        let g = cairo::RadialGradient::new(px, py, 0.0, px, py, r);
        g.add_color_stop_rgba(0.0, col.0, col.1, col.2, (0.26 + 0.45 * e) * alpha);
        g.add_color_stop_rgba(1.0, col.0, col.1, col.2, 0.0);
        c.set_source(&g).unwrap();
        c.arc(px, py, r, 0.0, 2.0 * PI);
        c.fill().unwrap();
    }
    let n = (w / 11.0) as usize;
    let step = w / n as f64;
    for i in 0..n {
        let f = i as f64 / n as f64;
        let e = level_at(&m.bars, f);
        let bh = ((0.12 + 0.88 * e) * h).max(3.0);
        let col = hsv((f * 0.85 + t * 0.12).fract(), 0.78, 1.0);
        c.set_source_rgba(col.0, col.1, col.2, 0.92 * alpha);
        let bw = step * 0.6;
        rounded(c, x + i as f64 * step + (step - bw) / 2.0, cy - bh / 2.0, bw, bh, bw / 2.0);
        c.fill().unwrap();
    }
    c.restore().unwrap();
}
