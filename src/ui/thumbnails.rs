use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use iced::Rectangle;
use iced::widget::image::Handle;
use image::imageops::FilterType;

const MAX_IN_FLIGHT: usize = 8;
const MAX_FAILED: usize = 512;
const MAX_DOWNLOAD_BYTES: usize = 512 * 1024;
const MAX_SOURCE_SIDE: u32 = 4096;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
const MEGABYTE: usize = 1024 * 1024;
const ALLOWED_HOSTS: [&str; 3] = [".ytimg.com", ".googleusercontent.com", ".ggpht.com"];

// Thumbnails are stored pre-cropped at this size: iced's CPU renderer ignores an image's own
// clip bounds, so cropping must happen on the pixels, not at draw time.
const SIDE: u32 = 80;
const PIXEL_BYTES: usize = (SIDE * SIDE * 4) as usize;

struct Entry {
    handle: Handle,
    used: u64,
}

pub struct Thumbnails {
    entries: HashMap<String, Entry>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    budget: usize,
    clock: u64,
}

impl Thumbnails {
    pub fn new(budget_mb: u32) -> Self {
        Self {
            entries: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            budget: budget_mb as usize * MEGABYTE,
            clock: 0,
        }
    }

    pub fn set_budget(&mut self, budget_mb: u32) {
        self.budget = budget_mb as usize * MEGABYTE;
        self.evict();
    }

    pub fn get(&self, url: &str) -> Option<&Handle> {
        self.entries.get(url).map(|entry| &entry.handle)
    }

    pub fn wanted<'a>(&mut self, visible: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        self.clock += 1;
        let mut wanted: Vec<String> = Vec::new();
        for url in visible {
            if let Some(entry) = self.entries.get_mut(url) {
                entry.used = self.clock;
                continue;
            }
            let busy = self.pending.len() + wanted.len() >= MAX_IN_FLIGHT;
            let skip = self.pending.contains(url)
                || self.failed.contains(url)
                || wanted.iter().any(|queued| queued == url)
                || !is_allowed(url);
            if !busy && !skip {
                wanted.push(url.to_owned());
            }
        }
        self.pending.extend(wanted.iter().cloned());
        wanted
    }

    pub fn finish(&mut self, url: String, result: Result<Vec<u8>, String>) {
        self.pending.remove(&url);
        match result.and_then(check_pixels) {
            Ok(pixels) => {
                let entry = Entry {
                    handle: Handle::from_rgba(SIDE, SIDE, pixels),
                    used: self.clock,
                };
                self.entries.insert(url, entry);
                self.evict();
            }
            Err(_) => {
                if self.failed.len() >= MAX_FAILED {
                    self.failed.clear();
                }
                self.failed.insert(url);
            }
        }
    }

    fn bytes(&self) -> usize {
        self.entries.len() * PIXEL_BYTES
    }

    // Never evicts what is on screen right now, or a too-small budget would refetch forever.
    fn evict(&mut self) {
        while self.bytes() > self.budget {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.used < self.clock)
                .min_by_key(|(_, entry)| entry.used)
                .map(|(url, _)| url.clone());
            let Some(url) = oldest else {
                break;
            };
            self.entries.remove(&url);
        }
    }
}

fn check_pixels(pixels: Vec<u8>) -> Result<Vec<u8>, String> {
    if pixels.len() == PIXEL_BYTES {
        Ok(pixels)
    } else {
        Err(format!("unexpected thumbnail of {} bytes", pixels.len()))
    }
}

pub async fn download(http: reqwest::Client, url: String) -> Result<Vec<u8>, String> {
    let mut response = http
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| err.to_string())?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        append_limited(&mut body, &chunk)?;
    }
    square_thumbnail(&body)
}

fn append_limited(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if body.len() + chunk.len() > MAX_DOWNLOAD_BYTES {
        return Err("thumbnail too large".into());
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn square_thumbnail(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    reader.limits(limits);
    let decoded = reader.decode().map_err(|err| err.to_string())?;
    let square = square_region(decoded.width(), decoded.height());
    let cropped = decoded
        .crop_imm(square.x, square.y, square.width, square.height)
        .resize_exact(SIDE, SIDE, FilterType::Triangle);
    Ok(cropped.to_rgba8().into_raw())
}

fn square_region(width: u32, height: u32) -> Rectangle<u32> {
    // YouTube's 4:3 video thumbnails letterbox 16:9 frames; crop inside the picture.
    let picture_height = if width * 3 == height * 4 {
        width * 9 / 16
    } else {
        height
    };
    let side = width.min(picture_height);
    Rectangle {
        x: (width - side) / 2,
        y: (height - side) / 2,
        width: side,
        height: side,
    }
}

pub fn is_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    ALLOWED_HOSTS.iter().any(|suffix| host.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "https://i.ytimg.com/vi/a/default.jpg";
    const B: &str = "https://lh3.googleusercontent.com/b=w60";
    const C: &str = "https://yt3.ggpht.com/c";

    fn pixels() -> Result<Vec<u8>, String> {
        Ok(vec![0; PIXEL_BYTES])
    }

    fn url(index: usize) -> String {
        format!("https://i.ytimg.com/vi/{index}/default.jpg")
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::new(width, height)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn crops_a_centered_square_inside_the_picture() {
        let square = |x, y, side| Rectangle {
            x,
            y,
            width: side,
            height: side,
        };
        assert_eq!(square_region(544, 544), square(0, 0, 544));
        assert_eq!(square_region(480, 360), square(105, 45, 270));
        assert_eq!(square_region(168, 94), square(37, 0, 94));
        assert_eq!(square_region(90, 160), square(0, 35, 90));
    }

    #[test]
    fn any_image_becomes_a_fixed_size_square() {
        for (width, height) in [(168, 94), (480, 360), (544, 544), (90, 160)] {
            let thumbnail = square_thumbnail(&png(width, height)).unwrap();
            assert_eq!(thumbnail.len(), PIXEL_BYTES, "{width}x{height}");
        }
        assert!(square_thumbnail(b"not an image").is_err());
        assert!(square_thumbnail(&png(MAX_SOURCE_SIDE + 1, 1)).is_err());
    }

    #[test]
    fn malformed_results_count_as_failed_downloads() {
        let mut thumbs = Thumbnails::new(1);
        thumbs.wanted([A]);
        thumbs.finish(A.into(), Ok(vec![0; 100]));
        assert!(thumbs.get(A).is_none());
        assert!(thumbs.wanted([A]).is_empty(), "not retried right away");
    }

    #[test]
    fn only_youtube_image_hosts_over_https_are_fetched() {
        assert!(is_allowed(A) && is_allowed(B) && is_allowed(C));
        assert!(!is_allowed("http://i.ytimg.com/x.jpg"));
        assert!(!is_allowed("https://evil.example/ytimg.com"));
        assert!(!is_allowed("https://ytimg.com.evil.example/x"));
        assert!(!is_allowed("file:///C:/x.jpg"));
    }

    #[test]
    fn requests_each_missing_url_once() {
        let mut thumbs = Thumbnails::new(1);
        assert_eq!(thumbs.wanted([A, B, A, "https://bad.example/x"]), [A, B]);
        assert!(thumbs.wanted([A, B]).is_empty());
        thumbs.finish(A.into(), pixels());
        thumbs.finish(B.into(), Err("404".into()));
        assert!(thumbs.get(A).is_some());
        assert!(thumbs.wanted([A, B]).is_empty());
    }

    #[test]
    fn evicts_least_recently_shown_but_never_what_is_on_screen() {
        let mut thumbs = Thumbnails::new(1);
        let fits = MEGABYTE / PIXEL_BYTES;
        for index in 0..fits {
            thumbs.finish(url(index), pixels());
            thumbs.wanted([]);
        }
        let newest = url(fits - 1);
        thumbs.wanted([newest.as_str()]);
        thumbs.finish(url(fits), pixels());
        assert!(thumbs.get(&url(0)).is_none(), "oldest entry should go");
        assert!(thumbs.get(&newest).is_some() && thumbs.get(&url(fits)).is_some());
        assert!(thumbs.bytes() <= MEGABYTE);
        thumbs.wanted([newest.as_str()]);
        thumbs.set_budget(0);
        assert!(
            thumbs.get(&newest).is_some(),
            "visible entry survives a tiny budget"
        );
        assert_eq!(thumbs.entries.len(), 1);
    }

    #[test]
    fn streamed_body_stops_at_the_size_limit() {
        let mut body = Vec::new();
        assert!(append_limited(&mut body, &[0; MAX_DOWNLOAD_BYTES - 1]).is_ok());
        assert!(append_limited(&mut body, &[0; 1]).is_ok());
        assert!(append_limited(&mut body, &[0; 1]).is_err());
        assert_eq!(body.len(), MAX_DOWNLOAD_BYTES);
    }

    #[test]
    fn caps_downloads_in_flight() {
        let mut thumbs = Thumbnails::new(4);
        let urls: Vec<String> = (0..20).map(url).collect();
        assert_eq!(
            thumbs.wanted(urls.iter().map(String::as_str)).len(),
            MAX_IN_FLIGHT
        );
    }
}
