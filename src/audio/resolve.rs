use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use thiserror::Error;

use super::native::NativeResolver;

const AUDIO_FORMAT: &str = "140/bestaudio[ext=m4a]";
const PRINT_TEMPLATE: &str = "%(.{url,http_headers,format_id,acodec,abr})j";
const STDERR_TAIL_CHARS: usize = 300;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("invalid video id `{0}`")]
    InvalidVideoId(String),
    #[error("could not run yt-dlp ({0}); is it installed and on PATH?")]
    Spawn(io::Error),
    #[error("yt-dlp failed: {0}")]
    Failed(String),
    #[error("unexpected yt-dlp output: {0}")]
    Output(#[from] serde_json::Error),
    #[error("{native}; yt-dlp fallback: {fallback}")]
    AllFailed {
        native: String,
        fallback: Box<ResolveError>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamInfo {
    pub url: String,
    #[serde(rename = "http_headers", default)]
    pub headers: HashMap<String, String>,
    #[serde(flatten)]
    pub format: StreamFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    Native,
    // Only yt-dlp output is deserialized, so that is the serde default.
    #[default]
    YtDlp,
}

// What the stream is, for Settings > Audio. Field names match yt-dlp's JSON.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct StreamFormat {
    #[serde(skip)]
    pub source: Source,
    #[serde(rename = "format_id", default)]
    pub itag: Option<String>,
    #[serde(rename = "acodec", default)]
    pub codec: Option<String>,
    #[serde(rename = "abr", default)]
    pub kbps: Option<f64>,
}

impl StreamFormat {
    pub fn codec_name(&self) -> Option<&str> {
        let codec = self.codec.as_deref()?;
        Some(match codec {
            "mp4a.40.2" => "AAC-LC",
            "mp4a.40.5" => "HE-AAC",
            "mp4a.40.29" => "HE-AAC v2",
            "opus" => "Opus",
            other => other,
        })
    }
}

pub struct Resolver {
    native: Option<NativeResolver>,
    ytdlp: PathBuf,
}

impl Resolver {
    pub fn new(ytdlp: PathBuf) -> Self {
        Self {
            native: NativeResolver::new().ok(),
            ytdlp,
        }
    }

    pub fn warm_up(&self) {
        if let Some(native) = &self.native {
            let _ = native.warm_up();
        }
    }

    pub fn resolve_native(&self, video_id: &str) -> Result<StreamInfo, String> {
        let native = self.native.as_ref().ok_or("native client unavailable")?;
        native.resolve(video_id).map_err(|err| err.to_string())
    }

    pub fn resolve(&self, video_id: &str) -> Result<StreamInfo, ResolveError> {
        if !is_video_id(video_id) {
            return Err(ResolveError::InvalidVideoId(video_id.to_owned()));
        }
        let native = match self.native.as_ref().map(|native| native.resolve(video_id)) {
            Some(Ok(stream)) => return Ok(stream),
            Some(Err(err)) => err.to_string(),
            None => "native client unavailable".to_owned(),
        };
        with_ytdlp(&self.ytdlp, video_id).map_err(|fallback| ResolveError::AllFailed {
            native,
            fallback: Box::new(fallback),
        })
    }
}

fn with_ytdlp(ytdlp: &Path, video_id: &str) -> Result<StreamInfo, ResolveError> {
    let output = command(ytdlp)
        .args(["--no-playlist", "--no-warnings", "-f", AUDIO_FORMAT])
        .args(["-O", PRINT_TEMPLATE, "--"])
        .arg(format!("https://music.youtube.com/watch?v={video_id}"))
        .output()
        .map_err(ResolveError::Spawn)?;
    if !output.status.success() {
        return Err(ResolveError::Failed(stderr_tail(&output.stderr)));
    }
    parse(&output.stdout)
}

fn parse(stdout: &[u8]) -> Result<StreamInfo, ResolveError> {
    Ok(serde_json::from_slice(stdout)?)
}

fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    let start = text
        .char_indices()
        .rev()
        .nth(STDERR_TAIL_CHARS)
        .map_or(0, |(index, _)| index);
    text[start..].to_owned()
}

#[cfg(windows)]
fn command(program: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(windows))]
fn command(program: &Path) -> Command {
    Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_printed_fields() {
        let info = parse(
            br#"{"url": "https://rr1.googlevideo.com/videoplayback?x=1", "http_headers": {"User-Agent": "UA"}}"#,
        )
        .unwrap();
        assert!(info.url.starts_with("https://"));
        assert_eq!(info.headers["User-Agent"], "UA");
        assert_eq!(info.format, StreamFormat::default());
    }

    #[test]
    fn parses_format_details_from_ytdlp() {
        let info = parse(
            br#"{"url": "https://x", "format_id": "140", "acodec": "mp4a.40.2", "abr": 129.477}"#,
        )
        .unwrap();
        assert_eq!(info.format.source, Source::YtDlp);
        assert_eq!(info.format.itag.as_deref(), Some("140"));
        assert_eq!(info.format.codec_name(), Some("AAC-LC"));
        assert_eq!(info.format.kbps.map(f64::round), Some(129.0));
        let missing = parse(br#"{"url": "https://x", "abr": null}"#).unwrap();
        assert_eq!(missing.format.kbps, None);
    }

    #[test]
    fn names_known_codecs() {
        let named = |codec: &str| {
            StreamFormat {
                codec: Some(codec.into()),
                ..StreamFormat::default()
            }
            .codec_name()
            .map(str::to_owned)
        };
        assert_eq!(named("mp4a.40.5").as_deref(), Some("HE-AAC"));
        assert_eq!(named("flac").as_deref(), Some("flac"));
        assert_eq!(StreamFormat::default().codec_name(), None);
    }

    #[test]
    fn rejects_garbage_output() {
        assert!(matches!(parse(b"ERROR: x"), Err(ResolveError::Output(_))));
        assert!(matches!(parse(b""), Err(ResolveError::Output(_))));
    }

    #[test]
    fn validates_video_ids() {
        assert!(is_video_id("0gAQLStkors"));
        assert!(is_video_id("a-b_c-d_e-f"));
        assert!(!is_video_id("--exec=calc"));
        assert!(!is_video_id("short"));
        assert!(!is_video_id("0gAQLStkor$"));
    }

    #[test]
    fn keeps_only_the_end_of_long_stderr() {
        let long = "x".repeat(1000) + "ERROR: the real cause";
        let tail = stderr_tail(long.as_bytes());
        assert!(tail.ends_with("the real cause"));
        assert!(tail.chars().count() <= STDERR_TAIL_CHARS + 1);
    }
}
