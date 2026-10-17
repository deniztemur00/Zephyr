use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use super::resolve::{Source, StreamFormat, StreamInfo};

// shortcut: mimics the visionOS client, the one yt-dlp currently uses without PO tokens or JS.
// When YouTube changes it, resolution falls back to yt-dlp; update these constants then.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
const CLIENT_NAME: &str = "VISIONOS";
const CLIENT_NAME_ID: &str = "101";
const CLIENT_VERSION: &str = "0.1";
const HOME_URL: &str = "https://www.youtube.com/";
const PLAYER_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const VISITOR_KEY: &str = "\"visitorData\":\"";
const PREFERRED_ITAG: u32 = 140;

#[derive(Debug, Error)]
pub enum NativeError {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("no visitor data on the YouTube home page")]
    NoVisitorData,
    #[error("unexpected player response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not playable: {0}")]
    Unplayable(String),
    #[error("no AAC audio stream offered")]
    NoAudio,
}

pub struct NativeResolver {
    http: Client,
    visitor: Mutex<Option<String>>,
}

impl NativeResolver {
    pub fn new() -> Result<Self, NativeError> {
        let http = Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self {
            http,
            visitor: Mutex::new(None),
        })
    }

    pub fn warm_up(&self) -> Result<(), NativeError> {
        self.visitor().map(drop)
    }

    pub fn resolve(&self, video_id: &str) -> Result<StreamInfo, NativeError> {
        self.try_resolve(video_id).or_else(|_| {
            *self.visitor.lock().unwrap_or_else(PoisonError::into_inner) = None;
            self.try_resolve(video_id)
        })
    }

    fn try_resolve(&self, video_id: &str) -> Result<StreamInfo, NativeError> {
        let visitor = self.visitor()?;
        self.player(video_id, &visitor)
    }

    fn visitor(&self) -> Result<String, NativeError> {
        let mut cached = self.visitor.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(visitor) = cached.as_ref() {
            return Ok(visitor.clone());
        }
        let page = self
            .http
            .get(HOME_URL)
            .header("Accept-Language", "en")
            .send()?
            .error_for_status()?
            .text()?;
        let visitor = extract_visitor_data(&page)
            .ok_or(NativeError::NoVisitorData)?
            .to_owned();
        *cached = Some(visitor.clone());
        Ok(visitor)
    }

    fn player(&self, video_id: &str, visitor: &str) -> Result<StreamInfo, NativeError> {
        let body = json!({
            "context": { "client": {
                "clientName": CLIENT_NAME,
                "clientVersion": CLIENT_VERSION,
                "deviceMake": "Apple",
                "deviceModel": "RealityDevice14,1",
                "osName": "visionOS",
                "osVersion": "1.3.21O771",
                "userAgent": USER_AGENT,
                "hl": "en",
                "visitorData": visitor,
            }},
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        let response = self
            .http
            .post(PLAYER_URL)
            .header("Content-Type", "application/json")
            .header("X-YouTube-Client-Name", CLIENT_NAME_ID)
            .header("X-YouTube-Client-Version", CLIENT_VERSION)
            .header("X-Goog-Visitor-Id", visitor)
            .header("Origin", "https://www.youtube.com")
            .body(body.to_string())
            .send()?
            .error_for_status()?
            .text()?;
        parse_player(&response)
    }
}

fn extract_visitor_data(page: &str) -> Option<&str> {
    let start = page.find(VISITOR_KEY)? + VISITOR_KEY.len();
    let rest = &page[start..];
    let value = &rest[..rest.find('"')?];
    (!value.is_empty()).then_some(value)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayerResponse {
    playability_status: Playability,
    #[serde(default)]
    streaming_data: Option<StreamingData>,
}

#[derive(Deserialize)]
struct Playability {
    status: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamingData {
    #[serde(default)]
    adaptive_formats: Vec<Format>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Format {
    itag: u32,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    average_bitrate: Option<u64>,
}

impl Format {
    fn details(&self) -> StreamFormat {
        StreamFormat {
            source: Source::Native,
            itag: Some(self.itag.to_string()),
            codec: self
                .mime_type
                .split("codecs=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .map(str::to_owned),
            kbps: self.average_bitrate.map(|bps| bps as f64 / 1000.0),
        }
    }
}

fn parse_player(body: &str) -> Result<StreamInfo, NativeError> {
    let response: PlayerResponse = serde_json::from_str(body)?;
    let status = response.playability_status;
    if status.status != "OK" {
        return Err(NativeError::Unplayable(
            status.reason.unwrap_or(status.status),
        ));
    }
    let formats = response
        .streaming_data
        .map(|data| data.adaptive_formats)
        .unwrap_or_default();
    let is_aac =
        |format: &&Format| format.mime_type.starts_with("audio/mp4") && format.url.is_some();
    let chosen = formats
        .iter()
        .filter(is_aac)
        .find(|format| format.itag == PREFERRED_ITAG)
        .or_else(|| formats.iter().find(is_aac))
        .ok_or(NativeError::NoAudio)?;
    Ok(StreamInfo {
        url: chosen.url.clone().ok_or(NativeError::NoAudio)?,
        headers: HashMap::from([("User-Agent".to_owned(), USER_AGENT.to_owned())]),
        format: chosen.details(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_visitor_data() {
        let page = r#"<script>var ytcfg={"visitorData":"Cgt4eHh4eHh4eHh4","x":1}</script>"#;
        assert_eq!(extract_visitor_data(page), Some("Cgt4eHh4eHh4eHh4"));
        assert_eq!(extract_visitor_data("no data here"), None);
        assert_eq!(extract_visitor_data(r#""visitorData":"""#), None);
    }

    #[test]
    fn prefers_itag_140_then_any_aac() {
        let body = r#"{"playabilityStatus":{"status":"OK"},"streamingData":{"adaptiveFormats":[
            {"itag":251,"url":"https://x/opus","mimeType":"audio/webm; codecs=\"opus\""},
            {"itag":139,"url":"https://x/low","mimeType":"audio/mp4; codecs=\"mp4a.40.5\""},
            {"itag":140,"url":"https://x/aac","mimeType":"audio/mp4; codecs=\"mp4a.40.2\""}]}}"#;
        assert_eq!(parse_player(body).unwrap().url, "https://x/aac");
        let fallback = body.replace("\"itag\":140", "\"itag\":141");
        assert_eq!(parse_player(&fallback).unwrap().url, "https://x/low");
    }

    #[test]
    fn records_the_chosen_format() {
        let body = r#"{"playabilityStatus":{"status":"OK"},"streamingData":{"adaptiveFormats":[
            {"itag":140,"url":"https://x/aac","mimeType":"audio/mp4; codecs=\"mp4a.40.2\"","averageBitrate":129476}]}}"#;
        let format = parse_player(body).unwrap().format;
        assert_eq!(format.source, Source::Native);
        assert_eq!(format.itag.as_deref(), Some("140"));
        assert_eq!(format.codec.as_deref(), Some("mp4a.40.2"));
        assert_eq!(format.kbps.map(f64::round), Some(129.0));
        let no_bitrate = body.replace(r#","averageBitrate":129476"#, "");
        assert_eq!(parse_player(&no_bitrate).unwrap().format.kbps, None);
    }

    #[test]
    fn reports_unplayable_reason() {
        let body =
            r#"{"playabilityStatus":{"status":"LOGIN_REQUIRED","reason":"Sign in to confirm"}}"#;
        assert!(
            matches!(parse_player(body), Err(NativeError::Unplayable(r)) if r == "Sign in to confirm")
        );
    }

    #[test]
    fn rejects_formats_without_direct_urls() {
        let body = r#"{"playabilityStatus":{"status":"OK"},"streamingData":{"adaptiveFormats":[
            {"itag":140,"signatureCipher":"s=abc","mimeType":"audio/mp4"}]}}"#;
        assert!(matches!(parse_player(body), Err(NativeError::NoAudio)));
        assert!(matches!(parse_player("{}"), Err(NativeError::Json(_))));
    }
}
