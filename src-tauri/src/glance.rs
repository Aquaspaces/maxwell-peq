//! The tray icon as a quick glance: a battery gauge with the percentage in
//! the middle, the mic state at bottom left and the active EQ at bottom right.
//!
//! Everything is drawn at four times the icon size and averaged down, so the
//! arc and the small shapes stay smooth at 16 pixels.

/// What the icon shows. `None` draws an empty gauge or leaves a part out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glance {
    pub battery: Option<u8>,
    pub mic_muted: Option<bool>,
    /// Custom preset 1 to 4; a built-in preset shows no pill.
    pub slot: Option<u8>,
    /// The taskbar is light, so the icon is drawn in dark ink.
    pub light: bool,
}

/// At or below this the gauge and number turn red.
pub const LOW_BATTERY: u8 = 15;
const SCALE: usize = 4;

// Layout in units of the icon's side, from the top left.
const CENTRE: (f32, f32) = (0.5, 0.4);
const RADIUS: f32 = 0.36;
const STROKE: f32 = 0.1;
/// The gauge sweeps 240 degrees, open at the bottom for the mic and the pill.
const START: f32 = 150.0;
const SWEEP: f32 = 240.0;

type Rgb = [f32; 3];

fn rgb(hex: u32) -> Rgb {
    [(hex >> 16) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0]
}

/// Premultiplied RGBA at `SCALE` times the icon size.
struct Canvas {
    side: usize,
    pixels: Vec<[f32; 4]>,
}

impl Canvas {
    /// Paint `colour` at `alpha` wherever `shape` covers the sample point (in units).
    fn fill(&mut self, colour: Rgb, alpha: f32, shape: impl Fn(f32, f32) -> bool) {
        let side = self.side as f32;
        for y in 0..self.side {
            for x in 0..self.side {
                if shape((x as f32 + 0.5) / side, (y as f32 + 0.5) / side) {
                    self.blend(x, y, colour, alpha);
                }
            }
        }
    }

    fn blend(&mut self, x: usize, y: usize, colour: Rgb, alpha: f32) {
        let pixel = &mut self.pixels[y * self.side + x];
        for channel in 0..3 {
            pixel[channel] = colour[channel] * alpha + pixel[channel] * (1.0 - alpha);
        }
        pixel[3] = alpha + pixel[3] * (1.0 - alpha);
    }

    /// Draw `text` centred on `centre`, as large as fits `width` by `height` (units).
    fn text(&mut self, font: &fontdue::Font, text: &str, centre: (f32, f32), width: f32, height: f32, colour: Rgb) {
        let side = self.side as f32;
        let ink = |px: f32| {
            let (mut pen, mut left, mut right, mut top, mut bottom) = (0.0f32, f32::MAX, f32::MIN, i32::MIN, i32::MAX);
            for c in text.chars() {
                let metrics = font.metrics(c, px);
                left = left.min(pen + metrics.xmin as f32);
                right = right.max(pen + metrics.xmin as f32 + metrics.width as f32);
                top = top.max(metrics.ymin + metrics.height as i32);
                bottom = bottom.min(metrics.ymin);
                pen += metrics.advance_width;
            }
            (left, right, top, bottom)
        };
        let (left, right, top, bottom) = ink(side);
        let fit = (width * side / (right - left).max(1.0)).min(height * side / (top - bottom).max(1) as f32);
        let px = side * fit;
        let (left, right, top, bottom) = ink(px);
        let mut pen = centre.0 * side - (right - left) / 2.0 - left;
        let baseline = (centre.1 * side + (top - bottom) as f32 / 2.0).round() as i32 + bottom;
        for c in text.chars() {
            let (metrics, bitmap) = font.rasterize(c, px);
            let gx = (pen + metrics.xmin as f32).round() as i32;
            let gy = baseline - (metrics.ymin + metrics.height as i32);
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let (x, y) = (gx + col as i32, gy + row as i32);
                    let coverage = f32::from(bitmap[row * metrics.width + col]) / 255.0;
                    if coverage > 0.0 && (0..self.side as i32).contains(&x) && (0..self.side as i32).contains(&y) {
                        self.blend(x as usize, y as usize, colour, coverage);
                    }
                }
            }
            pen += metrics.advance_width;
        }
    }

    /// Average each `SCALE`×`SCALE` block into straight RGBA bytes.
    fn finish(self) -> Vec<u8> {
        let size = self.side / SCALE;
        let mut out = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            for x in 0..size {
                let mut sum = [0.0f32; 4];
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let pixel = self.pixels[(y * SCALE + dy) * self.side + x * SCALE + dx];
                        (0..4).for_each(|channel| sum[channel] += pixel[channel]);
                    }
                }
                let alpha = sum[3] / (SCALE * SCALE) as f32;
                for channel in 0..3 {
                    let straight = if sum[3] > 0.0 { sum[channel] / sum[3] } else { 0.0 };
                    out.push((straight * 255.0).round().clamp(0.0, 255.0) as u8);
                }
                out.push((alpha * 255.0).round() as u8);
            }
        }
        out
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// The point on the gauge's centre line `degrees` past its start.
fn on_arc(degrees: f32) -> (f32, f32) {
    let angle = (START + degrees).to_radians();
    (CENTRE.0 + RADIUS * angle.cos(), CENTRE.1 + RADIUS * angle.sin())
}

/// The gauge from its start to `degrees`, with round caps.
fn arc(degrees: f32) -> impl Fn(f32, f32) -> bool {
    let (start, end) = (on_arc(0.0), on_arc(degrees));
    move |x, y| {
        let along = ((y - CENTRE.1).atan2(x - CENTRE.0).to_degrees() - START).rem_euclid(360.0);
        let band = (distance((x, y), CENTRE) - RADIUS).abs() <= STROKE / 2.0;
        (band && along <= degrees) || distance((x, y), start) <= STROKE / 2.0 || distance((x, y), end) <= STROKE / 2.0
    }
}

/// The distance from `p` to the segment `a`–`b`.
fn to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    distance(p, (a.0 + t * dx, a.1 + t * dy))
}

/// A microphone in the bottom-left corner: capsule, cradle and stand.
fn mic(x: f32, y: f32) -> bool {
    let line = 0.05;
    let capsule = to_segment((x, y), (0.15, 0.75), (0.15, 0.81)) <= 0.05;
    let cradle_centre = (0.15, 0.81);
    let cradle = y >= cradle_centre.1 && (distance((x, y), cradle_centre) - 0.095).abs() <= line / 2.0;
    let stand = to_segment((x, y), (0.15, 0.905), (0.15, 0.97)) <= line / 2.0;
    capsule || cradle || stand
}

fn mic_slash(x: f32, y: f32) -> bool {
    to_segment((x, y), (0.03, 0.7), (0.28, 0.99)) <= 0.035
}

/// The EQ pill in the bottom-right corner.
const PILL: (f32, f32, f32, f32) = (0.52, 0.68, 1.0, 1.0);

fn pill(x: f32, y: f32) -> bool {
    let (left, top, right, bottom) = PILL;
    let radius = (bottom - top) / 2.0;
    let cy = (top + bottom) / 2.0;
    to_segment((x, y), (left + radius, cy), (right - radius, cy)) <= radius
}

/// Draw the icon as `size`×`size` straight RGBA.
pub fn draw(font: &fontdue::Font, glance: Glance, size: u32) -> Vec<u8> {
    let side = size as usize * SCALE;
    let mut canvas = Canvas { side, pixels: vec![[0.0; 4]; side * side] };
    let ink = if glance.light { rgb(0x000000) } else { rgb(0xffffff) };
    let low = glance.battery.is_some_and(|battery| battery <= LOW_BATTERY);
    let red = if glance.light { rgb(0xc42b1c) } else { rgb(0xff99a4) };
    let level = if low { red } else { ink };

    canvas.fill(ink, 0.3, arc(SWEEP));
    if let Some(battery) = glance.battery {
        canvas.fill(level, 1.0, arc(SWEEP * f32::from(battery.min(100)) / 100.0));
    }
    match glance.battery {
        Some(battery) => canvas.text(font, &battery.min(100).to_string(), CENTRE, 0.48, 0.3, level),
        None => canvas.fill(ink, 1.0, |x, y| to_segment((x, y), (0.4, CENTRE.1), (0.6, CENTRE.1)) <= 0.035),
    }

    match glance.mic_muted {
        Some(true) => {
            canvas.fill(red, 1.0, mic);
            canvas.fill(red, 1.0, mic_slash);
        }
        Some(false) => canvas.fill(ink, 1.0, mic),
        None => {}
    }

    if let Some(slot) = glance.slot.filter(|slot| (1..=4).contains(slot)) {
        let (green, digit) = if glance.light { (rgb(0x0f7b0f), rgb(0xffffff)) } else { (rgb(0x6ccb5f), rgb(0x000000)) };
        canvas.fill(green, 1.0, pill);
        let (left, top, right, bottom) = PILL;
        canvas.text(font, &slot.to_string(), ((left + right) / 2.0, (top + bottom) / 2.0), 0.3, 0.21, digit);
    }
    canvas.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Option<fontdue::Font> {
        crate::system::font_paths()
            .iter()
            .filter_map(|path| std::fs::read(path).ok())
            .find_map(|bytes| fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok())
    }

    fn alpha_at(rgba: &[u8], size: u32, x: f32, y: f32) -> u8 {
        let (px, py) = ((x * size as f32) as u32, (y * size as f32) as u32);
        rgba[((py * size + px) * 4 + 3) as usize]
    }

    #[test]
    fn the_icon_shows_each_part_only_when_known() {
        let Some(font) = font() else { return };
        let size = 32;
        let full = Glance { battery: Some(100), mic_muted: Some(false), slot: Some(2), light: false };
        let rgba = draw(&font, full, size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        assert!(alpha_at(&rgba, size, 0.8, 0.85) > 200, "the pill");
        assert!(alpha_at(&rgba, size, 0.15, 0.75) > 100, "the mic");
        let empty = draw(&font, Glance { battery: None, mic_muted: None, slot: None, light: false }, size);
        assert_eq!(alpha_at(&empty, size, 0.8, 0.85), 0);
        assert_eq!(alpha_at(&empty, size, 0.15, 0.75), 0);
        // A built-in preset has no pill.
        let built_in = draw(&font, Glance { slot: Some(7), ..full }, size);
        assert_eq!(alpha_at(&built_in, size, 0.8, 0.85), 0);
    }

    /// `MAXWELL_PEQ_ICON_PREVIEW=<folder> cargo test -p maxwell-peq glance` writes PNGs to look at.
    #[test]
    fn preview() {
        let (Some(folder), Some(font)) = (std::env::var_os("MAXWELL_PEQ_ICON_PREVIEW"), font()) else { return };
        let folder = std::path::PathBuf::from(folder);
        std::fs::create_dir_all(&folder).unwrap();
        let cases = [
            ("dark-50-muted-eq2", Glance { battery: Some(50), mic_muted: Some(true), slot: Some(2), light: false }),
            ("dark-100-live-eq4", Glance { battery: Some(100), mic_muted: Some(false), slot: Some(4), light: false }),
            ("dark-9-live-eq1", Glance { battery: Some(9), mic_muted: Some(false), slot: Some(1), light: false }),
            ("dark-unknown", Glance { battery: None, mic_muted: None, slot: None, light: false }),
            ("light-72-muted-eq3", Glance { battery: Some(72), mic_muted: Some(true), slot: Some(3), light: true }),
        ];
        for (name, glance) in cases {
            for size in [16, 20, 24, 32, 64] {
                let rgba = draw(&font, glance, size);
                let file = std::fs::File::create(folder.join(format!("{name}-{size}.png"))).unwrap();
                let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size, size);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
            }
        }
    }
}
