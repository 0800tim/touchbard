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
        // Prefer now ÷ full: some gauges (ACPI SBS on Intel T2 Macs) report `capacity` against the
        // design capacity, so a worn battery never reads more than its health (85 % when full).
        let num = |f: &str| fs::read_to_string(p.join(f)).ok()?.trim().parse::<f64>().ok();
        let cap = match (num("charge_now").or(num("energy_now")), num("charge_full").or(num("energy_full"))) {
            (Some(now), Some(full)) if full > 0.0 => (now / full * 100.0).min(100.0),
            _ => num("capacity")?,
        };
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
    if let Some((msg, at)) = &m.toast {
        draw_toast(&p, &pal, m, msg, (now - *at).as_secs_f64());
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
                progress_bar(m, c, pal, x + 12.0, m.h - MARGIN_Y - 3.0, w - 24.0, 3.0, pos / len, false);
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
            // The cover, when there is one, sits to the left of everything.
            let art = if bg_mode != "off" { m.art.as_ref() } else { None };
            let side = m.h - 2.0 * MARGIN_Y - 6.0;
            let nw = if art.is_some() { side } else { p.text_width(note, 30.0, false) };
            let full = p.text_width(&text, LABEL_PX, false);
            let avail = (w - nw - 30.0).max(20.0);
            let scrolling = full > avail;
            m.marquee.set(scrolling && m.flag("playing"));
            let tw = full.min(avail);
            // Centred with the title; at the far left when lyrics take the space.
            let start = if m.karaoke_active() { x + 10.0 } else { x + (w - nw - 10.0 - tw) / 2.0 };
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
            if let Some(bt) = m.no_lyrics_banner(now) {
                // The condensed bar: the same message, one quiet pass.
                let msg = "No lyrics available";
                let mw = p.text_width(msg, LABEL_PX, true);
                let (ax, aw) = (x + nw + 24.0, w - nw - 34.0);
                let mx = ax + aw - (bt / NO_LYRICS_SECS) * (aw + mw);
                c.save().unwrap();
                c.rectangle(ax, 0.0, aw, m.h);
                c.clip();
                pal.fg_dim.set(c);
                p.text(msg, LABEL_PX, true, mx + mw / 2.0, cy, None);
                c.restore().unwrap();
            } else if m.karaoke_active() {
                m.marquee.set(m.flag("playing"));
                draw_lyrics(m, p, pal, x + nw + 24.0, w - nw - 34.0, cy, LABEL_PX, false, now);
            } else if scrolling {
                draw_marquee(m, p, pal, &text, full, tx, avail, now);
            } else {
                c.save().unwrap();
                c.rectangle(tx - 2.0, MARGIN_Y, avail + 4.0, m.h - 2.0 * MARGIN_Y);
                c.clip();
                pal.fg.set(c);
                p.text(&text, LABEL_PX, false, tx + tw / 2.0, cy, Some(tw + 1.0));
                c.restore().unwrap();
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
        3 => return draw_ripple(m, c, pal, x, y, w, h, t, alpha),
        4 => return draw_aurora(m, c, pal, x, y, w, h, t, alpha),
        5 => return draw_pixels(m, c, pal, x, y, w, h, t, alpha),
        6 => return draw_smoke_bars(m, c, pal, x, y, w, h, t, alpha),
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
        let col = viz_color(m, pal, t * 0.9, 0.78, 1.0);
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
            Part::Preset => draw_wand_button(m, p, pal, x, w, pl, now),
            Part::Colors => {
                // The palette icon wears the current mood's colours.
                p.pill(x, w, pal.surface.mix(pal.accent, 0.45 * pl));
                let t = (now - m.epoch).as_secs_f64();
                let g = LinearGradient::new(x + 18.0, 0.0, x + w - 18.0, 0.0);
                for k in 0..=4 {
                    let f = k as f64 / 4.0;
                    let col = viz_color(m, pal, f * 0.8 + t * 0.05, 0.85, 1.0);
                    g.add_color_stop_rgb(f, col.0, col.1, col.2);
                }
                c.set_source(&g).unwrap();
                p.text("\u{F03D8}", ICON_PX, false, x + w / 2.0, m.h / 2.0, None);
                c.new_path();
            }
            Part::Font => draw_font_button(m, p, pal, x, w, pl, now),
            Part::Volume => {
                // The speaker shows the level; lit while its control is open.
                let open = v.volume.is_some();
                let vol = m.num("volume").unwrap_or(0.0);
                let icon = if m.flag("muted") {
                    "\u{F075F}"
                } else if vol < 0.34 {
                    "\u{F057F}"
                } else if vol < 0.67 {
                    "\u{F0580}"
                } else {
                    "\u{F057E}"
                };
                p.pill(x, w, if open { pal.accent.mix(pal.surface, 0.35) } else { pal.surface.mix(pal.accent, 0.45 * pl) });
                p.content(Some(icon), None, x, w, if open { pal.bg } else { pal.fg });
            }
            Part::Karaoke => {
                // Lit up while karaoke is on.
                let on = m.flag("karaoke");
                p.pill(x, w, if on { pal.accent.mix(pal.surface, 0.35) } else { pal.surface.mix(pal.accent, 0.45 * pl) });
                p.content(Some("\u{F036C}"), None, x, w, if on { pal.bg } else { pal.fg });
            }
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
                    // Dimmed a little: the title's dot matrix is the star here.
                    draw_bars(m, c, pal, x + 8.0, y + 4.0, w - 16.0, h - 8.0, m.bar_style, 0.6);
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
                // With the sync adjuster open, lyrics play on the left of it.
                let (x, w) = match m.sync_open.and(m.sync_parts().first().map(|p| p.1)) {
                    Some(sx) => {
                        draw_sync(m, p, pal, now);
                        (x, (sx - x - 12.0).max(40.0))
                    }
                    None => (x, w),
                };
                let time_w = shown.map_or(0.0, |(pos, len)| {
                    p.text_width(&format!("{} / {}", fmt_time(pos), fmt_time(len)), 18.0, true) + 36.0
                });
                if v.volume.is_some() {
                    draw_volume_over(m, p, pal, x, w, y, h, now);
                } else if let Some(bt) = m.no_lyrics_banner(now) {
                    draw_no_lyrics(m, pal, c, x + 10.0, w - 20.0 - time_w, y + h / 2.0, bt, now);
                } else if m.karaoke_active() {
                    draw_lyrics(m, p, pal, x + 10.0, w - 20.0 - time_w, y + h / 2.0, 30.0, true, now);
                } else if !label.is_empty() {
                    match m.text_style {
                        1..=7 => draw_letter_title(m, p, pal, &label, x + 10.0, w - 20.0 - time_w, y, h, now),
                        _ => draw_dot_title(m, pal, c, &label, x + 10.0, w - 20.0 - time_w, y, h, now),
                    }
                }
                if let Some((pos, len)) = shown {
                    // The track time steps aside while the volume control is open.
                    if v.volume.is_none() && m.sync_open.is_none() {
                        let t = format!("{} / {}", fmt_time(pos), fmt_time(len));
                        let tw = p.text_width(&t, 18.0, true);
                        // With lyrics, a speedometer says: tap here to adjust their sync.
                        let hint = if m.karaoke_active() { 22.0 } else { 0.0 };
                        let (bx, bw) = (x + w - tw - 28.0 - hint, tw + 20.0 + hint);
                        m.time_rect.set((bx, bw));
                        pal.bg.set_a(c, 0.55);
                        rounded(c, bx, y + 5.0, bw, 28.0, 8.0);
                        c.fill().unwrap();
                        if hint > 0.0 {
                            pal.accent.set(c);
                            p.text("\u{F04C5}", 17.0, false, bx + 17.0, y + 19.0, None);
                        }
                        (if v.scrub.is_some() { pal.accent } else { pal.fg }).set(c);
                        p.text(&t, 18.0, true, x + w - 18.0 - tw / 2.0, y + 19.0, None);
                    } else {
                        m.time_rect.set((0.0, 0.0));
                    }
                    // Progress along the bottom edge; thicker, with a glowing knob, while scrubbing.
                    let th = if v.scrub.is_some() { 7.0 } else { 4.0 };
                    progress_bar(m, c, pal, x + 10.0, y + h - th - 3.0, w - 20.0, th, pos / len.max(1.0), v.scrub.is_some());
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
        draw_comet(m, c, pal, bx + full + gap_a, cy, t);
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
fn draw_comet(m: &Model, c: &Context, pal: &Palette, x0: f64, cy: f64, t: f64) {
    let h = m.h - 2.0 * MARGIN_Y - 8.0;
    // Smoke first, so the bars sit on top of it.
    for j in 0..8 {
        let fj = j as f64 / 7.0;
        let px = x0 + COMET_W * (0.12 + 0.88 * fj) + (t * 1.3 + j as f64 * 1.7).sin() * 7.0;
        let py = cy + (t * 0.9 + j as f64 * 2.3).sin() * 8.0 * fj;
        let r = 10.0 + 24.0 * fj;
        let col = viz_color(m, pal, fj * 0.7 + t * 0.07 + 0.55, 0.7, 1.0);
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
        let col = viz_color(m, pal, frac * 0.85 + t * 0.12, 0.78, 1.0);
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
fn draw_ripple(m: &Model, c: &Context, pal: &Palette, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
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
            let col = viz_color(m, pal, fi * 0.8 + t * 0.05 + phase * 0.35, 0.75, 1.0);
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
        let col = viz_color(m, pal, fi * 0.8 + t * 0.05, 0.6, 1.0);
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
fn draw_aurora(m: &Model, c: &Context, pal: &Palette, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
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
            let col = viz_color(m, pal, f * 0.9 + t * 0.04 + lf * 0.23, 0.7, 1.0);
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
fn draw_pixels(m: &Model, c: &Context, pal: &Palette, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
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
                (viz_color(m, pal, f * 0.9 + t * 0.06 - r as f64 * 0.02, 0.8, v), 1.0)
            } else if r + 1 == peak_row && peak_row > lit {
                (viz_color(m, pal, f * 0.9 + t * 0.06, 0.25, 1.0), 0.85)
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
fn draw_smoke_bars(m: &Model, c: &Context, pal: &Palette, x: f64, y: f64, w: f64, h: f64, t: f64, alpha: f64) {
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
        let col = viz_color(m, pal, f * 0.85 + t * 0.05 + 0.5, 0.7, 1.0);
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
        let col = viz_color(m, pal, f * 0.85 + t * 0.12, 0.78, 1.0);
        c.set_source_rgba(col.0, col.1, col.2, 0.92 * alpha);
        let bw = step * 0.6;
        rounded(c, x + i as f64 * step + (step - bw) / 2.0, cy - bh / 2.0, bw, bh, bw / 2.0);
        c.fill().unwrap();
    }
    c.restore().unwrap();
}

/// Colour at position `h` (0..1, cycling) for the visualisers: from the
/// track's artwork palette, the rainbow, or the theme's two accents. `s` and
/// `v` soften and dim it the way HSV would, so the styles read the same in
/// every colour mode.
fn viz_color(m: &Model, pal: &Palette, h: f64, s: f64, v: f64) -> Rgb {
    let h = h.rem_euclid(1.0);
    let mood = m.mood_name();
    if let Some((colours, stepped, dim)) = mood_palette(mood) {
        // Fixed moods: blended gradients, or hard colour blocks for disco.
        let n = colours.len();
        let base = if stepped {
            Rgb::parse(colours[(h * n as f64) as usize % n])
        } else {
            let pos = h * n as f64;
            let (i, f) = (pos.floor() as usize % n, pos - pos.floor());
            Rgb::parse(colours[i]).mix(Rgb::parse(colours[(i + 1) % n]), f * f * (3.0 - 2.0 * f))
        };
        let k = v * dim;
        return Rgb(base.0 * k, base.1 * k, base.2 * k);
    }
    let base = match mood {
        "rainbow" => return hsv(h, s, v),
        "theme" => pal.accent.mix(pal.accent2, 1.0 - (2.0 * h - 1.0).abs()),
        _ => {
            let cur = m.palette();
            if cur.is_empty() {
                return hsv(h, s, v);
            }
            let fade = ((Instant::now() - m.palette_at).as_secs_f64() / 1.5).min(1.0);
            let now = palette_lerp(&cur, h);
            if fade < 1.0 && !m.prev_palette.is_empty() {
                palette_lerp(&m.prev_palette, h).mix(now, fade)
            } else {
                now
            }
        }
    };
    let white = Rgb(1.0, 1.0, 1.0);
    let b = base.mix(white, ((0.78 - s) / 0.78).clamp(0.0, 1.0));
    Rgb(b.0 * v, b.1 * v, b.2 * v)
}

/// Smoothly around a cyclic palette.
fn palette_lerp(colours: &[String], h: f64) -> Rgb {
    let n = colours.len();
    let pos = h * n as f64;
    let i = pos.floor() as usize % n;
    let f = pos - pos.floor();
    let f = f * f * (3.0 - 2.0 * f); // ease between stops
    Rgb::parse(&colours[i]).mix(Rgb::parse(&colours[(i + 1) % n]), f)
}


thread_local! {
    /// The last title's dot raster, so it's rasterised once per track, not per frame.
    static DOTS: std::cell::RefCell<Option<(String, usize, usize, Vec<bool>)>> = const { std::cell::RefCell::new(None) };
}

/// Rasterise `text` with no antialiasing at a tiny size: each lit pixel
/// becomes one dot. Returns (columns, rows, lit), trimmed to the ink.
fn dot_raster(font: &str, text: &str) -> (usize, usize, Vec<bool>) {
    if let Some(hit) = DOTS.with(|d| d.borrow().as_ref().filter(|(t, ..)| t == text).map(|(_, a, b, v)| (*a, *b, v.clone()))) {
        return hit;
    }
    // Small raster, then thickened: few, big, bold dots, like Omarchy's block logo.
    let px = 8.0;
    let (w, h) = ((text.chars().count() as f64 * px + 8.0) as i32, (px * 1.8) as i32);
    let mut surf = cairo::ImageSurface::create(cairo::Format::A8, w.max(1), h.max(1)).unwrap();
    {
        let c = Context::new(&surf).unwrap();
        let l = pangocairo::functions::create_layout(&c);
        let mut opts = cairo::FontOptions::new().unwrap();
        opts.set_antialias(cairo::Antialias::None);
        opts.set_hint_style(cairo::HintStyle::Full);
        pangocairo::functions::context_set_font_options(&l.context(), Some(&opts));
        l.context_changed();
        let mut fd = pango::FontDescription::from_string(font);
        fd.set_absolute_size(px * pango::SCALE as f64);
        fd.set_weight(pango::Weight::Heavy);
        l.set_font_description(Some(&fd));
        l.set_text(text);
        c.move_to(2.0, 1.0);
        pangocairo::functions::show_layout(&c, &l);
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let data = surf.data().unwrap().to_vec();
    let (w, h) = (w as usize, h as usize);
    // One extra dot to the right of every stroke: two-dot-thick letters.
    let raw = |x: usize, y: usize| data[y * stride + x] > 110;
    let lit = |x: usize, y: usize| raw(x, y) || (x > 0 && raw(x - 1, y));
    let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| lit(x, y))).collect();
    let cols: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| lit(x, y))).collect();
    let (Some(&y0), Some(&y1), Some(&x0), Some(&x1)) = (rows.first(), rows.last(), cols.first(), cols.last()) else {
        return (0, 0, vec![]);
    };
    let (cw, rh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = vec![false; cw * rh];
    for y in 0..rh {
        for x in 0..cw {
            out[y * cw + x] = lit(x0 + x, y0 + y);
        }
    }
    DOTS.with(|d| *d.borrow_mut() = Some((text.to_string(), cw, rh, out.clone())));
    (cw, rh, out)
}

/// The full-screen title as a big dot-matrix display that is itself an
/// equaliser: each column of dots rides its slice of the spectrum, the beat
/// jolts and jitters it, and every dot takes a flowing gradient in the
/// song's colours. Long titles scroll.
#[allow(clippy::too_many_arguments)]
fn draw_dot_title(m: &Model, pal: &Palette, c: &Context, text: &str, x: f64, w: f64, y: f64, h: f64, now: Instant) {
    let (cols, rows, lit) = dots_5x7(text).unwrap_or_else(|| dot_raster(&m.theme.font, text));
    if cols == 0 || w <= 0.0 {
        return;
    }
    let t = (now - m.epoch).as_secs_f64();
    let beat = m.beat_level(now);
    let cell = ((h - 8.0) / rows as f64).floor().clamp(3.0, 7.0);
    let text_w = cols as f64 * cell;
    let amp = (h - rows as f64 * cell) / 2.0 - 1.0;
    let cy = y + h / 2.0;
    // Scroll when it doesn't fit, like the marquee.
    let (start, gap) = (x, 60.0);
    let off = if text_w > w {
        let moving = if m.flag("playing") { ((now - m.title_since).as_secs_f64() - MARQUEE_REST).max(0.0) } else { 0.0 };
        (moving * MARQUEE_SPEED) % (text_w + gap)
    } else {
        -(w - text_w) / 2.0
    };
    c.save().unwrap();
    c.rectangle(x, y, w, h);
    c.clip();
    let copies = if text_w > w { 2 } else { 1 };
    for k in 0..copies {
        let ox = start - off + k as f64 * (text_w + gap);
        for col in 0..cols {
            let px = ox + col as f64 * cell;
            if px + cell < x || px > x + w {
                continue;
            }
            let f = col as f64 / cols as f64;
            let level = level_at(&m.bars, f);
            let cf = col as f64;
            // How this column moves: each equaliser style gives the title its own personality.
            let dy = match m.bar_style {
                // peaks: letters bounce on the falling peak caps
                1 => -(level_at(&m.peaks, f) - 0.3) * amp * 1.6,
                // dots: a steady title that breathes instead of moving
                2 => 0.0,
                // ripple: a wave rolls through the lettering
                3 => (cf * 0.22 - t * 4.0).sin() * amp * (0.35 + 0.65 * level),
                // aurora: slow, misty drift
                4 => (cf * 0.07 + t * 0.9).sin() * amp * 0.55 + (t * 0.5 + cf * 0.02).cos() * 1.5,
                // pixels (negative) and comet: gentle spectrum ride
                5 | 6 => -(level - 0.35) * amp * 0.9,
                // bars: the reactive one, jolting and jittering on the beat
                _ => -(level - 0.35) * amp * 1.4 - beat * 3.0 + (t * 47.0 + cf * 0.9).sin() * beat * 1.6,
            }
            .clamp(-amp, amp);
            let top = cy - rows as f64 * cell / 2.0 + dy;
            if m.bar_style == 4 && col % 6 == 2 {
                // aurora: one soft haze per letter, like smoke in space
                let hz = viz_color(m, pal, f * 0.9 + t * 0.08, 0.85, 1.0);
                c.set_source_rgba(hz.0, hz.1, hz.2, 0.16);
                c.arc(px, cy + dy, rows as f64 * cell * 0.62, 0.0, 2.0 * PI);
                c.fill().unwrap();
            }
            for row in 0..rows {
                if !lit[row * cols + col] {
                    continue;
                }
                let hue = f * 0.9 + t * 0.08 + row as f64 * 0.018;
                // Lifted toward white so the title stands out from the visualiser behind.
                let lift = title_lift(m, 0.28 - 0.18 * level);
                let col_rgb = viz_color(m, pal, hue, 0.85, 1.0).mix(Rgb(1.0, 1.0, 1.0), lift);
                let (dx0, dy0) = (px + 0.5, top + row as f64 * cell + 0.5);
                let d = cell - 1.0;
                match m.bar_style {
                    2 => {
                        // dots: each dot breathes with its column
                        let size = d * (0.55 + 0.45 * level) * (1.0 + 0.25 * beat);
                        c.set_source_rgb(col_rgb.0, col_rgb.1, col_rgb.2);
                        c.arc(dx0 + d / 2.0, dy0 + d / 2.0, size / 2.0, 0.0, 2.0 * PI);
                        c.fill().unwrap();
                        continue;
                    }
                    5 => {
                        // pixels: the negative, punched out of the lit matrix, with a coloured rim
                        pal.bg.set(c);
                        rounded(c, dx0, dy0, d, d, d * 0.32);
                        c.fill_preserve().unwrap();
                        c.set_source_rgba(col_rgb.0, col_rgb.1, col_rgb.2, 0.75);
                        c.set_line_width(1.0);
                        c.stroke().unwrap();
                        continue;
                    }
                    6 => {
                        // comet: a short smoky trail behind every dot
                        for k in 1..=2 {
                            let kf = k as f64;
                            let tr = viz_color(m, pal, hue + kf * 0.05, 0.8, 1.0);
                            c.set_source_rgba(tr.0, tr.1, tr.2, 0.3 / kf);
                            let sz = d * (0.8 - 0.2 * kf);
                            c.rectangle(dx0 + kf * cell * 1.1, dy0 + (d - sz) / 2.0 + (t * 3.0 + kf).sin(), sz, sz);
                            c.fill().unwrap();
                        }
                    }
                    _ => {}
                }
                // A dark shadow under each dot keeps it legible over busy visuals.
                pal.bg.set_a(c, 0.65);
                c.rectangle(dx0 + 1.0, dy0 + 1.0, d, d);
                c.fill().unwrap();
                c.set_source_rgb(col_rgb.0, col_rgb.1, col_rgb.2);
                if d >= 4.0 {
                    rounded(c, dx0, dy0, d, d, d * 0.3);
                } else {
                    c.rectangle(dx0, dy0, d, d);
                }
                c.fill().unwrap();
            }
        }
    }
    c.restore().unwrap();
}

/// fm.video's brand spectrum: pink, magenta, purple.
const FM_PINK: Rgb = Rgb(1.0, 0.180, 0.604);
const FM_MAGENTA: Rgb = Rgb(0.808, 0.204, 0.776);
const FM_PURPLE: Rgb = Rgb(0.608, 0.302, 1.0);

/// Track progress in fm.video's pink-to-purple gradient, rounded, with a
/// glowing knob while it's being dragged.
#[allow(clippy::too_many_arguments)]
fn progress_bar(m: &Model, c: &Context, pal: &Palette, x: f64, y: f64, w: f64, th: f64, frac: f64, scrubbing: bool) {
    let frac = frac.clamp(0.0, 1.0);
    pal.bg.set_a(c, 0.55);
    rounded(c, x, y, w, th, th / 2.0);
    c.fill().unwrap();
    // The gradient spans the whole track, so each colour belongs to a place in the song.
    // fm.video's pink-to-purple with the song's own colours; otherwise the
    // mood's gradient, so greyscale stays greyscale and matrix stays green.
    let stops = if m.mood_name() == "music" {
        [FM_PINK, FM_MAGENTA, FM_PURPLE]
    } else {
        [viz_color(m, pal, 0.05, 0.8, 1.0), viz_color(m, pal, 0.4, 0.8, 1.0), viz_color(m, pal, 0.75, 0.8, 1.0)]
    };
    let g = LinearGradient::new(x, 0.0, x + w, 0.0);
    let soft = if scrubbing { 1.0 } else { 0.8 };
    for (at, col) in [(0.0, stops[0]), (0.5, stops[1]), (1.0, stops[2])] {
        g.add_color_stop_rgba(at, col.0, col.1, col.2, soft);
    }
    c.set_source(&g).unwrap();
    rounded(c, x, y, (w * frac).max(th), th, th / 2.0);
    c.fill().unwrap();
    if scrubbing {
        let (kx, ky) = (x + w * frac, y + th / 2.0);
        let tip = stops[0].mix(stops[2], frac);
        let glow = cairo::RadialGradient::new(kx, ky, 0.0, kx, ky, 16.0);
        glow.add_color_stop_rgba(0.0, tip.0, tip.1, tip.2, 0.55);
        glow.add_color_stop_rgba(1.0, tip.0, tip.1, tip.2, 0.0);
        c.set_source(&glow).unwrap();
        c.arc(kx, ky, 16.0, 0.0, 2.0 * PI);
        c.fill().unwrap();
        c.set_source_rgb(1.0, 1.0, 1.0);
        c.arc(kx, ky, th * 0.85, 0.0, 2.0 * PI);
        c.fill().unwrap();
    }
}

/// The classic 5x7 LED-sign font for printable ASCII (0x20..0x7E), one
/// byte per column, bit 0 at the top. Legible even at a handful of dots.
const FONT_5X7: [[u8; 5]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x5F, 0x00, 0x00],
    [0x00, 0x07, 0x00, 0x07, 0x00],
    [0x14, 0x7F, 0x14, 0x7F, 0x14],
    [0x24, 0x2A, 0x7F, 0x2A, 0x12],
    [0x23, 0x13, 0x08, 0x64, 0x62],
    [0x36, 0x49, 0x55, 0x22, 0x50],
    [0x00, 0x05, 0x03, 0x00, 0x00],
    [0x00, 0x1C, 0x22, 0x41, 0x00],
    [0x00, 0x41, 0x22, 0x1C, 0x00],
    [0x08, 0x2A, 0x1C, 0x2A, 0x08],
    [0x08, 0x08, 0x3E, 0x08, 0x08],
    [0x00, 0x50, 0x30, 0x00, 0x00],
    [0x08, 0x08, 0x08, 0x08, 0x08],
    [0x00, 0x60, 0x60, 0x00, 0x00],
    [0x20, 0x10, 0x08, 0x04, 0x02],
    [0x3E, 0x51, 0x49, 0x45, 0x3E],
    [0x00, 0x42, 0x7F, 0x40, 0x00],
    [0x42, 0x61, 0x51, 0x49, 0x46],
    [0x21, 0x41, 0x45, 0x4B, 0x31],
    [0x18, 0x14, 0x12, 0x7F, 0x10],
    [0x27, 0x45, 0x45, 0x45, 0x39],
    [0x3C, 0x4A, 0x49, 0x49, 0x30],
    [0x01, 0x71, 0x09, 0x05, 0x03],
    [0x36, 0x49, 0x49, 0x49, 0x36],
    [0x06, 0x49, 0x49, 0x29, 0x1E],
    [0x00, 0x36, 0x36, 0x00, 0x00],
    [0x00, 0x56, 0x36, 0x00, 0x00],
    [0x08, 0x14, 0x22, 0x41, 0x00],
    [0x14, 0x14, 0x14, 0x14, 0x14],
    [0x00, 0x41, 0x22, 0x14, 0x08],
    [0x02, 0x01, 0x51, 0x09, 0x06],
    [0x32, 0x49, 0x79, 0x41, 0x3E],
    [0x7E, 0x11, 0x11, 0x11, 0x7E],
    [0x7F, 0x49, 0x49, 0x49, 0x36],
    [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x22, 0x1C],
    [0x7F, 0x49, 0x49, 0x49, 0x41],
    [0x7F, 0x09, 0x09, 0x01, 0x01],
    [0x3E, 0x41, 0x41, 0x51, 0x32],
    [0x7F, 0x08, 0x08, 0x08, 0x7F],
    [0x00, 0x41, 0x7F, 0x41, 0x00],
    [0x20, 0x40, 0x41, 0x3F, 0x01],
    [0x7F, 0x08, 0x14, 0x22, 0x41],
    [0x7F, 0x40, 0x40, 0x40, 0x40],
    [0x7F, 0x02, 0x04, 0x02, 0x7F],
    [0x7F, 0x04, 0x08, 0x10, 0x7F],
    [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 0x09, 0x09, 0x09, 0x06],
    [0x3E, 0x41, 0x51, 0x21, 0x5E],
    [0x7F, 0x09, 0x19, 0x29, 0x46],
    [0x46, 0x49, 0x49, 0x49, 0x31],
    [0x01, 0x01, 0x7F, 0x01, 0x01],
    [0x3F, 0x40, 0x40, 0x40, 0x3F],
    [0x1F, 0x20, 0x40, 0x20, 0x1F],
    [0x7F, 0x20, 0x18, 0x20, 0x7F],
    [0x63, 0x14, 0x08, 0x14, 0x63],
    [0x03, 0x04, 0x78, 0x04, 0x03],
    [0x61, 0x51, 0x49, 0x45, 0x43],
    [0x00, 0x7F, 0x41, 0x41, 0x00],
    [0x02, 0x04, 0x08, 0x10, 0x20],
    [0x00, 0x41, 0x41, 0x7F, 0x00],
    [0x04, 0x02, 0x01, 0x02, 0x04],
    [0x40, 0x40, 0x40, 0x40, 0x40],
    [0x00, 0x01, 0x02, 0x04, 0x00],
    [0x20, 0x54, 0x54, 0x54, 0x78],
    [0x7F, 0x48, 0x44, 0x44, 0x38],
    [0x38, 0x44, 0x44, 0x44, 0x20],
    [0x38, 0x44, 0x44, 0x48, 0x7F],
    [0x38, 0x54, 0x54, 0x54, 0x18],
    [0x08, 0x7E, 0x09, 0x01, 0x02],
    [0x08, 0x14, 0x54, 0x54, 0x3C],
    [0x7F, 0x08, 0x04, 0x04, 0x78],
    [0x00, 0x44, 0x7D, 0x40, 0x00],
    [0x20, 0x40, 0x44, 0x3D, 0x00],
    [0x7F, 0x10, 0x28, 0x44, 0x00],
    [0x00, 0x41, 0x7F, 0x40, 0x00],
    [0x7C, 0x04, 0x18, 0x04, 0x78],
    [0x7C, 0x08, 0x04, 0x04, 0x78],
    [0x38, 0x44, 0x44, 0x44, 0x38],
    [0x7C, 0x14, 0x14, 0x14, 0x08],
    [0x08, 0x14, 0x14, 0x18, 0x7C],
    [0x7C, 0x08, 0x04, 0x04, 0x08],
    [0x48, 0x54, 0x54, 0x54, 0x20],
    [0x04, 0x3F, 0x44, 0x40, 0x20],
    [0x3C, 0x40, 0x40, 0x20, 0x7C],
    [0x1C, 0x20, 0x40, 0x20, 0x1C],
    [0x3C, 0x40, 0x30, 0x40, 0x3C],
    [0x44, 0x28, 0x10, 0x28, 0x44],
    [0x0C, 0x50, 0x50, 0x50, 0x3C],
    [0x44, 0x64, 0x54, 0x4C, 0x44],
    [0x00, 0x08, 0x36, 0x41, 0x00],
    [0x00, 0x00, 0x7F, 0x00, 0x00],
    [0x00, 0x41, 0x36, 0x08, 0x00],
    [0x08, 0x04, 0x08, 0x10, 0x08],
];

/// A character's 5x7 columns: ASCII directly, common accents folded to their
/// base letter, the middle dot as a dot. None if the font can't show it.
fn glyph_5x7(ch: char) -> Option<[u8; 5]> {
    let base = match ch {
        '·' | '•' => return Some([0x00, 0x00, 0x08, 0x00, 0x00]),
        '–' | '—' => '-',
        '‘' | '’' => '\'',
        '“' | '”' => '"',
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'Á' | 'À' | 'Â' | 'Ä' | 'Ã' | 'Å' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' => 'o',
        'Ó' | 'Ò' | 'Ô' | 'Ö' | 'Õ' | 'Ø' => 'O',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ç' => 'c',
        'Ç' => 'C',
        c => c,
    };
    let code = base as u32;
    (0x20..=0x7E).contains(&code).then(|| FONT_5X7[(code - 0x20) as usize])
}

/// The title in the 5x7 font: (columns, rows, lit), or None if any character
/// is outside it (then the rasterised font takes over).
fn dots_5x7(text: &str) -> Option<(usize, usize, Vec<bool>)> {
    let glyphs: Option<Vec<[u8; 5]>> = text.chars().map(glyph_5x7).collect();
    let glyphs = glyphs?;
    let cols = glyphs.len() * 6;
    let rows = 7;
    let mut lit = vec![false; cols * rows];
    for (i, g) in glyphs.iter().enumerate() {
        for (cx, byte) in g.iter().enumerate() {
            for row in 0..rows {
                if byte >> row & 1 == 1 {
                    lit[row * cols + i * 6 + cx] = true;
                }
            }
        }
    }
    Some((cols, rows, lit))
}

/// The fixed colour moods: (colours, stepped rather than blended, brightness).
fn mood_palette(mood: &str) -> Option<(&'static [&'static str], bool, f64)> {
    Some(match mood {
        "mono" => (&["#f4f4f4", "#9a9a9a", "#3a3a3a", "#c8c8c8", "#6a6a6a"][..], false, 0.9),
        "smoke" => (&["#1f3b5c", "#4a78a6", "#8fb3d4", "#2c5480", "#6b93bd"][..], false, 0.85),
        "amethyst" => (&["#2e1052", "#5b24a0", "#8a4fd0", "#401777", "#b07fe8"][..], false, 0.85),
        "matrix" => (&["#00ff41", "#008f11", "#39ff14", "#005c0b", "#00c832"][..], false, 1.0),
        "disco" => (&["#ff0055", "#ffcc00", "#00d4ff", "#7cff00", "#b000ff", "#ff6a00"][..], true, 1.0),
        // Rastafari red, gold and green, in flag-like bands.
        "rasta" => (&["#e31b23", "#fcd116", "#009b3a"][..], true, 1.0),
        _ => return None,
    })
}

/// How far the title's colour is lifted toward white so it reads over the
/// visuals: dark moods need it, bright ones (matrix, disco) stay pure.
fn title_lift(m: &Model, default: f64) -> f64 {
    match m.mood_name() {
        "smoke" | "amethyst" => 0.4,
        "mono" => 0.1,
        "matrix" | "disco" | "rasta" => 0.0,
        _ => default,
    }
}

/// Typewriter timing: letters shown at `secs` into the cycle (type, hold, erase, pause).
fn typewriter_count(n: usize, secs: f64) -> usize {
    let (typing, hold, erasing, pause) = (n as f64 / 12.0, 3.0, n as f64 / 30.0, 0.6);
    let tc = secs % (typing + hold + erasing + pause);
    if tc < typing {
        (tc * 12.0) as usize
    } else if tc < typing + hold {
        n
    } else if tc < typing + hold + erasing {
        n.saturating_sub(((tc - typing - hold) * 30.0) as usize)
    } else {
        0
    }
}

/// One letter in the 5x7 font as square pixels of side `cell`, top-left at (x, top).
fn pixel_letter(c: &Context, ch: char, x: f64, top: f64, cell: f64, col: Rgb, alpha: f64) {
    let g = glyph_5x7(ch).unwrap_or_else(|| glyph_5x7('?').unwrap());
    c.set_source_rgba(col.0, col.1, col.2, alpha);
    for (cx, byte) in g.iter().enumerate() {
        for row in 0..7 {
            if byte >> row & 1 == 1 {
                c.rectangle(x + cx as f64 * cell, top + row as f64 * cell, cell - 1.0, cell - 1.0);
            }
        }
    }
    c.fill().unwrap();
}

/// The full-screen title drawn letter by letter. Each text style has its own
/// type and its own response to the music:
/// 1 wave (bold letters bobbing on their frequencies), 2 outline (huge hollow
/// glowing letters drifting), 3 typewriter (crisp pixels typed out, cursor
/// flashing on the beat), 4 scatter (letters drifting like particles in smoke,
/// pushed out by beats), 5 blocks (chunky pixels lighting up in sequence).
#[allow(clippy::too_many_arguments)]
fn draw_letter_title(m: &Model, p: &Painter, pal: &Palette, text: &str, x: f64, w: f64, y: f64, h: f64, now: Instant) {
    let c = p.c;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() || w <= 0.0 {
        return;
    }
    let t = (now - m.epoch).as_secs_f64();
    let beat = m.beat_level(now);
    let cy = y + h / 2.0;
    let n = chars.len();
    let style = m.text_style;
    let white = Rgb(1.0, 1.0, 1.0);
    let lift = |col: Rgb, k: f64| col.mix(white, title_lift(m, k));
    // Pixel styles sit on the 5x7 grid; the others use the real font.
    let (px, cell) = match style {
        2 => (46.0, 0.0),
        3 => (0.0, 4.0),
        5..=7 => (0.0, 6.0),
        _ => (34.0, 0.0),
    };
    let pixel = cell > 0.0;
    let widths: Vec<f64> = chars
        .iter()
        .map(|ch| if pixel { glyph_advance(style) * cell } else { p.text_width(&ch.to_string(), px, true).max(px * 0.3) })
        .collect();
    let total: f64 = widths.iter().sum();
    let gap = 70.0;
    let off = if total > w {
        let moving = if m.flag("playing") { ((now - m.title_since).as_secs_f64() - MARQUEE_REST).max(0.0) } else { 0.0 };
        (moving * MARQUEE_SPEED) % (total + gap)
    } else {
        -(w - total) / 2.0
    };
    let shown = if style == 3 { typewriter_count(n, (now - m.title_since).as_secs_f64()) } else { n };

    c.save().unwrap();
    c.rectangle(x, y, w, h);
    c.clip();
    let copies = if total > w { 2 } else { 1 };
    for k in 0..copies {
        let mut lx = x - off + k as f64 * (total + gap);
        for (i, ch) in chars.iter().enumerate() {
            let lw = widths[i];
            if i >= shown || lx + lw < x - 20.0 || lx > x + w + 20.0 || ch.is_whitespace() {
                lx += lw;
                continue;
            }
            let fi = i as f64;
            let f = fi / n as f64;
            let level = level_at(&m.bars, f);
            let hue = f * 0.9 + t * 0.08;
            let s = ch.to_string();
            match style {
                1 => {
                    // wave: bold letters bobbing, each on its own frequency
                    let dy = (t * 3.2 + fi * 0.55).sin() * 3.5 - (level - 0.3) * 10.0 - beat * 3.0;
                    let col = lift(viz_color(m, pal, hue, 0.85, 1.0), 0.2);
                    pal.bg.set_a(c, 0.7);
                    p.text(&s, px, true, lx + lw / 2.0 + 1.5, cy + dy + 1.5, None);
                    col.set(c);
                    p.text(&s, px, true, lx + lw / 2.0, cy + dy, None);
                }
                2 => {
                    // outline: huge hollow letters with a soft glow, drifting slowly
                    let dy = (t * 0.8 + fi * 0.35).sin() * 3.0;
                    let l = p.layout(&s, px, true);
                    let (_, ext) = l.pixel_extents();
                    let scale = 1.0 + 0.06 * beat;
                    c.save().unwrap();
                    c.translate(lx + lw / 2.0, cy + dy);
                    c.scale(scale, scale);
                    c.move_to(-ext.width() as f64 / 2.0, -ext.height() as f64 / 2.0);
                    pangocairo::functions::layout_path(c, &l);
                    c.restore().unwrap();
                    let col = lift(viz_color(m, pal, hue, 0.85, 1.0), 0.15);
                    c.set_source_rgba(col.0, col.1, col.2, 0.18 + 0.2 * level);
                    c.set_line_width(6.0);
                    c.stroke_preserve().unwrap();
                    c.set_source_rgba(col.0, col.1, col.2, 0.95);
                    c.set_line_width(1.6);
                    c.stroke().unwrap();
                }
                3 => {
                    // typewriter: crisp pixels; the newest letter glows
                    let newest = i + 1 == shown && shown < n;
                    let col = viz_color(m, pal, hue, 0.85, 0.6 + 0.4 * level).mix(white, title_lift(m, 0.0));
                    let col = if newest { col.mix(white, 0.6) } else { col };
                    pixel_letter(c, *ch, lx, cy - 3.5 * cell, cell, col, 1.0);
                }
                4 => {
                    // scatter: letters adrift like particles in smoke, pushed out by beats
                    let dx = (t * 0.7 + fi * 1.3).sin() * 5.0 + (t * 0.3 + fi).cos() * 3.0 + beat * (fi * 2.1).sin() * 8.0;
                    let dy = (t * 0.9 + fi * 0.7).cos() * 6.0 + beat * (fi * 1.7).cos() * 6.0;
                    let a = 0.55 + 0.45 * (0.5 + 0.5 * (t * 1.1 + fi).sin());
                    let col = lift(viz_color(m, pal, hue, 0.8, 1.0), 0.15);
                    for (sx, sy, sa) in [(-4.0, 2.0, 0.12), (3.0, -2.0, 0.12)] {
                        c.set_source_rgba(col.0, col.1, col.2, sa * a);
                        p.text(&s, px, true, lx + lw / 2.0 + dx + sx, cy + dy + sy, None);
                    }
                    c.set_source_rgba(col.0, col.1, col.2, a);
                    p.text(&s, px, true, lx + lw / 2.0 + dx, cy + dy, None);
                }
                6 => sparkle_letter(m, pal, c, *ch, lx, cy - 3.5 * cell, cell, i, t, now, 1.0),
                7 => {
                    // explode: on strong beats a few letters blast apart and snap back
                    let since = (now - m.beat_at).as_secs_f64();
                    let chosen = hash01(fi, m.beats as f64) > 0.55 && m.beat > 0.5 && m.flag("playing");
                    let burst = if chosen && since < 0.6 { (PI * since / 0.6).sin() * 0.55 } else { 0.0 };
                    if burst > 0.01 {
                        explode_letter(m, pal, c, *ch, lx, cy - 3.5 * cell, cell, i, t, burst, 1.0);
                    } else {
                        sparkle_letter(m, pal, c, *ch, lx, cy - 3.5 * cell, cell, i, t, now, 1.0);
                    }
                }
                _ => {
                    // blocks: chunky pixels; letters light up in sequence on each beat
                    let seq = (m.beats as usize) % n.max(1);
                    let hit = if i == seq || (i + m.beats as usize) % 4 == 0 { beat } else { 0.0 };
                    let bright = (0.3 + 0.7 * level.max(hit)).min(1.0);
                    let col = viz_color(m, pal, f + m.beats as f64 * 0.137, 0.9, bright);
                    let jump = hit * 4.0;
                    pixel_letter(c, *ch, lx, cy - 3.5 * cell - jump, cell, col, 1.0);
                }
            }
            lx += lw;
        }
        // The typewriter's cursor, flashing on the beat.
        if style == 3 && k == 0 && shown < n {
            let cx = x - off + widths[..shown].iter().sum::<f64>();
            let on = (t * 2.0).fract() < 0.5 || beat > 0.3;
            if on {
                let col = viz_color(m, pal, t * 0.08, 0.85, 1.0).mix(white, 0.3 * beat);
                c.set_source_rgb(col.0, col.1, col.2);
                c.rectangle(cx + 1.0, cy - 3.5 * cell, 5.0 * cell - 1.0, 7.0 * cell - 1.0);
                c.fill().unwrap();
            }
        }
    }
    c.restore().unwrap();
}

/// A message bubble in the middle of the bar, fading in and out.
fn draw_toast(p: &Painter, pal: &Palette, m: &Model, msg: &str, age: f64) {
    let c = p.c;
    let a = (age / 0.15).min(1.0) * ((1.8 - age) / 0.35).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let tw = p.text_width(msg, 24.0, true) + 64.0;
    let (x, y, h) = ((m.w - tw) / 2.0, MARGIN_Y + 3.0, m.h - 2.0 * MARGIN_Y - 6.0);
    c.push_group();
    pal.bg.set(c);
    rounded(c, x - 3.0, y - 3.0, tw + 6.0, h + 6.0, h / 2.0 + 3.0);
    c.fill().unwrap();
    pal.surface.mix(pal.accent, 0.35).set(c);
    rounded(c, x, y, tw, h, h / 2.0);
    c.fill_preserve().unwrap();
    pal.accent.set(c);
    c.set_line_width(1.5);
    c.stroke().unwrap();
    pal.fg.set(c);
    p.text("\u{F036C}", 24.0, false, x + 26.0, m.h / 2.0, None);
    p.text(msg, 24.0, true, x + 40.0 + (tw - 52.0) / 2.0, m.h / 2.0, None);
    c.pop_group_to_source().unwrap();
    c.paint_with_alpha(a).unwrap();
}

/// Lyrics as a strip scrolling across in time with the song. The current
/// line colours in left to right as it's sung, the classic karaoke fill;
/// others are dimmed. Each line holds still while it's sung, then glides to
/// the next. In full-screen mode the current line also bounces to the beat.
#[allow(clippy::too_many_arguments)]
fn draw_lyrics(m: &Model, p: &Painter, pal: &Palette, x: f64, w: f64, cy: f64, px: f64, full: bool, now: Instant) {
    let c = p.c;
    let lines = m.lyrics();
    let Some((pos, len)) = m.position(now) else { return };
    let pos = pos + m.lyrics_offset(); // the sync adjuster's correction for this track
    if lines.is_empty() || w <= 0.0 {
        return;
    }
    let gap = if full { 90.0 } else { 60.0 };
    let widths: Vec<f64> = lines.iter().map(|(_, l)| lyric_width(m, p, l, px, full)).collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut acc = 0.0;
    for wd in &widths {
        starts.push(acc);
        acc += wd + gap;
    }
    // The line being sung, and how far through it we are.
    let cur = lines.iter().rposition(|(t, _)| *t <= pos);
    let (offset, progress) = match cur {
        None => (starts[0] - w * 0.55 * (1.0 - (pos / lines[0].0.max(0.1)).min(1.0)), 0.0),
        Some(i) => {
            let t0 = lines[i].0;
            let t1 = lines.get(i + 1).map(|l| l.0).unwrap_or(len.max(t0 + 4.0));
            let pr = ((pos - t0) / (t1 - t0).max(0.1)).clamp(0.0, 1.0);
            // Hold while it's sung, then glide to the next line.
            let glide = ((pr - 0.6) / 0.4).clamp(0.0, 1.0);
            let glide = glide * glide * (3.0 - 2.0 * glide);
            let next = starts.get(i + 1).copied().unwrap_or(starts[i] + widths[i] + gap);
            let fill = (pr / 0.85).min(1.0);
            // A line wider than the space: follow the word being sung.
            let follow = (widths[i] * fill + 24.0 - w * 0.72).clamp(0.0, (widths[i] + 24.0 - w).max(0.0));
            (starts[i] + follow.max((next - starts[i]) * glide), fill)
        }
    };
    let t = (now - m.epoch).as_secs_f64();
    let beat = if full { m.beat_level(now) } else { 0.0 };
    let lead = 24.0; // the current line starts a little in from the left
    c.save().unwrap();
    c.rectangle(x, cy - m.h / 2.0, w, m.h);
    c.clip();
    for (i, (_, line)) in lines.iter().enumerate() {
        let lx = x + lead + starts[i] - offset;
        if lx > x + w || lx + widths[i] < x {
            continue;
        }
        let is_cur = cur == Some(i);
        let dy = if is_cur { -beat * 3.0 } else { 0.0 };
        if full && m.text_style == 7 {
            // explode: each word dissolves into sparks shortly after it's sung
            let t0 = lines[i].0;
            let t1 = lines.get(i + 1).map(|l| l.0).unwrap_or(len.max(t0 + 4.0));
            draw_explode_line(m, pal, c, line, lx, cy + dy, t0, t1, pos, t, now);
            continue;
        }
        if is_cur {
            let lit = if full {
                viz_color(m, pal, t * 0.08, 0.85, 1.0).mix(Rgb(1.0, 1.0, 1.0), title_lift(m, 0.15))
            } else {
                pal.accent
            };
            // Unsung part, then the sung part filled over it.
            lyric_draw(m, p, line, lx, cy + dy, px, full, pal.fg, 0.55, t, true);
            c.save().unwrap();
            c.rectangle(lx - 4.0, cy - m.h / 2.0, widths[i] * progress + 4.0, m.h);
            c.clip();
            lyric_draw(m, p, line, lx, cy + dy, px, full, lit, 1.0, t, true);
            c.restore().unwrap();
        } else {
            let a = if cur.is_some_and(|k| i < k) { 0.35 } else { 0.55 };
            lyric_draw(m, p, line, lx, cy, px, full, pal.fg_dim, a, t, false);
        }
    }
    c.restore().unwrap();
    // Soft edges.
    for (x0, x1) in [(x, x + 30.0), (x + w, x + w - 30.0)] {
        let g = LinearGradient::new(x0, 0.0, x1, 0.0);
        g.add_color_stop_rgba(0.0, pal.bg.0, pal.bg.1, pal.bg.2, 0.9);
        g.add_color_stop_rgba(1.0, pal.bg.0, pal.bg.1, pal.bg.2, 0.0);
        c.set_source(&g).unwrap();
        c.rectangle(x0.min(x1), cy - m.h / 2.0, 30.0, m.h);
        c.fill().unwrap();
    }
}

thread_local! {
    /// Glyph advance widths, keyed by (character, size in quarter pixels).
    static CHAR_W: std::cell::RefCell<std::collections::HashMap<(char, u32), f64>> = std::cell::RefCell::new(Default::default());
}

fn char_width(p: &Painter, ch: char, px: f64) -> f64 {
    let key = (ch, (px * 4.0) as u32);
    if let Some(w) = CHAR_W.with(|m| m.borrow().get(&key).copied()) {
        return w;
    }
    let w = if ch == ' ' { px * 0.35 } else { p.text_width(&ch.to_string(), px, true) };
    CHAR_W.with(|m| m.borrow_mut().insert(key, w));
    w
}

/// Pixel size of the 5x7 lyric styles, or None for the font-based ones.
fn lyric_cell(style: u8) -> Option<f64> {
    match style {
        0 | 5..=7 => Some(6.0),
        3 => Some(5.0),
        _ => None,
    }
}

fn lyric_px(style: u8) -> f64 {
    if style == 2 { 46.0 } else { 34.0 }
}

/// Width of a lyric line in the chosen text style (full screen), or the
/// plain font (condensed bar).
fn lyric_width(m: &Model, p: &Painter, line: &str, px: f64, full: bool) -> f64 {
    if !full {
        return p.text_width(line, px, true);
    }
    match lyric_cell(m.text_style) {
        Some(cell) => line.chars().count() as f64 * glyph_advance(m.text_style) * cell,
        None if m.text_style == 2 => p.text_width(line, lyric_px(2), true),
        None => line.chars().map(|ch| char_width(p, ch, lyric_px(m.text_style))).sum(),
    }
}

/// One lyric line, left edge at `lx`, in the chosen text style. `lively`
/// lets letters move (the line being sung); the rest stay still.
#[allow(clippy::too_many_arguments)]
fn lyric_draw(m: &Model, p: &Painter, line: &str, lx: f64, cy: f64, px: f64, full: bool, col: Rgb, alpha: f64, t: f64, lively: bool) {
    let c = p.c;
    if !full {
        col.set_a(c, alpha);
        p.text(line, px, true, lx + p.text_width(line, px, true) / 2.0, cy, None);
        return;
    }
    let style = m.text_style;
    let motion = if lively { 1.0 } else { 0.0 };
    if let Some(cell) = lyric_cell(style) {
        let top = cy - 3.5 * cell;
        for (i, ch) in line.chars().enumerate() {
            let x = lx + i as f64 * glyph_advance(style) * cell;
            if style == 0 {
                // big rounded dots, the dot matrix
                let g = glyph_5x7(ch).unwrap_or_else(|| glyph_5x7('?').unwrap());
                c.set_source_rgba(col.0, col.1, col.2, alpha);
                let d = cell - 1.0;
                for (cx, byte) in g.iter().enumerate() {
                    let bob = (t * 4.0 + (i * 6 + cx) as f64 * 0.3).sin() * 1.5 * motion;
                    for row in 0..7 {
                        if byte >> row & 1 == 1 {
                            rounded(c, x + cx as f64 * cell, top + row as f64 * cell + bob, d, d, d * 0.3);
                        }
                    }
                }
                c.fill().unwrap();
            } else if style == 6 {
                // Sparkle while it's being sung; a quiet thick outline of itself otherwise.
                if lively && alpha >= 1.0 {
                    sparkle_letter(m, &Palette::from(&m.theme), c, ch, x, top, cell, i, t, Instant::now(), 1.0);
                } else {
                    thick_letter(c, ch, x, top, cell, col, alpha);
                }
            } else {
                pixel_letter(c, ch, x, top, cell, col, alpha);
            }
        }
        return;
    }
    let size = lyric_px(style);
    if style == 2 {
        // outline: one huge hollow line with a soft glow
        let l = p.layout(line, size, true);
        let (_, ext) = l.pixel_extents();
        c.move_to(lx, cy - ext.height() as f64 / 2.0);
        pangocairo::functions::layout_path(c, &l);
        c.set_source_rgba(col.0, col.1, col.2, 0.2 * alpha);
        c.set_line_width(6.0);
        c.stroke_preserve().unwrap();
        c.set_source_rgba(col.0, col.1, col.2, alpha);
        c.set_line_width(1.6);
        c.stroke().unwrap();
        return;
    }
    // wave and scatter: letter by letter
    let mut x = lx;
    for (i, ch) in line.chars().enumerate() {
        let cw = char_width(p, ch, size);
        if !ch.is_whitespace() {
            let fi = i as f64;
            let (dx, dy) = if style == 4 {
                ((t * 0.7 + fi * 1.3).sin() * 2.5 * motion, (t * 0.9 + fi * 0.7).cos() * 3.0 * motion)
            } else {
                (0.0, (t * 3.2 + fi * 0.55).sin() * 3.0 * motion)
            };
            col.set_a(c, alpha);
            p.text(&ch.to_string(), size, true, x + cw / 2.0 + dx, cy + dy, None);
        }
        x += cw;
    }
}

/// Columns per character on the pixel grid: 5x7 glyphs plus a gap, or the
/// sparkle style's thickened 6x7 plus a gap.
fn glyph_advance(style: u8) -> f64 {
    if style >= 6 { 7.0 } else { 6.0 }
}

/// A 5x7 glyph thickened to 6x7: every stroke two pixels wide.
fn thick_glyph(ch: char) -> [u8; 6] {
    let g = glyph_5x7(ch).unwrap_or_else(|| glyph_5x7('?').unwrap());
    let mut out = [0u8; 6];
    for i in 0..6 {
        out[i] = g.get(i).copied().unwrap_or(0) | if i > 0 { g[i - 1] } else { 0 };
    }
    out
}

fn thick_letter(c: &Context, ch: char, x: f64, top: f64, cell: f64, col: Rgb, alpha: f64) {
    c.set_source_rgba(col.0, col.1, col.2, alpha);
    for (cx, byte) in thick_glyph(ch).iter().enumerate() {
        for row in 0..7 {
            if byte >> row & 1 == 1 {
                c.rectangle(x + cx as f64 * cell, top + row as f64 * cell, cell - 1.0, cell - 1.0);
            }
        }
    }
    c.fill().unwrap();
}

/// Cheap, stable per-pixel noise in 0..1.
fn hash01(a: f64, b: f64) -> f64 {
    ((a * 12.9898 + b * 78.233).sin() * 43758.5453).rem_euclid(1.0)
}

/// Sparkle: a thick block letter where every pixel twinkles at its own
/// speed, cycles quickly through the mood's colours, now and then flashes
/// white like glitter, and lights up as each beat's wave flows through.
#[allow(clippy::too_many_arguments)]
fn sparkle_letter(m: &Model, pal: &Palette, c: &Context, ch: char, x: f64, top: f64, cell: f64, index: usize, t: f64, now: Instant, alpha: f64) {
    // The beat wave: a bright front running left to right from each hit.
    let since = (now - m.beat_at).as_secs_f64();
    let front = since * 900.0;
    let wave_on = m.flag("playing") && since < 2.5;
    let d = cell - 1.0;
    for (cx, byte) in thick_glyph(ch).iter().enumerate() {
        let col_i = (index * 7 + cx) as f64;
        for row in 0..7 {
            if byte >> row & 1 == 0 {
                continue;
            }
            let rf = row as f64;
            let h = hash01(col_i, rf);
            let px = x + cx as f64 * cell;
            let py = top + rf * cell;
            // Each pixel twinkles on its own clock.
            // Bright range only, so letters stay solid while they shimmer.
            let twinkle = 0.78 + 0.22 * (t * (5.0 + h * 9.0) + h * 20.0).sin();
            // How close the beat wave is (in screen x from the bar's left edge).
            let near = if wave_on { (1.0 - ((px - front).abs() / 60.0)).clamp(0.0, 1.0) * (1.0 - since / 2.5) } else { 0.0 };
            let hue = col_i * 0.013 + t * 0.35 + h * 0.18 + near * 0.25;
            let mut col = viz_color(m, pal, hue, 0.9, (twinkle + 0.4 * near).min(1.0));
            // Glitter: a few pixels flash white each moment.
            if hash01(col_i + (t * 12.0).floor(), rf * 3.1) > 0.965 {
                col = col.mix(Rgb(1.0, 1.0, 1.0), 0.85);
            }
            col = col.mix(Rgb(1.0, 1.0, 1.0), (0.45 * near).max(title_lift(m, 0.12)));
            let lift = 2.0 * near;
            c.set_source_rgba(col.0, col.1, col.2, alpha);
            c.rectangle(px, py - lift, d, d);
            c.fill().unwrap();
        }
    }
}

/// Volume over the still-playing visualiser: a wide translucent bar in the
/// mood's colours, the speaker on the left and the level on the right.
#[allow(clippy::too_many_arguments)]
fn draw_volume_over(m: &Model, p: &Painter, pal: &Palette, x: f64, w: f64, y: f64, h: f64, now: Instant) {
    let c = p.c;
    let vol = m.num("volume").unwrap_or(0.0).clamp(0.0, 1.0);
    let muted = m.flag("muted");
    let (tx, tw) = m.volume_track();
    let cy = y + h / 2.0;
    // Soften the visuals a touch so the control reads, without hiding them.
    pal.bg.set_a(c, 0.35);
    rounded(c, x, y, w, h, RADIUS);
    c.fill().unwrap();
    let t = (now - m.epoch).as_secs_f64();
    let lo = viz_color(m, pal, t * 0.05, 0.8, 1.0);
    let hi = viz_color(m, pal, t * 0.05 + 0.5, 0.8, 1.0);
    // Track, then the level in the mood's gradient.
    let th = 12.0;
    pal.bg.set_a(c, 0.6);
    rounded(c, tx, cy - th / 2.0, tw, th, th / 2.0);
    c.fill().unwrap();
    let g = LinearGradient::new(tx, 0.0, tx + tw, 0.0);
    g.add_color_stop_rgba(0.0, lo.0, lo.1, lo.2, 0.95);
    g.add_color_stop_rgba(1.0, hi.0, hi.1, hi.2, 0.95);
    c.set_source(&g).unwrap();
    if muted {
        pal.fg_dim.set_a(c, 0.8);
    }
    rounded(c, tx, cy - th / 2.0, (tw * vol).max(th), th, th / 2.0);
    c.fill().unwrap();
    // Knob with a soft halo.
    let kx = tx + tw * vol;
    let tip = lo.mix(hi, vol);
    let halo = cairo::RadialGradient::new(kx, cy, 0.0, kx, cy, 22.0);
    halo.add_color_stop_rgba(0.0, tip.0, tip.1, tip.2, 0.5);
    halo.add_color_stop_rgba(1.0, tip.0, tip.1, tip.2, 0.0);
    c.set_source(&halo).unwrap();
    c.arc(kx, cy, 22.0, 0.0, 2.0 * PI);
    c.fill().unwrap();
    c.set_source_rgb(1.0, 1.0, 1.0);
    c.arc(kx, cy, 11.0, 0.0, 2.0 * PI);
    c.fill().unwrap();
    // Speaker on the left, level on the right.
    let icon = if muted { "\u{F075F}" } else { "\u{F057E}" };
    pal.fg.set(c);
    p.text(icon, ICON_PX * 0.8, false, x + 36.0, cy, None);
    let label = if muted { "muted".to_string() } else { format!("{:.0}%", vol * 100.0) };
    p.text(&label, 26.0, true, x + w - 52.0, cy, None);
}

/// One letter bursting into sparks: every thick pixel flies outward along
/// its own direction, falls a little, shrinks and fades. `burst` 0..1.
#[allow(clippy::too_many_arguments)]
fn explode_letter(m: &Model, pal: &Palette, c: &Context, ch: char, x: f64, top: f64, cell: f64, index: usize, t: f64, burst: f64, alpha: f64) {
    let d = cell - 1.0;
    let fade = (1.0 - burst).powf(1.2) * alpha;
    if fade <= 0.01 {
        return;
    }
    for (cx, byte) in thick_glyph(ch).iter().enumerate() {
        let col_i = (index * 7 + cx) as f64;
        for row in 0..7 {
            if byte >> row & 1 == 0 {
                continue;
            }
            let rf = row as f64;
            let (h1, h2) = (hash01(col_i, rf), hash01(rf * 7.1, col_i * 3.3));
            let angle = h1 * 2.0 * PI;
            let speed = 30.0 + 110.0 * h2;
            let dx = angle.cos() * speed * burst;
            let dy = angle.sin() * speed * burst * 0.6 + 45.0 * burst * burst; // a little gravity
            let size = d * (1.0 - 0.75 * burst);
            let hue = col_i * 0.013 + t * 0.35 + h1 * 0.25;
            // A white flash as it breaks, then the mood's colours as it scatters.
            let flash = (burst * 4.0).min(1.0) * (1.0 - burst);
            let col = viz_color(m, pal, hue, 0.9, 1.0).mix(Rgb(1.0, 1.0, 1.0), 0.6 * flash);
            c.set_source_rgba(col.0, col.1, col.2, fade);
            c.rectangle(x + cx as f64 * cell + dx + (d - size) / 2.0, top + rf * cell + dy + (d - size) / 2.0, size, size);
            c.fill().unwrap();
        }
    }
}

/// A karaoke line in the explode style: letters wait as quiet outlines,
/// sparkle as they're sung, then burst into sparks and dissolve behind the wipe.
#[allow(clippy::too_many_arguments)]
fn draw_explode_line(m: &Model, pal: &Palette, c: &Context, line: &str, lx: f64, cy: f64, t0: f64, t1: f64, pos: f64, t: f64, now: Instant) {
    let cell = 6.0;
    let top = cy - 3.5 * cell;
    let n = line.chars().count().max(1);
    for (i, ch) in line.chars().enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        let x = lx + i as f64 * 7.0 * cell;
        // When this letter is sung: spread across the line's time.
        let sung_at = t0 + (i as f64 / n as f64) * (t1 - t0) * 0.85;
        if pos < sung_at {
            thick_letter(c, ch, x, top, cell, pal.fg_dim, 0.45);
            continue;
        }
        let burst = ((pos - sung_at - 0.7) / 1.6).clamp(0.0, 1.0);
        if burst <= 0.0 {
            sparkle_letter(m, pal, c, ch, x, top, cell, i, t, now, 1.0);
        } else {
            explode_letter(m, pal, c, ch, x, top, cell, i, t, burst, 1.0);
        }
    }
}

/// The style button: a magic wand in the mood's shifting colours, with
/// little sparkles twinkling around it.
fn draw_wand_button(m: &Model, p: &Painter, pal: &Palette, x: f64, w: f64, pl: f64, now: Instant) {
    let c = p.c;
    p.pill(x, w, pal.surface.mix(pal.accent, 0.45 * pl));
    let t = (now - m.epoch).as_secs_f64();
    let g = LinearGradient::new(x + 16.0, 0.0, x + w - 16.0, 0.0);
    for k in 0..=4 {
        let f = k as f64 / 4.0;
        let col = viz_color(m, pal, f * 0.8 + t * 0.05 + 0.3, 0.85, 1.0);
        g.add_color_stop_rgb(f, col.0, col.1, col.2);
    }
    c.set_source(&g).unwrap();
    p.text("\u{F0068}", ICON_PX * 0.9, false, x + w / 2.0, m.h / 2.0, None);
    c.new_path();
    // Three four-point sparkles, each on its own twinkle.
    for (k, (sx, sy, r)) in [(0.78, 0.26, 5.0), (0.22, 0.72, 3.5), (0.84, 0.74, 3.0)].iter().enumerate() {
        let tw = 0.5 + 0.5 * (t * (2.3 + k as f64 * 0.9) + k as f64 * 2.1).sin();
        let r = r * (0.6 + 0.4 * tw);
        let (cx, cy) = (x + w * sx, m.h * sy);
        let col = viz_color(m, pal, t * 0.07 + k as f64 * 0.3, 0.6, 1.0).mix(Rgb(1.0, 1.0, 1.0), 0.4);
        c.set_source_rgba(col.0, col.1, col.2, 0.35 + 0.65 * tw);
        c.move_to(cx, cy - r);
        c.line_to(cx + r * 0.28, cy - r * 0.28);
        c.line_to(cx + r, cy);
        c.line_to(cx + r * 0.28, cy + r * 0.28);
        c.line_to(cx, cy + r);
        c.line_to(cx - r * 0.28, cy + r * 0.28);
        c.line_to(cx - r, cy);
        c.line_to(cx - r * 0.28, cy - r * 0.28);
        c.close_path();
        c.fill().unwrap();
    }
}

/// The text-style button: a tiny live "Aa" in the current text style and mood.
fn draw_font_button(m: &Model, p: &Painter, pal: &Palette, x: f64, w: f64, pl: f64, now: Instant) {
    let c = p.c;
    p.pill(x, w, pal.surface.mix(pal.accent, 0.45 * pl));
    let t = (now - m.epoch).as_secs_f64();
    let cy = m.h / 2.0;
    let colour = |f: f64| viz_color(m, pal, f * 0.6 + t * 0.06, 0.85, 1.0).mix(Rgb(1.0, 1.0, 1.0), title_lift(m, 0.15));
    let style = m.text_style;
    if matches!(style, 0 | 3 | 5 | 6 | 7) {
        // Pixel styles: "Aa" on the dot grid.
        let cell = 3.6;
        let adv = glyph_advance(style) * cell;
        let (x0, top) = (x + (w - 2.0 * adv + cell) / 2.0, cy - 3.5 * cell);
        for (i, ch) in ['A', 'a'].into_iter().enumerate() {
            let lx = x0 + i as f64 * adv;
            let col = colour(i as f64 * 0.5);
            match style {
                0 => {
                    let g = glyph_5x7(ch).unwrap();
                    c.set_source_rgb(col.0, col.1, col.2);
                    for (cx, byte) in g.iter().enumerate() {
                        for row in 0..7 {
                            if byte >> row & 1 == 1 {
                                c.new_sub_path(); // separate dots, not joined by lines
                                c.arc(lx + cx as f64 * cell + 1.5, top + row as f64 * cell + 1.5, 1.3, 0.0, 2.0 * PI);
                            }
                        }
                    }
                    c.fill().unwrap();
                }
                6 => sparkle_letter(m, pal, c, ch, lx, top, cell, i, t, now, 1.0),
                7 => {
                    sparkle_letter(m, pal, c, ch, lx, top, cell, i, t, now, 1.0);
                    // A few sparks flying off, looping.
                    for k in 0..4 {
                        let ph = (t * 0.8 + k as f64 * 0.25 + i as f64 * 0.5).fract();
                        let ang = hash01(k as f64, i as f64) * 2.0 * PI;
                        let (sx, sy) = (lx + 9.0 + ang.cos() * 16.0 * ph, cy + ang.sin() * 12.0 * ph + 6.0 * ph * ph);
                        let col = colour(k as f64 * 0.25);
                        c.set_source_rgba(col.0, col.1, col.2, 1.0 - ph);
                        c.rectangle(sx, sy, 2.2 * (1.0 - ph) + 0.6, 2.2 * (1.0 - ph) + 0.6);
                        c.fill().unwrap();
                    }
                }
                _ => pixel_letter(c, ch, lx, top, cell, col, 1.0),
            }
        }
        return;
    }
    // Font styles: smooth letters, hollow for the outline style.
    let size = if style == 2 { 32.0 } else { 28.0 };
    let aw = p.text_width("A", size, true);
    let bw = p.text_width("a", size, true);
    let x0 = x + (w - aw - bw) / 2.0;
    for (i, (ch, cw, ox)) in [("A", aw, 0.0), ("a", bw, aw)].into_iter().enumerate() {
        let col = colour(i as f64 * 0.5);
        let dy = match style {
            1 => (t * 3.2 + i as f64 * 1.4).sin() * 2.0,
            4 => (t * 0.9 + i as f64 * 2.0).cos() * 2.0,
            _ => 0.0,
        };
        let cx = x0 + ox + cw / 2.0;
        if style == 2 {
            let l = p.layout(ch, size, true);
            let (_, ext) = l.pixel_extents();
            c.move_to(cx - ext.width() as f64 / 2.0, cy - ext.height() as f64 / 2.0);
            pangocairo::functions::layout_path(c, &l);
            c.set_source_rgb(col.0, col.1, col.2);
            c.set_line_width(1.3);
            c.stroke().unwrap();
        } else {
            col.set(c);
            p.text(ch, size, true, cx, cy + dy, None);
        }
    }
}

/// "No lyrics available", sweeping past once in big sparkly block letters
/// whatever the text style: glittering as they travel, bursting into sparks
/// as they reach the left edge.
#[allow(clippy::too_many_arguments)]
fn draw_no_lyrics(m: &Model, pal: &Palette, c: &Context, x: f64, w: f64, cy: f64, bt: f64, now: Instant) {
    let msg = "No lyrics available";
    let cell = 6.0;
    let adv = 7.0 * cell;
    let text_w = msg.chars().count() as f64 * adv;
    let t = (now - m.epoch).as_secs_f64();
    // Right edge to fully past the left edge over the banner's lifetime.
    let start = x + w - (bt / NO_LYRICS_SECS) * (w + text_w + 160.0);
    let top = cy - 3.5 * cell;
    c.save().unwrap();
    c.rectangle(x, cy - m.h / 2.0, w, m.h);
    c.clip();
    for (i, ch) in msg.chars().enumerate() {
        let lx = start + i as f64 * adv;
        if ch.is_whitespace() || lx > x + w || lx + adv < x - 160.0 {
            continue;
        }
        // Letters dissolve as they cross into the last stretch on the left.
        let burst = ((x + 120.0 - lx) / 160.0).clamp(0.0, 1.0);
        if burst > 0.0 {
            explode_letter(m, pal, c, ch, lx, top, cell, i, t, burst, 1.0);
        } else {
            sparkle_letter(m, pal, c, ch, lx, top, cell, i, t, now, 1.0);
        }
    }
    c.restore().unwrap();
}

/// The lyrics sync adjuster: speedometer, minus, offset, plus, reset, share.
fn draw_sync(m: &Model, p: &Painter, pal: &Palette, now: Instant) {
    let c = p.c;
    let off = m.lyrics_offset();
    let t = (now - m.epoch).as_secs_f64();
    let cy = m.h / 2.0;
    for (id, x, w) in m.sync_parts() {
        let pl = press_level(m, Hit::Sync(id), now);
        let fill = pal.surface.mix(pal.accent, 0.45 * pl);
        let two_line = |big: &str, small: &str, ink: Rgb| {
            ink.set(c);
            p.text(big, 26.0, true, x + w / 2.0, cy - 7.0, None);
            pal.fg_dim.set(c);
            p.text(small, 12.0, true, x + w / 2.0, cy + 15.0, None);
        };
        match id {
            0 => {
                // The speedometer, in the mood's colours.
                let g = LinearGradient::new(x, 0.0, x + w, 0.0);
                for k in 0..=2 {
                    let col = viz_color(m, pal, k as f64 * 0.35 + t * 0.05, 0.85, 1.0);
                    g.add_color_stop_rgb(k as f64 / 2.0, col.0, col.1, col.2);
                }
                c.set_source(&g).unwrap();
                p.text("\u{F04C5}", ICON_PX, false, x + w / 2.0, cy, None);
                c.new_path();
            }
            1 => {
                p.pill(x, w, fill);
                two_line("−", "later", pal.fg);
            }
            2 => {
                pal.bg.set_a(c, 0.55);
                rounded(c, x, MARGIN_Y, w, m.h - 2.0 * MARGIN_Y, RADIUS);
                c.fill().unwrap();
                let hint = if off.abs() < 0.05 { "in sync" } else if off > 0.0 { "lyrics earlier" } else { "lyrics later" };
                two_line(&format!("{:+.1} s", off), hint, if off.abs() < 0.05 { pal.fg } else { pal.accent });
            }
            3 => {
                p.pill(x, w, fill);
                two_line("+", "earlier", pal.fg);
            }
            4 => {
                p.pill(x, w, fill);
                p.content(Some("\u{F0450}"), None, x, w * 0.45, pal.fg);
                pal.fg.set(c);
                p.text("Reset", 16.0, true, x + w * 0.66, cy, None);
            }
            _ => {
                p.pill(x, w, fill);
                p.content(Some("\u{F0167}"), None, x + 4.0, w * 0.36, pal.fg);
                pal.fg.set(c);
                p.text("Share", 16.0, true, x + w * 0.7, cy, None);
            }
        }
    }
}
