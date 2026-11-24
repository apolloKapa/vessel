//! Pictures in memory: decoding (PNG/JPEG/WebP/BMP/ICO/SVG), covers and icons on disk, and the
//! pictures the views show (cropped covers, gradients with initials, blurred NSFW covers).

use crate::store;
use crate::util::{self, join};
use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};

pub static ICON_SVG: &[u8] = include_bytes!("../vessel.svg");

pub fn render_svg(data: &[u8], width: u32) -> Option<RgbaImage> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = width as f32 / size.width().max(1.0);
    let w = width.max(1);
    let h = ((size.height() * scale).round() as u32).max(1);
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    // tiny-skia keeps premultiplied alpha
    let mut img = RgbaImage::from_raw(w, h, pm.take())?;
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(img)
}

fn looks_svg(data: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&data[..data.len().min(1024)]).to_lowercase();
    head.contains("<svg")
}

pub fn decode(data: &[u8]) -> Option<RgbaImage> {
    if looks_svg(data) {
        return render_svg(data, 600);
    }
    image::load_from_memory(data).ok().map(|i| i.into_rgba8())
}

pub fn load(path: &str) -> Option<RgbaImage> {
    if path.is_empty() {
        return None;
    }
    decode(&std::fs::read(path).ok()?)
}

/// keeps a cover for an entry: at most 900 wide, as PNG; its file name
pub fn save_cover(eid: &str, data: &[u8]) -> Result<String, String> {
    let mut img = decode(data).ok_or("not an image")?;
    if img.width() > 900 {
        let h = (img.height() as f64 * 900.0 / img.width() as f64).round().max(1.0) as u32;
        img = imageops::resize(&img, 900, h, FilterType::Lanczos3);
    }
    std::fs::create_dir_all(store::covers_dir()).map_err(|e| e.to_string())?;
    let name = format!("{eid}-{}.png", (util::now() * 1000.0) as i64);
    img.save(join(&store::covers_dir(), &name)).map_err(|e| e.to_string())?;
    Ok(name)
}

fn save_icon_image(eid: &str, img: &RgbaImage) -> String {
    if std::fs::create_dir_all(store::icons_dir()).is_err() {
        return String::new();
    }
    let name = format!("{eid}-{}.png", util::now_secs());
    match img.save(join(&store::icons_dir(), &name)) {
        Ok(()) => name,
        Err(_) => String::new(),
    }
}

/// the .exe's own icon, kept for the entry ("" when it has none)
pub fn save_exe_icon(eid: &str, exe: &str) -> String {
    match crate::files::exe_icon(exe).and_then(|d| decode(&d)) {
        Some(img) => save_icon_image(eid, &img),
        None => String::new(),
    }
}

/// an image file as the entry's icon, fitted into 256 × 256
pub fn save_icon_file(eid: &str, path: &str) -> String {
    match load(path) {
        Some(img) => save_icon_image(eid, &fit(&img, 256, 256, false)),
        None => String::new(),
    }
}

/// scaled to fit inside w × h (keeping its shape); `pad` centres it on a transparent w × h
pub fn fit(img: &RgbaImage, w: u32, h: u32, pad: bool) -> RgbaImage {
    let s = (w as f64 / img.width() as f64).min(h as f64 / img.height() as f64);
    let nw = ((img.width() as f64 * s).round() as u32).max(1);
    let nh = ((img.height() as f64 * s).round() as u32).max(1);
    let scaled = imageops::resize(img, nw, nh, FilterType::CatmullRom);
    if !pad {
        return scaled;
    }
    let mut out = RgbaImage::new(w, h);
    imageops::overlay(&mut out, &scaled, ((w - nw.min(w)) / 2) as i64, ((h - nh.min(h)) / 2) as i64);
    out
}

/// scaled to cover w × h and cropped in the middle
pub fn cover(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let s = (w as f64 / img.width() as f64).max(h as f64 / img.height() as f64);
    let nw = ((img.width() as f64 * s).round() as u32).max(w);
    let nh = ((img.height() as f64 * s).round() as u32).max(h);
    let scaled = imageops::resize(img, nw, nh, FilterType::CatmullRom);
    imageops::crop_imm(&scaled, (nw - w) / 2, (nh - h) / 2, w, h).to_image()
}

/// HSL (0-359, 0-255, 0-255, like Qt's QColor.fromHsl) to RGB
pub fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let (s, l) = (s / 255.0, l / 255.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8]
}

/// the gradient of an entry without a cover: its hue comes from its name
pub fn gradient(name: &str, w: u32, h: u32) -> RgbaImage {
    let hue = (name.chars().map(|c| c as u32).sum::<u32>() % 360) as f32;
    let a = hsl(hue, 120.0, 92.0);
    let b = hsl((hue + 45.0) % 360.0, 130.0, 40.0);
    // along the diagonal, like QLinearGradient(0, 0, w, h)
    let (fw, fh) = (w as f32, h as f32);
    let len2 = fw * fw + fh * fh;
    RgbaImage::from_fn(w, h, |x, y| {
        let t = ((x as f32 * fw + y as f32 * fh) / len2).clamp(0.0, 1.0);
        let mix = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
        Rgba([mix(0), mix(1), mix(2), 255])
    })
}

/// what the picture of an entry needs drawn on top of it (text the GPU draws sharper)
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Overlay {
    None,
    /// the gradient alone: its initials go in the middle
    Initials,
    /// the gradient with its icon: the name goes under the icon (portrait only)
    NameUnder { icon_bottom: f32 },
    /// blurred: "NSFW" in the middle
    Nsfw,
}

/// the gradient with the entry's icon (or space for its initials)
pub fn placeholder(name: &str, w: u32, h: u32, icon: Option<&RgbaImage>) -> (RgbaImage, Overlay) {
    let mut out = gradient(name, w, h);
    let Some(icon) = icon else { return (out, Overlay::Initials) };
    let portrait = h as f32 > w as f32 * 1.2;
    let side = (w.min(h) as f32 * if portrait { 0.55 } else { 0.7 }) as u32;
    let ic = fit(icon, side.max(1), side.max(1), false);
    let y = (h as i64 - ic.height() as i64) / 2 - if portrait { (h as f32 * 0.06) as i64 } else { 0 };
    imageops::overlay(&mut out, &ic, (w as i64 - ic.width() as i64) / 2, y);
    if portrait {
        (out, Overlay::NameUnder { icon_bottom: (y + ic.height() as i64) as f32 / h as f32 })
    } else {
        (out, Overlay::None)
    }
}

/// soft and darker, for NSFW covers until you point at them
pub fn blurred(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let small = imageops::resize(img, (w / 14).max(1), (h / 14).max(1), FilterType::Triangle);
    let mut big = imageops::resize(&small, w, h, FilterType::Triangle);
    for p in big.pixels_mut() {
        for c in 0..3 {
            p[c] = (p[c] as u32 * 185 / 255) as u8;
        }
    }
    big
}

/// the window/tray icon, w pixels wide
pub fn app_icon(w: u32) -> RgbaImage {
    render_svg(ICON_SVG, w).unwrap_or_else(|| gradient("Vessel", w, w))
}
