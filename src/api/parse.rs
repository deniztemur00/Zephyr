use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

const SINGLE_COLUMN_SECTIONS: &str = "/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents";
const TRACK_SECTIONS: &str =
    "/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents";
const PAGE_HEADER: &str = "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicResponsiveHeaderRenderer";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("response is not valid JSON: {0}")]
    Json(String),
    #[error("unexpected response shape: no {0}")]
    Missing(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Song(String),
    Page(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub title: String,
    pub subtitle: String,
    pub target: Target,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub title: String,
    pub cards: Vec<Card>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artists: String,
    pub album: Option<String>,
    pub duration: Option<String>,
    pub thumbnail: Option<String>,
}

// Smallest thumbnail at least this wide, so HiDPI screens stay sharp without big downloads.
const ROW_THUMB_WIDTH: u64 = 60;
const CARD_THUMB_WIDTH: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomePage {
    pub shelves: Vec<Shelf>,
    pub next: Option<String>,
}

pub fn home(body: &str) -> Result<HomePage, ParseError> {
    let root = parse_json(body)?;
    let list = root
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer")
        .ok_or(ParseError::Missing("home sections"))?;
    home_sections(list, "home sections")
}

pub fn home_continuation(body: &str) -> Result<HomePage, ParseError> {
    let root = parse_json(body)?;
    let list = root
        .pointer("/continuationContents/sectionListContinuation")
        .ok_or(ParseError::Missing("home continuation"))?;
    home_sections(list, "home continuation")
}

fn home_sections(list: &Value, what: &'static str) -> Result<HomePage, ParseError> {
    let sections = list
        .get("contents")
        .and_then(Value::as_array)
        .ok_or(ParseError::Missing(what))?;
    let shelves = sections
        .iter()
        .filter_map(|section| shelf(section.get("musicCarouselShelfRenderer")?))
        .collect();
    let next = list
        .pointer("/continuations/0/nextContinuationData/continuation")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(HomePage { shelves, next })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    pub name: String,
    pub params: String,
}

pub fn genres(body: &str) -> Result<Vec<Genre>, ParseError> {
    let root = parse_json(body)?;
    let sections = array_at(&root, SINGLE_COLUMN_SECTIONS, "genre sections")?;
    let grid = sections
        .iter()
        .filter_map(|section| section.get("gridRenderer"))
        .find(|grid| {
            grid.pointer("/header/gridHeaderRenderer/title")
                .map(text)
                .is_some_and(|title| title == "Genres")
        })
        .ok_or(ParseError::Missing("genres grid"))?;
    let items = grid
        .get("items")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    Ok(items
        .iter()
        .filter_map(|item| {
            let button = item.get("musicNavigationButtonRenderer")?;
            Some(Genre {
                name: text(button.get("buttonText")?),
                params: button
                    .pointer("/clickCommand/browseEndpoint/params")?
                    .as_str()?
                    .to_owned(),
            })
        })
        .collect())
}

pub fn genre_playlists(body: &str) -> Result<Vec<(String, Vec<String>)>, ParseError> {
    let root = parse_json(body)?;
    let sections = array_at(&root, SINGLE_COLUMN_SECTIONS, "genre page")?;
    Ok(sections
        .iter()
        .filter_map(|section| shelf(section.get("musicCarouselShelfRenderer")?))
        .map(|shelf| {
            let ids = shelf
                .cards
                .into_iter()
                .filter_map(|card| match card.target {
                    Target::Page(id) => id.strip_prefix("VL").map(str::to_owned),
                    Target::Song(_) => None,
                })
                .collect();
            (shelf.title, ids)
        })
        .collect())
}

pub fn library_grid(body: &str) -> Result<Vec<Card>, ParseError> {
    let root = parse_json(body)?;
    let sections = array_at(&root, SINGLE_COLUMN_SECTIONS, "library sections")?;
    Ok(sections
        .iter()
        .filter_map(|section| section.pointer("/gridRenderer/items")?.as_array())
        .flatten()
        .filter_map(|item| two_row_card(item.get("musicTwoRowItemRenderer")?))
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
    Command(String),
    Legacy(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowContext {
    album: Option<String>,
    artist: String,
    thumbnail: Option<String>,
}

impl RowContext {
    fn apply(&self, track: Track) -> Track {
        Track {
            artists: if track.artists.is_empty() {
                self.artist.clone()
            } else {
                track.artists
            },
            album: self.album.clone().or(track.album),
            thumbnail: track.thumbnail.or_else(|| self.thumbnail.clone()),
            ..track
        }
    }

    fn row(&self, row: &Value) -> Option<Track> {
        if let Some(node) = row.get("musicMultiRowListItemRenderer") {
            let episode = episode(node, &self.artist)?;
            return Some(Track {
                thumbnail: episode.thumbnail.or_else(|| self.thumbnail.clone()),
                ..episode
            });
        }
        list_row(row).map(|track| self.apply(track))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct More {
    pub token: Continuation,
    pub context: RowContext,
}

#[cfg(test)]
impl More {
    pub fn test_token() -> Self {
        Self {
            token: Continuation::Command("test".into()),
            context: RowContext::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackPage {
    pub title: String,
    pub tracks: Vec<Track>,
    pub more: Option<More>,
}

pub fn tracks(body: &str) -> Result<TrackPage, ParseError> {
    let root = parse_json(body)?;
    let sections = array_at(&root, TRACK_SECTIONS, "track list")?;
    let header = root.pointer(PAGE_HEADER);
    let title = header
        .and_then(|header| header.get("title"))
        .map(text)
        .unwrap_or_default();
    let shelf = |name: &str| sections.iter().find_map(|section| section.get(name));
    let (shelf, context) = match shelf("musicPlaylistShelfRenderer") {
        Some(playlist) => (playlist, RowContext::default()),
        None => {
            let shelf = shelf("musicShelfRenderer").ok_or(ParseError::Missing("track list"))?;
            let artist = header
                .and_then(|header| header.get("straplineTextOne"))
                .map(text)
                .unwrap_or_default();
            let context = RowContext {
                album: Some(title.clone()),
                artist,
                thumbnail: header.and_then(|header| thumbnail(header, ROW_THUMB_WIDTH)),
            };
            (shelf, context)
        }
    };
    let rows = shelf
        .get("contents")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let (tracks, token) = read_rows(rows, shelf, &context);
    let more = token.map(|token| More { token, context });
    Ok(TrackPage {
        title,
        tracks,
        more,
    })
}

pub fn continuation(
    body: &str,
    context: &RowContext,
) -> Result<(Vec<Track>, Option<More>), ParseError> {
    let root = parse_json(body)?;
    let appended = root
        .get("onResponseReceivedActions")
        .and_then(Value::as_array)
        .and_then(|actions| {
            actions.iter().find_map(|action| {
                action.pointer("/appendContinuationItemsAction/continuationItems")
            })
        });
    let (rows, shelf) = match appended.and_then(Value::as_array) {
        Some(items) => (items.as_slice(), &Value::Null),
        None => {
            let shelf = root
                .pointer("/continuationContents/musicShelfContinuation")
                .or_else(|| root.pointer("/continuationContents/musicPlaylistShelfContinuation"))
                .ok_or(ParseError::Missing("continuation items"))?;
            let rows = shelf
                .get("contents")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            (rows, shelf)
        }
    };
    let (tracks, token) = read_rows(rows, shelf, context);
    let more = token.map(|token| More {
        token,
        context: context.clone(),
    });
    Ok((tracks, more))
}

fn read_rows(
    rows: &[Value],
    shelf: &Value,
    context: &RowContext,
) -> (Vec<Track>, Option<Continuation>) {
    let tracks = rows.iter().filter_map(|row| context.row(row)).collect();
    let command = rows.iter().rev().find_map(|row| {
        row.pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")?
            .as_str()
            .map(|token| Continuation::Command(token.to_owned()))
    });
    let legacy = || {
        shelf
            .pointer("/continuations/0/nextContinuationData/continuation")?
            .as_str()
            .map(|token| Continuation::Legacy(token.to_owned()))
    };
    (tracks, command.or_else(legacy))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebStat {
    pub video_id: String,
    pub views: u64,
    pub age_days: Option<u32>,
    pub title: String,
    pub channel: String,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsPage {
    pub stats: Vec<WebStat>,
    pub next: Option<String>,
}

pub fn web_stats(body: &str) -> Result<StatsPage, ParseError> {
    let root = parse_json(body)?;
    let mut lockups = Vec::new();
    collect(&root, "lockupViewModel", &mut lockups);
    let stats = lockups.into_iter().filter_map(web_stat).collect();
    let mut tokens = Vec::new();
    collect(&root, "continuationCommand", &mut tokens);
    let next = tokens
        .last()
        .and_then(|command| command.get("token")?.as_str())
        .map(str::to_owned);
    Ok(StatsPage { stats, next })
}

fn web_stat(lockup: &Value) -> Option<WebStat> {
    let mut texts = Vec::new();
    collect(lockup, "content", &mut texts);
    let texts: Vec<&str> = texts.into_iter().filter_map(Value::as_str).collect();
    let metadata = lockup.pointer("/metadata/lockupMetadataViewModel");
    let content = |pointer: &str| {
        metadata
            .and_then(|metadata| metadata.pointer(pointer)?.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some(WebStat {
        video_id: lockup.get("contentId")?.as_str()?.to_owned(),
        views: texts.iter().find_map(|text| parse_views(text))?,
        age_days: texts.iter().find_map(|text| parse_age_days(text)),
        title: content("/title/content"),
        channel: content(
            "/metadata/contentMetadataViewModel/metadataRows/0/metadataParts/0/text/content",
        ),
        thumbnail: lockup
            .pointer("/contentImage/thumbnailViewModel/image/sources/0/url")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn collect<'a>(node: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match node {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                out.push(found);
            }
            map.values().for_each(|value| collect(value, key, out));
        }
        Value::Array(items) => items.iter().for_each(|value| collect(value, key, out)),
        _ => {}
    }
}

pub fn parse_views(text: &str) -> Option<u64> {
    let number = text
        .strip_suffix(" views")
        .or_else(|| text.strip_suffix(" view"))?;
    if number == "No" {
        return Some(0);
    }
    let (digits, scale) = match number.chars().last()? {
        'K' => (&number[..number.len() - 1], 1e3),
        'M' => (&number[..number.len() - 1], 1e6),
        'B' => (&number[..number.len() - 1], 1e9),
        _ => (number, 1.0),
    };
    let value: f64 = digits.replace(',', "").parse().ok()?;
    Some((value * scale).round() as u64)
}

pub fn parse_age_days(text: &str) -> Option<u32> {
    let words: Vec<&str> = text.strip_suffix(" ago")?.split_whitespace().collect();
    let [.., count, unit] = words.as_slice() else {
        return None;
    };
    let count: u32 = count.parse().ok()?;
    let days = match unit.trim_end_matches('s') {
        "second" | "minute" | "hour" => 0,
        "day" => 1,
        "week" => 7,
        "month" => 30,
        "year" => 365,
        _ => return None,
    };
    Some(count * days)
}

pub fn radio(body: &str) -> Result<Vec<Track>, ParseError> {
    let root = parse_json(body)?;
    let items = array_at(
        &root,
        "/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs/0/tabRenderer/content/musicQueueRenderer/content/playlistPanelRenderer/contents",
        "radio queue",
    )?;
    Ok(items
        .iter()
        .filter_map(|item| {
            item.get("playlistPanelVideoRenderer").or_else(|| {
                item.pointer(
                    "/playlistPanelVideoWrapperRenderer/primaryRenderer/playlistPanelVideoRenderer",
                )
            })
        })
        .filter_map(panel_track)
        .collect())
}

fn panel_track(node: &Value) -> Option<Track> {
    let byline = node.get("longBylineText").map(text).unwrap_or_default();
    let parts: Vec<&str> = byline.split(" • ").collect();
    let is_year = |part: &str| part.len() == 4 && part.bytes().all(|b| b.is_ascii_digit());
    let album = match parts.as_slice() {
        [_, album, year] if is_year(year) => Some((*album).to_owned()),
        _ => None,
    };
    Some(Track {
        video_id: node.get("videoId")?.as_str()?.to_owned(),
        title: text(node.get("title")?),
        artists: parts.first().copied().unwrap_or_default().to_owned(),
        album,
        duration: node.get("lengthText").map(text).filter(|d| !d.is_empty()),
        thumbnail: thumbnail(node, ROW_THUMB_WIDTH),
    })
}

fn thumbnail(node: &Value, min_width: u64) -> Option<String> {
    let list = [
        "/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails",
        "/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails",
        "/thumbnail/thumbnails",
    ]
    .iter()
    .find_map(|pointer| node.pointer(pointer)?.as_array())?;
    let sized = || {
        list.iter()
            .filter_map(|item| Some((item.get("width")?.as_u64()?, item.get("url")?.as_str()?)))
    };
    sized()
        .filter(|(width, _)| *width >= min_width)
        .min_by_key(|(width, _)| *width)
        .or_else(|| sized().max_by_key(|(width, _)| *width))
        .map(|(_, url)| url.to_owned())
}

fn episode(node: &Value, host: &str) -> Option<Track> {
    let video_id = node
        .pointer("/onTap/watchEndpoint/videoId")?
        .as_str()?
        .to_owned();
    let non_empty = |value: String| (!value.is_empty()).then_some(value);
    let duration = node
        .pointer("/playbackProgress/musicPlaybackProgressRenderer/durationText")
        .map(|value| {
            text(value)
                .trim_matches(|c: char| c == '•' || c.is_whitespace())
                .to_owned()
        })
        .and_then(non_empty);
    Some(Track {
        video_id,
        title: non_empty(text(node.get("title")?))?,
        artists: host.to_owned(),
        album: node.get("subtitle").map(text).and_then(non_empty),
        duration,
        thumbnail: thumbnail(node, ROW_THUMB_WIDTH),
    })
}

fn list_row(row: &Value) -> Option<Track> {
    track(row.get("musicResponsiveListItemRenderer")?)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Envelope {
    response_context: ResponseContext,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ResponseContext {
    service_tracking_params: Vec<Service>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Service {
    params: Vec<Param>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Param {
    key: String,
    value: String,
}

// YouTube answers an expired cookie with a normal 200 page for signed-out users.
pub fn signed_out(body: &str) -> bool {
    serde_json::from_str::<Envelope>(body).is_ok_and(|envelope| {
        envelope
            .response_context
            .service_tracking_params
            .iter()
            .flat_map(|service| &service.params)
            .any(|param| param.key == "logged_in" && param.value == "0")
    })
}

fn parse_json(body: &str) -> Result<Value, ParseError> {
    serde_json::from_str(body).map_err(|err| ParseError::Json(err.to_string()))
}

fn array_at<'a>(
    root: &'a Value,
    pointer: &str,
    what: &'static str,
) -> Result<&'a Vec<Value>, ParseError> {
    root.pointer(pointer)
        .and_then(Value::as_array)
        .ok_or(ParseError::Missing(what))
}

fn shelf(node: &Value) -> Option<Shelf> {
    let title = text(node.pointer("/header/musicCarouselShelfBasicHeaderRenderer/title")?);
    let cards: Vec<Card> = node
        .get("contents")?
        .as_array()?
        .iter()
        .filter_map(shelf_item)
        .collect();
    (!cards.is_empty()).then_some(Shelf { title, cards })
}

fn shelf_item(item: &Value) -> Option<Card> {
    if let Some(node) = item.get("musicTwoRowItemRenderer") {
        return two_row_card(node);
    }
    let node = item.get("musicResponsiveListItemRenderer")?;
    let track = track(node)?;
    Some(Card {
        target: Target::Song(track.video_id),
        title: track.title,
        subtitle: track.artists,
        thumbnail: thumbnail(node, CARD_THUMB_WIDTH),
    })
}

fn two_row_card(node: &Value) -> Option<Card> {
    Some(Card {
        title: text(node.get("title")?),
        subtitle: node.get("subtitle").map(text).unwrap_or_default(),
        target: target(node.get("navigationEndpoint")?)?,
        thumbnail: thumbnail(node, CARD_THUMB_WIDTH),
    })
}

fn target(endpoint: &Value) -> Option<Target> {
    let id = |pointer| endpoint.pointer(pointer).and_then(Value::as_str);
    id("/watchEndpoint/videoId")
        .map(|video| Target::Song(video.to_owned()))
        .or_else(|| id("/browseEndpoint/browseId").map(|page| Target::Page(page.to_owned())))
}

fn track(node: &Value) -> Option<Track> {
    let columns = node.get("flexColumns")?.as_array()?;
    let column = |index: usize| {
        columns
            .get(index)?
            .pointer("/musicResponsiveListItemFlexColumnRenderer/text")
            .map(text)
            .filter(|value| !value.is_empty())
    };
    let video_id = node
        .pointer("/playlistItemData/videoId")
        .or_else(|| {
            columns.first()?.pointer(
                "/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId",
            )
        })?
        .as_str()?;
    Some(Track {
        video_id: video_id.to_owned(),
        title: column(0)?,
        artists: column(1).unwrap_or_default(),
        album: column(2),
        duration: node
            .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text")
            .map(text)
            .filter(|value| !value.is_empty()),
        thumbnail: thumbnail(node, ROW_THUMB_WIDTH),
    })
}

fn text(node: &Value) -> String {
    if let Some(runs) = node.get("runs").and_then(Value::as_array) {
        return runs
            .iter()
            .filter_map(|run| run.get("text")?.as_str())
            .collect();
    }
    node.get("simpleText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = include_str!("../../tests/fixtures/home.json");
    const PLAYLISTS: &str = include_str!("../../tests/fixtures/library_playlists.json");
    const LIKED: &str = include_str!("../../tests/fixtures/liked.json");
    const ALBUM: &str = include_str!("../../tests/fixtures/album.json");
    const PODCAST: &str = include_str!("../../tests/fixtures/podcast.json");

    #[test]
    fn home_parses_two_row_and_list_shelves() {
        let shelves = home(HOME).unwrap().shelves;
        assert_eq!(shelves.len(), 2);
        assert_eq!(shelves[0].title, "Listen again");
        assert_eq!(
            shelves[0].cards[0],
            Card {
                title: "Song One".into(),
                subtitle: "Artist A • 1M views".into(),
                target: Target::Song("vid00000001".into()),
                thumbnail: None,
            }
        );
        assert_eq!(shelves[0].cards[1].target, Target::Page("VLPLfake".into()));
        assert_eq!(shelves[1].title, "Quick picks");
        assert_eq!(
            shelves[1].cards[0].target,
            Target::Song("vid00000003".into())
        );
        assert_eq!(shelves[1].cards[0].subtitle, "Artist C");
    }

    #[test]
    fn library_skips_new_playlist_button() {
        let titles: Vec<_> = library_grid(PLAYLISTS)
            .unwrap()
            .into_iter()
            .map(|card| card.title)
            .collect();
        assert_eq!(titles, ["Liked Music", "Road trip"]);
    }

    #[test]
    fn album_takes_artist_and_album_from_header() {
        let page = tracks(ALBUM).unwrap();
        assert_eq!(page.title, "Fake Album");
        assert_eq!(
            page.tracks,
            [
                Track {
                    video_id: "vid00000020".into(),
                    title: "Album Opener".into(),
                    artists: "Album Artist".into(),
                    album: Some("Fake Album".into()),
                    duration: Some("3:29".into()),
                    thumbnail: None,
                },
                Track {
                    video_id: "vid00000021".into(),
                    title: "Feature".into(),
                    artists: "Guest".into(),
                    album: Some("Fake Album".into()),
                    duration: None,
                    thumbnail: None,
                },
            ]
        );
    }

    #[test]
    fn podcast_episodes_become_tracks() {
        let page = tracks(PODCAST).unwrap();
        assert_eq!(page.title, "Fake Show");
        assert_eq!(
            page.tracks,
            [Track {
                video_id: "vid00000030".into(),
                title: "Episode One".into(),
                artists: "Fake Host".into(),
                album: Some("244K views • Sep 23".into()),
                duration: Some("1 hr 36 min".into()),
                thumbnail: None,
            }]
        );
    }

    #[test]
    fn liked_parses_tracks_and_skips_unplayable_rows() {
        let page = tracks(LIKED).unwrap();
        assert_eq!(page.title, "Liked Music");
        let tracks = page.tracks;
        assert_eq!(tracks.len(), 2);
        assert_eq!(
            tracks[0],
            Track {
                video_id: "vid00000010".into(),
                title: "First Track".into(),
                artists: "Artist X & Artist Y".into(),
                album: Some("Album Z".into()),
                duration: Some("3:09".into()),
                thumbnail: None,
            }
        );
        assert_eq!(tracks[1].album, None);
        assert_eq!(tracks[1].video_id, "vid00000011");
        assert_eq!(
            page.more.map(|more| more.token),
            Some(Continuation::Command("fake".into()))
        );
    }

    #[test]
    fn podcast_and_album_pages_carry_their_continuation_style() {
        let podcast = tracks(PODCAST).unwrap();
        assert_eq!(
            podcast.more.map(|more| more.token),
            Some(Continuation::Legacy("fake".into()))
        );
        assert_eq!(tracks(ALBUM).unwrap().more, None);
    }

    #[test]
    fn playlist_continuation_appends_rows_and_next_token() {
        let body = r#"{"onResponseReceivedActions":[{"appendContinuationItemsAction":{"continuationItems":[
            {"musicResponsiveListItemRenderer":{"flexColumns":[
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Later Song"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Someone"}]}}}],
             "playlistItemData":{"videoId":"vid00000040"}}},
            {"continuationItemRenderer":{"continuationEndpoint":{"continuationCommand":{"token":"next"}}}}]}}]}"#;
        let (tracks, more) = continuation(body, &RowContext::default()).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Later Song");
        assert_eq!(
            more.map(|m| m.token),
            Some(Continuation::Command("next".into()))
        );
    }

    #[test]
    fn podcast_continuation_uses_host_from_context() {
        let context = tracks(PODCAST).unwrap().more.unwrap().context;
        let body = r#"{"continuationContents":{"musicShelfContinuation":{"contents":[
            {"musicMultiRowListItemRenderer":{"onTap":{"watchEndpoint":{"videoId":"vid00000041"}},
             "title":{"runs":[{"text":"Episode Two"}]}}}]}}}"#;
        let (tracks, more) = continuation(body, &context).unwrap();
        assert_eq!(tracks[0].artists, "Fake Host");
        assert_eq!(more, None);
        assert_eq!(
            continuation("{}", &context),
            Err(ParseError::Missing("continuation items"))
        );
    }

    #[test]
    fn radio_reads_panel_videos_and_wrapped_ones() {
        let body = r#"{"contents":{"singleColumnMusicWatchNextResultsRenderer":{"tabbedRenderer":{"watchNextTabbedResultsRenderer":{"tabs":[{"tabRenderer":{"content":{"musicQueueRenderer":{"content":{"playlistPanelRenderer":{"contents":[
            {"playlistPanelVideoRenderer":{"videoId":"vid00000050","title":{"runs":[{"text":"Song"}]},
              "longBylineText":{"runs":[{"text":"Artist"},{"text":" • "},{"text":"Album"},{"text":" • "},{"text":"2020"}]},
              "lengthText":{"runs":[{"text":"3:01"}]}}},
            {"playlistPanelVideoWrapperRenderer":{"primaryRenderer":{"playlistPanelVideoRenderer":{"videoId":"vid00000051",
              "title":{"runs":[{"text":"Video"}]},"longBylineText":{"runs":[{"text":"Channel • 1M views • 2K likes"}]}}}}},
            {"automixPreviewVideoRenderer":{}}]}}}}}}]}}}}}"#;
        let tracks = radio(body).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].album.as_deref(), Some("Album"));
        assert_eq!(tracks[0].artists, "Artist");
        assert_eq!(tracks[1].artists, "Channel");
        assert_eq!(tracks[1].album, None);
        assert_eq!(radio("{}"), Err(ParseError::Missing("radio queue")));
    }

    #[test]
    fn parses_view_counts() {
        assert_eq!(parse_views("1.7M views"), Some(1_700_000));
        assert_eq!(parse_views("12,345 views"), Some(12_345));
        assert_eq!(parse_views("244K views"), Some(244_000));
        assert_eq!(parse_views("1.1B views"), Some(1_100_000_000));
        assert_eq!(parse_views("1 view"), Some(1));
        assert_eq!(parse_views("No views"), Some(0));
        assert_eq!(parse_views("3 weeks ago"), None);
        assert_eq!(parse_views("Tyla"), None);
    }

    #[test]
    fn parses_upload_age() {
        assert_eq!(parse_age_days("6 years ago"), Some(2190));
        assert_eq!(parse_age_days("1 month ago"), Some(30));
        assert_eq!(parse_age_days("3 weeks ago"), Some(21));
        assert_eq!(parse_age_days("Streamed 2 days ago"), Some(2));
        assert_eq!(parse_age_days("5 hours ago"), Some(0));
        assert_eq!(parse_age_days("244K views"), None);
        assert_eq!(parse_age_days("ages ago"), None);
    }

    #[test]
    fn web_stats_reads_lockups_and_continuation() {
        let body = r#"{"contents":{"x":[
            {"lockupViewModel":{"contentId":"vid00000060","metadata":{"rows":[
                {"text":{"content":"Some Song"}},{"text":{"content":"1.7M views"}},{"text":{"content":"6 years ago"}}]}}},
            {"lockupViewModel":{"contentId":"vid00000061","metadata":{"rows":[{"text":{"content":"Live now"}}]}}},
            {"continuationItemRenderer":{"continuationEndpoint":{"continuationCommand":{"token":"page2"}}}}]}}"#;
        let page = web_stats(body).unwrap();
        assert_eq!(
            page.stats,
            [WebStat {
                video_id: "vid00000060".into(),
                views: 1_700_000,
                age_days: Some(2190),
                title: String::new(),
                channel: String::new(),
                thumbnail: None,
            }]
        );
        assert_eq!(page.next.as_deref(), Some("page2"));
    }

    #[test]
    fn web_stats_read_title_channel_and_art_from_lockup_metadata() {
        let body = r#"{"lockupViewModel":{"contentId":"vid00000062",
            "contentImage":{"thumbnailViewModel":{"image":{"sources":[{"url":"https://i.ytimg.com/vi/x/hq.jpg","width":168}]}}},
            "metadata":{"lockupMetadataViewModel":{"title":{"content":"Song"},"metadata":{"contentMetadataViewModel":{"metadataRows":[
                {"metadataParts":[{"text":{"content":"Indie Kid"}}]},
                {"metadataParts":[{"text":{"content":"61K views"}},{"text":{"content":"2 months ago"}}]}]}}}}}}"#;
        let stat = web_stats(body).unwrap().stats.remove(0);
        assert_eq!(
            (stat.title.as_str(), stat.channel.as_str()),
            ("Song", "Indie Kid")
        );
        assert_eq!((stat.views, stat.age_days), (61_000, Some(60)));
        assert_eq!(
            stat.thumbnail.as_deref(),
            Some("https://i.ytimg.com/vi/x/hq.jpg")
        );
    }

    #[test]
    fn home_continuation_returns_shelves_and_next_token() {
        let body = r#"{"continuationContents":{"sectionListContinuation":{"contents":[
            {"musicCarouselShelfRenderer":{"header":{"musicCarouselShelfBasicHeaderRenderer":{"title":{"runs":[{"text":"Quick picks"}]}}},
             "contents":[{"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"Mix"}]},"navigationEndpoint":{"browseEndpoint":{"browseId":"VLRDmix"}}}}]}}],
            "continuations":[{"nextContinuationData":{"continuation":"more"}}]}}}"#;
        let page = home_continuation(body).unwrap();
        assert_eq!(page.shelves[0].title, "Quick picks");
        assert_eq!(page.next.as_deref(), Some("more"));
        assert_eq!(
            home_continuation("{}"),
            Err(ParseError::Missing("home continuation"))
        );
    }

    #[test]
    fn genres_come_from_the_genres_grid_only() {
        let body = r#"{"contents":{"singleColumnBrowseResultsRenderer":{"tabs":[{"tabRenderer":{"content":{"sectionListRenderer":{"contents":[
            {"gridRenderer":{"header":{"gridHeaderRenderer":{"title":{"runs":[{"text":"Moods & moments"}]}}},
              "items":[{"musicNavigationButtonRenderer":{"buttonText":{"runs":[{"text":"Chill"}]},"clickCommand":{"browseEndpoint":{"params":"p0"}}}}]}},
            {"gridRenderer":{"header":{"gridHeaderRenderer":{"title":{"runs":[{"text":"Genres"}]}}},
              "items":[{"musicNavigationButtonRenderer":{"buttonText":{"runs":[{"text":"Turkish indie"}]},"clickCommand":{"browseEndpoint":{"params":"p1"}}}}]}}]}}}}]}}}"#;
        assert_eq!(
            genres(body).unwrap(),
            [Genre {
                name: "Turkish indie".into(),
                params: "p1".into(),
            }]
        );
    }

    #[test]
    fn genre_page_lists_playlist_ids_per_shelf() {
        let body = r#"{"contents":{"singleColumnBrowseResultsRenderer":{"tabs":[{"tabRenderer":{"content":{"sectionListRenderer":{"contents":[
            {"musicCarouselShelfRenderer":{"header":{"musicCarouselShelfBasicHeaderRenderer":{"title":{"runs":[{"text":"Community playlists"}]}}},
             "contents":[{"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"P"}]},"navigationEndpoint":{"browseEndpoint":{"browseId":"VLPLabc"}}}},
                         {"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"A"}]},"navigationEndpoint":{"browseEndpoint":{"browseId":"MPREb_x"}}}}]}}]}}}}]}}}"#;
        assert_eq!(
            genre_playlists(body).unwrap(),
            [("Community playlists".to_owned(), vec!["PLabc".to_owned()])]
        );
    }

    #[test]
    fn picks_smallest_thumbnail_that_is_wide_enough() {
        let node: Value = serde_json::from_str(
            r#"{"thumbnail":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[
                {"url":"a60","width":60},{"url":"a120","width":120},{"url":"a544","width":544}]}}}}"#,
        )
        .unwrap();
        assert_eq!(thumbnail(&node, 100).as_deref(), Some("a120"));
        assert_eq!(thumbnail(&node, 60).as_deref(), Some("a60"));
        assert_eq!(thumbnail(&node, 1000).as_deref(), Some("a544"));
        assert_eq!(thumbnail(&Value::Null, 60), None);
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(matches!(home("{\"contents\": "), Err(ParseError::Json(_))));
    }

    #[test]
    fn empty_object_reports_missing_shape() {
        assert_eq!(home("{}"), Err(ParseError::Missing("home sections")));
        assert_eq!(tracks("{}"), Err(ParseError::Missing("track list")));
        let no_shelf = r#"{"contents":{"twoColumnBrowseResultsRenderer":{"secondaryContents":{"sectionListRenderer":{"contents":[{"musicCarouselShelfRenderer":{}}]}}}}}"#;
        assert_eq!(tracks(no_shelf), Err(ParseError::Missing("track list")));
    }

    #[test]
    fn detects_signed_out_responses() {
        let with = |value: &str| {
            format!(
                r#"{{"responseContext":{{"serviceTrackingParams":[{{"service":"CSI","params":[{{"key":"c","value":"WEB_REMIX"}}]}},{{"service":"GFEEDBACK","params":[{{"key":"logged_in","value":"{value}"}}]}}]}},"contents":{{}}}}"#
            )
        };
        assert!(signed_out(&with("0")));
        assert!(!signed_out(&with("1")));
        assert!(!signed_out("{}"));
        assert!(!signed_out(r#"{"responseContext":{}}"#));
        assert!(!signed_out("not json"));
    }

    #[test]
    fn empty_sections_give_empty_lists() {
        let body = r#"{"contents":{"singleColumnBrowseResultsRenderer":{"tabs":[{"tabRenderer":{"content":{"sectionListRenderer":{"contents":[]}}}}]}}}"#;
        assert_eq!(home(body).unwrap().shelves, []);
        assert_eq!(library_grid(body).unwrap(), []);
    }

    #[test]
    #[ignore = "needs real captures: set ZEPHYR_CAPTURES to the captures folder"]
    fn parses_real_captures() {
        let dir = std::path::PathBuf::from(std::env::var("ZEPHYR_CAPTURES").unwrap());
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        let shelves = home(&read("home.json")).unwrap().shelves;
        let playlists = library_grid(&read("playlists.json")).unwrap();
        let liked = tracks(&read("liked.json")).unwrap().tracks;
        println!(
            "shelves={} cards={} playlists={} liked={}",
            shelves.len(),
            shelves.iter().map(|s| s.cards.len()).sum::<usize>(),
            playlists.len(),
            liked.len(),
        );
        assert!(!shelves.is_empty() && !playlists.is_empty() && !liked.is_empty());
        let context = tracks(&read("VLPL9a-XcBZFOg_QDtqFeH7sLonP9owBTlny.json"))
            .unwrap()
            .more
            .unwrap()
            .context;
        let (more_rows, next) = continuation(&read("cont_playlist.json"), &context).unwrap();
        println!(
            "playlist continuation: rows={} next={}",
            more_rows.len(),
            next.is_some()
        );
        let show = tracks(&read("MPSPPLCiAikFFaMJouorRXDSfS2UoKV4BfKyQm.json"))
            .unwrap()
            .more
            .unwrap();
        let (episodes, next) = continuation(&read("cont_podcast.json"), &show.context).unwrap();
        println!(
            "podcast continuation: episodes={} host='{}' next={}",
            episodes.len(),
            episodes[0].artists,
            next.is_some()
        );
        let radio_tracks = radio(&read("radio_playlist.json")).unwrap();
        println!(
            "radio: tracks={} first={:?}",
            radio_tracks.len(),
            radio_tracks
                .first()
                .map(|t| (&t.title, &t.artists, &t.album))
        );
        let with_art = liked.iter().filter(|t| t.thumbnail.is_some()).count();
        let cards_with_art = shelves
            .iter()
            .flat_map(|s| &s.cards)
            .filter(|c| c.thumbnail.is_some())
            .count();
        println!(
            "thumbnails: liked rows {with_art}/{}, home cards {cards_with_art}",
            liked.len()
        );
        for name in ["web_playlist.json", "web_liked.json", "web_podcast.json"] {
            let page = web_stats(&read(name)).unwrap();
            let aged = page.stats.iter().filter(|s| s.age_days.is_some()).count();
            println!(
                "{name}: stats={} with_age={aged} next={}",
                page.stats.len(),
                page.next.is_some()
            );
        }
        let shows = library_grid(&read("library_podcasts.json")).unwrap();
        println!(
            "podcast library: {:?}",
            shows.iter().map(|c| &c.title).collect::<Vec<_>>()
        );
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if ["VL", "MPRE", "MPSP"].iter().any(|p| name.starts_with(p))
                && !name.starts_with("cont")
            {
                let page = tracks(&read(&name)).unwrap();
                let first = page.tracks.first().unwrap();
                println!(
                    "{name}: '{}' tracks={} first artists='{}' album={:?}",
                    page.title,
                    page.tracks.len(),
                    first.artists,
                    first.album
                );
            }
        }
    }
}
