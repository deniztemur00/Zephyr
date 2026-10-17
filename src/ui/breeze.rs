use std::f32::consts::TAU;

use iced::widget::image::Handle;
use image::{ImageFormat, Rgba, RgbaImage};

const FRAMES: usize = 4;
const AMPLITUDE: f32 = 5.0;
const WAVELENGTH: f32 = 150.0;

// Frame 0 is the drawing itself; the others sway the strokes along a wave travelling
// downwind, measured against the drawing so every loop returns to it exactly.
pub fn frames(png: &[u8]) -> Vec<Handle> {
    let Ok(logo) = image::load_from_memory_with_format(png, ImageFormat::Png) else {
        return Vec::new();
    };
    let logo = with_margin(&logo.into_rgba8());
    (0..FRAMES)
        .map(|frame| {
            let swayed = sway(&logo, TAU * frame as f32 / FRAMES as f32);
            Handle::from_rgba(swayed.width(), swayed.height(), swayed.into_raw())
        })
        .collect()
}

// Room for the largest sway (2 × AMPLITUDE) so strokes near the edges are never cut off.
fn with_margin(logo: &RgbaImage) -> RgbaImage {
    let margin = (2.0 * AMPLITUDE).ceil() as u32;
    let mut canvas = RgbaImage::new(logo.width(), logo.height() + 2 * margin);
    image::imageops::overlay(&mut canvas, logo, 0, i64::from(margin));
    canvas
}

fn sway(logo: &RgbaImage, phase: f32) -> RgbaImage {
    let width = logo.width() as f32;
    RgbaImage::from_fn(logo.width(), logo.height(), |x, y| {
        let wave = TAU * x as f32 / WAVELENGTH;
        let downwind = 0.35 + 0.65 * x as f32 / width;
        let shift = AMPLITUDE * downwind * ((wave - phase).sin() - wave.sin());
        sample_column(logo, x, y as f32 - shift)
    })
}

fn sample_column(logo: &RgbaImage, x: u32, y: f32) -> Rgba<u8> {
    let pixel = |row: f32| {
        if row >= 0.0 && row < logo.height() as f32 {
            logo.get_pixel(x, row as u32).0
        } else {
            [0; 4]
        }
    };
    let (above, below, weight) = (pixel(y.floor()), pixel(y.floor() + 1.0), y - y.floor());
    Rgba(std::array::from_fn(|channel| {
        (f32::from(above[channel]) * (1.0 - weight) + f32::from(below[channel]) * weight).round()
            as u8
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripes() -> RgbaImage {
        RgbaImage::from_fn(300, 40, |_, y| {
            Rgba([239, 230, 207, if y % 8 < 3 { 255 } else { 0 }])
        })
    }

    fn alpha_mass(image: &RgbaImage) -> u64 {
        image.pixels().map(|p| u64::from(p.0[3])).sum()
    }

    #[test]
    fn first_frame_is_the_drawing_and_the_others_sway() {
        let logo = with_margin(&stripes());
        assert_eq!(sway(&logo, 0.0), logo);
        for frame in 1..FRAMES {
            let swayed = sway(&logo, TAU * frame as f32 / FRAMES as f32);
            assert_ne!(swayed, logo);
            let (before, after) = (alpha_mass(&logo) as f64, alpha_mass(&swayed) as f64);
            assert!((after / before - 1.0).abs() < 0.05, "{before} vs {after}");
        }
    }

    #[test]
    fn decodes_four_frames_from_the_embedded_logo() {
        assert_eq!(frames(super::super::BREEZE_PNG).len(), FRAMES);
        assert!(frames(b"not a png").is_empty());
    }
}
