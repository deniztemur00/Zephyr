use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use thiserror::Error;

use crate::auth::{ORIGIN, Session};

mod parse;

use parse::Continuation;
pub use parse::{
    Card, Genre, HomePage, More, ParseError, Shelf, StatsPage, Target, Track, TrackPage, WebStat,
};

const PODCASTS_BROWSE_ID: &str = "FEmusic_library_non_music_audio_list";
const GENRES_BROWSE_ID: &str = "FEmusic_moods_and_genres";
const GENRE_BROWSE_ID: &str = "FEmusic_moods_and_genres_category";

struct Site {
    origin: &'static str,
    client_name: &'static str,
    client_version: &'static str,
}

// shortcut: fixed client versions; refresh them if InnerTube starts rejecting requests.
const MUSIC: Site = Site {
    origin: ORIGIN,
    client_name: "WEB_REMIX",
    client_version: "1.20261012.01.00",
};

// youtube.com lists carry per-video view counts and upload ages; YouTube Music lists don't.
const WEB: Site = Site {
    origin: "https://www.youtube.com",
    client_name: "WEB",
    client_version: "2.20261010.00.00",
};
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";

#[derive(Debug, Clone, Copy)]
pub enum BrowseId {
    Home,
    LibraryPlaylists,
    LikedSongs,
}

impl BrowseId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "FEmusic_home",
            Self::LibraryPlaylists => "FEmusic_liked_playlists",
            Self::LikedSongs => "VLLM",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "home" => Some(Self::Home),
            "playlists" => Some(Self::LibraryPlaylists),
            "liked" => Some(Self::LikedSongs),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("refusing to send the sign-in cookie to {0}")]
    BlockedHost(&'static str),
    #[error(
        "YouTube no longer accepts this sign-in; sign out in Settings > Account and sign in again"
    )]
    SignedOut,
}

// The only places the sign-in cookie may ever be sent. Every authenticated request
// goes through `post_to`, which checks this list.
const COOKIE_ORIGINS: [&str; 2] = ["https://music.youtube.com", "https://www.youtube.com"];

fn ensure_cookie_origin(origin: &'static str) -> Result<(), ApiError> {
    if COOKIE_ORIGINS.contains(&origin) {
        Ok(())
    } else {
        Err(ApiError::BlockedHost(origin))
    }
}

pub struct Client {
    http: reqwest::Client,
    session: Session,
}

impl Client {
    pub fn new(session: Session) -> Result<Self, ApiError> {
        // No redirects: a redirect could otherwise carry the request to another host.
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http, session })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub async fn home(&self) -> Result<HomePage, ApiError> {
        Ok(parse::home(&self.browse(BrowseId::Home.as_str()).await?)?)
    }

    pub async fn more_home(&self, token: &str) -> Result<HomePage, ApiError> {
        let query = format!("&ctoken={token}&continuation={token}&type=next");
        Ok(parse::home_continuation(
            &self.post("browse", query, json!({})).await?,
        )?)
    }

    pub async fn genres(&self) -> Result<Vec<Genre>, ApiError> {
        Ok(parse::genres(&self.browse(GENRES_BROWSE_ID).await?)?)
    }

    pub async fn genre_playlists(
        &self,
        params: &str,
    ) -> Result<Vec<(String, Vec<String>)>, ApiError> {
        let body = json!({ "browseId": GENRE_BROWSE_ID, "params": params });
        Ok(parse::genre_playlists(
            &self.post("browse", String::new(), body).await?,
        )?)
    }

    pub async fn library_playlists(&self) -> Result<Vec<Card>, ApiError> {
        Ok(parse::library_grid(
            &self.browse(BrowseId::LibraryPlaylists.as_str()).await?,
        )?)
    }

    pub async fn library_podcasts(&self) -> Result<Vec<Card>, ApiError> {
        Ok(parse::library_grid(
            &self.browse(PODCASTS_BROWSE_ID).await?,
        )?)
    }

    pub async fn tracks(&self, browse_id: &str) -> Result<TrackPage, ApiError> {
        Ok(parse::tracks(&self.browse(browse_id).await?)?)
    }

    pub async fn more(&self, more: &More) -> Result<(Vec<Track>, Option<More>), ApiError> {
        let body = match &more.token {
            Continuation::Command(token) => {
                self.post("browse", String::new(), json!({ "continuation": token }))
                    .await?
            }
            Continuation::Legacy(token) => {
                let query = format!("&ctoken={token}&continuation={token}&type=next");
                self.post("browse", query, json!({})).await?
            }
        };
        Ok(parse::continuation(&body, &more.context)?)
    }

    pub async fn radio(&self, playlist_id: &str) -> Result<Vec<Track>, ApiError> {
        let body = json!({
            "playlistId": format!("RDAMPL{playlist_id}"),
            "enablePersistentPlaylistPanel": true,
            "isAudioOnly": true,
        });
        Ok(parse::radio(
            &self.post("next", String::new(), body).await?,
        )?)
    }

    pub async fn browse(&self, browse_id: &str) -> Result<String, ApiError> {
        self.post("browse", String::new(), json!({ "browseId": browse_id }))
            .await
    }

    pub async fn video_stats(&self, playlist_id: &str) -> Result<StatsPage, ApiError> {
        let body = json!({ "browseId": format!("VL{playlist_id}") });
        Ok(parse::web_stats(
            &self.post_to(&WEB, "browse", String::new(), body).await?,
        )?)
    }

    pub async fn more_video_stats(&self, token: &str) -> Result<StatsPage, ApiError> {
        let body = json!({ "continuation": token });
        Ok(parse::web_stats(
            &self.post_to(&WEB, "browse", String::new(), body).await?,
        )?)
    }

    async fn post(&self, endpoint: &str, query: String, body: Value) -> Result<String, ApiError> {
        self.post_to(&MUSIC, endpoint, query, body).await
    }

    async fn post_to(
        &self,
        site: &Site,
        endpoint: &str,
        query: String,
        mut body: Value,
    ) -> Result<String, ApiError> {
        ensure_cookie_origin(site.origin)?;
        body["context"] = json!({
            "client": { "clientName": site.client_name, "clientVersion": site.client_version, "hl": "en" }
        });
        let response = self
            .http
            .post(format!(
                "{}/youtubei/v1/{endpoint}?prettyPrint=false{query}",
                site.origin
            ))
            .header("Content-Type", "application/json")
            .header("Cookie", self.session.cookie())
            .header(
                "Authorization",
                self.session.authorization_for(site.origin, unix_now()),
            )
            .header("Origin", site.origin)
            .header("X-Origin", site.origin)
            .header("X-Goog-AuthUser", "0")
            .body(body.to_string())
            .send()
            .await?
            .error_for_status()?;
        let body = response.text().await?;
        if parse::signed_out(&body) {
            return Err(ApiError::SignedOut);
        }
        Ok(body)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_only_goes_to_youtube_origins() {
        assert!(ensure_cookie_origin(MUSIC.origin).is_ok());
        assert!(ensure_cookie_origin(WEB.origin).is_ok());
        assert!(ensure_cookie_origin("https://music.youtube.com.evil.example").is_err());
        assert!(ensure_cookie_origin("http://music.youtube.com").is_err());
        assert!(ensure_cookie_origin("https://example.com").is_err());
    }
}
