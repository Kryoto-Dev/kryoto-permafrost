//! The art panel: a game frozen in ASCII. Before the install the cover is locked in cells
//! of ice; as the game unpacks, the ice gives way from the bottom up. The cells are the
//! same block-and-double-line lettering as the rest of Kryoto, drawn as exact rectangles.

use crate::canvas::*;
use crate::mark::{MARK, MARK_H, MARK_W};
use kryo::format::Image;

/// One character cell, in layout units. The lettering's 3:5.
pub const CELL_W: f32 = 6.0;
pub const CELL_H: f32 = 10.0;

fn rand(a: f32, b: f32) -> f32 {
    let n = (a * 127.1 + b * 311.7 + 7.3).sin() * 43758.547;
    n - n.floor()
}

/// Draws the K// at `width` wide, each pixel shaded by how much of it the artwork covers.
/// `fill` picks the colour per pixel row, so the mark can sit half in the ice.
pub fn mark(c: &mut Canvas, x: f32, y: f32, width: f32, alpha: f32, fill: impl Fn(f32) -> Color) {
    let scale = c.scale;
    let k = width / MARK_W * scale;
    let (left, top) = (x * scale, y * scale);
    let (w, h) = ((width * scale).ceil() as usize + 2, (width * MARK_H / MARK_W * scale).ceil() as usize + 2);
    let mut coverage = vec![0f32; w * h];
    let (fx, fy) = (left - left.floor(), top - top.floor());
    for &(rx, ry, rw, rh) in MARK {
        let (x0, y0) = (fx + rx as f32 * k, fy + ry as f32 * k);
        let (x1, y1) = (x0 + rw as f32 * k, y0 + rh as f32 * k);
        for row in (y0.floor() as usize)..(y1.ceil() as usize).min(h) {
            let dy = (y1.min(row as f32 + 1.0) - y0.max(row as f32)).max(0.0);
            for col in (x0.floor() as usize)..(x1.ceil() as usize).min(w) {
                let dx = (x1.min(col as f32 + 1.0) - x0.max(col as f32)).max(0.0);
                let cell = &mut coverage[row * w + col];
                *cell = (*cell + dx * dy).min(1.0);
            }
        }
    }
    for row in 0..h {
        let color = fill((top.floor() + row as f32) / scale);
        for col in 0..w {
            let a = coverage[row * w + col];
            if a > 0.0 {
                c.fill_px(left.floor() as i32 + col as i32, top.floor() as i32 + row as i32, 1, 1, color, a * alpha);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Shade {
    Dot,
    Light,
    Medium,
    Dark,
    Full,
    Top,
    Bottom,
}

fn cell(c: &mut Canvas, x: f32, y: f32, shade: Shade, color: Color, alpha: f32) {
    match shade {
        Shade::Dot => c.rect(x + 2.5, y + 4.5, 1.0, 1.0, color, alpha),
        Shade::Light => c.rect(x, y, CELL_W, CELL_H, color, alpha * 0.2),
        Shade::Medium => c.rect(x, y, CELL_W, CELL_H, color, alpha * 0.45),
        Shade::Dark => c.rect(x, y, CELL_W, CELL_H, color, alpha * 0.72),
        Shade::Full => c.rect(x, y, CELL_W, CELL_H, color, alpha),
        Shade::Top => c.rect(x, y, CELL_W, CELL_H / 2.0, color, alpha),
        Shade::Bottom => c.rect(x, y + CELL_H / 2.0, CELL_W, CELL_H / 2.0, color, alpha),
    }
}

/// Average colour and brightness of each cell of the cover, worked out once.
pub struct Frozen {
    cols: usize,
    rows: usize,
    cells: Vec<(Color, f32)>,
}

impl Frozen {
    pub fn new(image: Option<&Image>, w: f32, h: f32) -> Self {
        let cols = (w / CELL_W).ceil() as usize;
        let rows = (h / CELL_H).ceil() as usize;
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let (u, v) = ((col as f32 + 0.5) / cols as f32, (row as f32 + 0.5) / rows as f32);
                cells.push(match image {
                    Some(image) => sample(image, u, v, w / h),
                    None => empty_cell(u, v),
                });
            }
        }
        Frozen { cols, rows, cells }
    }
}

fn sample(image: &Image, u: f32, v: f32, aspect: f32) -> (Color, f32) {
    let (iw, ih) = (image.width as f32, image.height as f32);
    let (cw, ch) = if iw / ih > aspect { (ih * aspect, ih) } else { (iw, iw / aspect) };
    let x = (((iw - cw) / 2.0 + u * cw) as u32).min(image.width - 1);
    let y = (((ih - ch) / 2.0 + v * ch) as u32).min(image.height - 1);
    let i = ((y * image.width + x) * 4) as usize;
    let (r, g, b) = (image.rgba[i] as u32, image.rgba[i + 1] as u32, image.rgba[i + 2] as u32);
    let luma = (r as f32 * 0.3 + g as f32 * 0.59 + b as f32 * 0.11) / 255.0;
    (r << 16 | g << 8 | b, luma)
}

/// With no cover to freeze there is only ice, and the K// is drawn over it.
fn empty_cell(u: f32, v: f32) -> (Color, f32) {
    (0x20262c, 0.05 + 0.1 * rand(u * 41.0, v * 57.0))
}

fn shade_for(luma: f32) -> Shade {
    match luma {
        l if l < 0.08 => Shade::Dot,
        l if l < 0.26 => Shade::Light,
        l if l < 0.5 => Shade::Medium,
        l if l < 0.74 => Shade::Dark,
        _ => Shade::Full,
    }
}

/// `thaw` runs from 0, all ice, to 1, all cover.
pub fn panel(c: &mut Canvas, frozen: &Frozen, image: Option<&Image>, x: f32, y: f32, w: f32, h: f32, thaw: f32, time: f32, accent: Color) {
    c.clip(x, y, w, h);
    c.rect(x, y, w, h, 0x0c0e10, 1.0);
    if let Some(image) = image {
        if thaw > 0.0 {
            c.cover(image, x, y, w, h);
        }
    }

    // The melt line climbs from below the panel to above it, ragged and never quite still.
    let line = (1.0 - thaw) * (frozen.rows as f32 + 8.0) - 4.0;
    let sweep = (time * 0.11).fract() * 1.8 - 0.4;
    for row in 0..frozen.rows {
        for col in 0..frozen.cols {
            let (color, luma) = frozen.cells[row * frozen.cols + col];
            let (cf, rf) = (col as f32, row as f32);
            let edge = line + 3.0 * (cf * 0.21 + time * 0.8).sin() + 2.0 * rand(cf, 3.0);
            let depth = edge - rf;
            if depth < -0.5 {
                continue;
            }
            let (px, py) = (x + cf * CELL_W, y + rf * CELL_H);
            // Ice is the cover pulled towards the accent, as if seen through it.
            let tint = mix(color, accent, 0.35);
            let base = mix(0x0c0e10, tint, 0.55 + luma * 0.45);
            c.rect(px, py, CELL_W, CELL_H, 0x0c0e10, 1.0);

            // The shimmer that runs across the site's lettering.
            let diagonal = (cf / frozen.cols as f32 + rf / frozen.rows as f32) / 2.0;
            let glint = (1.0 - ((diagonal - sweep).abs() / 0.06)).clamp(0.0, 1.0);

            if depth < 2.5 {
                // At the line the cells churn, the way the site's text scrambles and settles.
                let churn = rand(cf + (time * 9.0).floor(), rf);
                let shade = [Shade::Top, Shade::Bottom, Shade::Light, Shade::Medium, Shade::Dot][(churn * 5.0) as usize % 5];
                cell(c, px, py, shade, mix(base, accent, 0.6), 1.0);
                continue;
            }
            cell(c, px, py, shade_for(luma), mix(base, INK, glint * 0.5), 1.0);
        }
    }
    if image.is_none() {
        // The real mark: tinted while frozen, plain ink once the ice has left it.
        let width = w * 0.74;
        let my = y + (h - width * MARK_H / MARK_W) / 2.0;
        let melt_y = y + line * CELL_H;
        mark(c, x + (w - width) / 2.0, my, width, 1.0, |row| if row > melt_y { INK } else { mix(INK, accent, 0.65) });
    }
    c.unclip();
}

/// A bar in the same cells: `████▓▒░·····`.
pub fn bar(c: &mut Canvas, x: f32, y: f32, w: f32, value: f32, time: f32, accent: Color) {
    let cells = (w / CELL_W) as i32;
    let filled = value.clamp(0.0, 1.0) * cells as f32;
    for i in 0..cells {
        let px = x + i as f32 * CELL_W;
        let f = i as f32;
        if f + 1.0 <= filled {
            let pulse = ((f * 0.35 - time * 3.0).sin() * 0.5 + 0.5) * 0.25;
            cell(c, px, y, Shade::Full, mix(accent, INK, pulse), 1.0);
        } else if f < filled + 3.0 {
            let shade = match (f - filled) as i32 {
                i32::MIN..=0 => Shade::Dark,
                1 => Shade::Medium,
                _ => Shade::Light,
            };
            cell(c, px, y, shade, accent, 1.0);
        } else {
            cell(c, px, y, Shade::Dot, LINE, 1.0);
        }
    }
}
