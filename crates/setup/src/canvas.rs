//! A small software renderer. The setup draws every pixel itself: nothing to install,
//! and nothing that looks different from one PC to the next.

use fontdue::{Font, FontSettings};
use std::collections::HashMap;

pub type Color = u32;

pub const BG: Color = 0x0a0a0a;
pub const PANEL: Color = 0x121212;
pub const LINE: Color = 0x2a2a2a;
pub const MUTED: Color = 0x8c8c8c;
pub const INK: Color = 0xededed;
pub const DANGER: Color = 0xf07a7a;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Regular,
    Bold,
}

struct Glyph {
    width: i32,
    height: i32,
    left: i32,
    top: i32,
    advance: f32,
    alpha: Vec<u8>,
}

pub struct Fonts {
    regular: Font,
    bold: Font,
    /// For names our font does not cover. Loaded the first time one turns up.
    fallback: Option<Vec<Font>>,
    glyphs: HashMap<(char, u32, Face), Glyph>,
}

impl Fonts {
    pub fn new() -> Self {
        let load = |bytes: &[u8]| Font::from_bytes(bytes, FontSettings::default()).expect("built-in font");
        Fonts {
            regular: load(include_bytes!("../fonts/geist-mono-regular.ttf")),
            bold: load(include_bytes!("../fonts/geist-mono-bold.ttf")),
            fallback: None,
            glyphs: HashMap::new(),
        }
    }

    fn system_fonts() -> Vec<Font> {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        ["segoeui.ttf", "msyh.ttc", "msgothic.ttc", "malgun.ttf"]
            .iter()
            .filter_map(|name| std::fs::read(format!(r"{dir}\Fonts\{name}")).ok())
            .filter_map(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
            .collect()
    }

    fn glyph(&mut self, ch: char, px: f32, face: Face) -> &Glyph {
        let key = (ch, (px * 4.0) as u32, face);
        if !self.glyphs.contains_key(&key) {
            let main = if face == Face::Bold { &self.bold } else { &self.regular };
            let (metrics, alpha) = if main.lookup_glyph_index(ch) != 0 || ch.is_ascii() {
                main.rasterize(ch, px)
            } else {
                let fonts = self.fallback.get_or_insert_with(Self::system_fonts);
                match fonts.iter().find(|f| f.lookup_glyph_index(ch) != 0) {
                    Some(font) => font.rasterize(ch, px),
                    None => self.regular.rasterize('?', px),
                }
            };
            self.glyphs.insert(
                key,
                Glyph {
                    width: metrics.width as i32,
                    height: metrics.height as i32,
                    left: metrics.xmin,
                    top: metrics.ymin + metrics.height as i32,
                    advance: metrics.advance_width,
                    alpha,
                },
            );
        }
        &self.glyphs[&key]
    }
}

pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub px: Vec<u32>,
    /// Physical pixels per layout unit.
    pub scale: f32,
    clip: (i32, i32, i32, i32),
    pub fonts: Fonts,
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = (t.clamp(0.0, 1.0) * 256.0) as u32;
    let u = 256 - t;
    let rb = ((a & 0xff00ff) * u + (b & 0xff00ff) * t) >> 8;
    let g = ((a & 0x00ff00) * u + (b & 0x00ff00) * t) >> 8;
    (rb & 0xff00ff) | (g & 0x00ff00)
}

impl Canvas {
    pub fn new(width: i32, height: i32, scale: f32) -> Self {
        Canvas {
            width,
            height,
            px: vec![BG; (width * height) as usize],
            scale,
            clip: (0, 0, width, height),
            fonts: Fonts::new(),
        }
    }

    pub fn resize(&mut self, width: i32, height: i32, scale: f32) {
        self.width = width;
        self.height = height;
        self.scale = scale;
        self.px = vec![BG; (width * height) as usize];
        self.clip = (0, 0, width, height);
    }

    pub fn clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let s = self.scale;
        self.clip = (
            ((x * s) as i32).max(0),
            ((y * s) as i32).max(0),
            (((x + w) * s).ceil() as i32).min(self.width),
            (((y + h) * s).ceil() as i32).min(self.height),
        );
    }

    pub fn unclip(&mut self) {
        self.clip = (0, 0, self.width, self.height);
    }

    #[inline]
    fn blend(&mut self, x: i32, y: i32, color: Color, alpha: f32) {
        let (x0, y0, x1, y1) = self.clip;
        if x < x0 || y < y0 || x >= x1 || y >= y1 {
            return;
        }
        let i = (y * self.width + x) as usize;
        self.px[i] = mix(self.px[i], color, alpha);
    }

    /// Fills a rectangle given in physical pixels.
    pub fn fill_px(&mut self, x: i32, y: i32, w: i32, h: i32, color: Color, alpha: f32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let (x0, y0, x1, y1) = (x.max(cx0), y.max(cy0), (x + w).min(cx1), (y + h).min(cy1));
        if alpha >= 0.999 {
            for row in y0..y1 {
                let start = (row * self.width + x0) as usize;
                let end = (row * self.width + x1).max(start as i32) as usize;
                self.px[start..end].fill(color);
            }
        } else {
            for row in y0..y1 {
                for col in x0..x1 {
                    let i = (row * self.width + col) as usize;
                    self.px[i] = mix(self.px[i], color, alpha);
                }
            }
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color, alpha: f32) {
        let s = self.scale;
        let (px, py) = ((x * s).round() as i32, (y * s).round() as i32);
        let (qx, qy) = (((x + w) * s).round() as i32, ((y + h) * s).round() as i32);
        self.fill_px(px, py, (qx - px).max(1), (qy - py).max(1), color, alpha);
    }

    /// Antialiased rounded rectangle. With `stroke`, draws only its outline.
    pub fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color, alpha: f32, stroke: Option<f32>) {
        let s = self.scale;
        let (x, y, w, h, r) = (x * s, y * s, w * s, h * s, (radius * s).min(w * s / 2.0).min(h * s / 2.0));
        let stroke = stroke.map(|v| v * s);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        for py in (y.floor() as i32 - 1)..=((y + h).ceil() as i32) {
            for px in (x.floor() as i32 - 1)..=((x + w).ceil() as i32) {
                let qx = ((px as f32 + 0.5) - cx).abs() - (w / 2.0 - r);
                let qy = ((py as f32 + 0.5) - cy).abs() - (h / 2.0 - r);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = outside + qx.max(qy).min(0.0) - r;
                let mut coverage = (0.5 - d).clamp(0.0, 1.0);
                if let Some(width) = stroke {
                    coverage *= (d + width + 0.5).clamp(0.0, 1.0);
                }
                if coverage > 0.0 {
                    self.blend(px, py, color, coverage * alpha);
                }
            }
        }
    }

    pub fn text_width(&mut self, text: &str, size: f32, face: Face) -> f32 {
        let px = size * self.scale;
        let total: f32 = text.chars().map(|c| self.fonts.glyph(c, px, face).advance).sum();
        total / self.scale
    }

    /// Draws text with `y` as the top of a line of `size`.
    pub fn text(&mut self, x: f32, y: f32, text: &str, size: f32, face: Face, color: Color) {
        let px = size * self.scale;
        let baseline = ((y + size * 0.92) * self.scale).round() as i32;
        let mut pen = x * self.scale;
        for ch in text.chars() {
            let glyph = self.fonts.glyph(ch, px, face);
            let (gw, gh, left, top, advance) = (glyph.width, glyph.height, glyph.left, glyph.top, glyph.advance);
            let alpha = std::mem::take(&mut self.fonts.glyphs.get_mut(&(ch, (px * 4.0) as u32, face)).unwrap().alpha);
            let ox = pen.round() as i32 + left;
            let oy = baseline - top;
            for row in 0..gh {
                for col in 0..gw {
                    let a = alpha[(row * gw + col) as usize];
                    if a > 0 {
                        self.blend(ox + col, oy + row, color, a as f32 / 255.0);
                    }
                }
            }
            self.fonts.glyphs.get_mut(&(ch, (px * 4.0) as u32, face)).unwrap().alpha = alpha;
            pen += advance;
        }
    }

    /// Cuts `text` to fit, keeping the end when it is a path.
    pub fn fit(&mut self, text: &str, size: f32, face: Face, width: f32, keep_end: bool) -> String {
        if self.text_width(text, size, face) <= width {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut keep = chars.len();
        while keep > 1 {
            keep -= 1;
            let cut: String = if keep_end {
                std::iter::once('.').chain(std::iter::once('.')).chain(chars[chars.len() - keep..].iter().copied()).collect()
            } else {
                chars[..keep].iter().copied().chain(std::iter::once('.')).chain(std::iter::once('.')).collect()
            };
            if self.text_width(&cut, size, face) <= width {
                return cut;
            }
        }
        String::new()
    }

    /// Breaks text into lines that fit `width`.
    pub fn wrap(&mut self, text: &str, size: f32, face: Face, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.lines() {
            let mut line = String::new();
            for word in paragraph.split_whitespace() {
                let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if self.text_width(&trial, size, face) > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_string();
                } else {
                    line = trial;
                }
            }
            lines.push(line);
        }
        lines
    }

    /// Draws an RGBA image scaled to cover the box, centred. `dim` darkens it.
    pub fn cover(&mut self, image: &kryo::format::Image, x: f32, y: f32, w: f32, h: f32) {
        let s = self.scale;
        let (px, py, pw, ph) = ((x * s) as i32, (y * s) as i32, (w * s) as i32, (h * s) as i32);
        let (iw, ih) = (image.width as f32, image.height as f32);
        let zoom = (pw as f32 / iw).max(ph as f32 / ih);
        let (ox, oy) = ((iw - pw as f32 / zoom) / 2.0, (ih - ph as f32 / zoom) / 2.0);
        for row in 0..ph {
            let sy = ((oy + row as f32 / zoom) as u32).min(image.height - 1);
            for col in 0..pw {
                let sx = ((ox + col as f32 / zoom) as u32).min(image.width - 1);
                let i = ((sy * image.width + sx) * 4) as usize;
                let color = (image.rgba[i] as u32) << 16 | (image.rgba[i + 1] as u32) << 8 | image.rgba[i + 2] as u32;
                self.blend(px + col, py + row, color, 1.0);
            }
        }
    }
}
