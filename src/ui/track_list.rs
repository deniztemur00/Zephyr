use std::collections::HashMap;
use std::ops::Range;

use iced::widget::{Space, button, column, container, image, row, scrollable, text};
use iced::{Alignment, ContentFit, Element, Length};

use super::thumbnails::Thumbnails;
use super::{Message, Source, clip, theme};
use crate::api::{More, Track, TrackPage};
use crate::stats::{self, Monthly, Popularity};

// Hard cap so a pathological playlist can't grow memory without bound.
pub const MAX_TRACKS: usize = 10_000;
const ROW_HEIGHT: f32 = 46.0;
const ROW_GAP: f32 = 4.0;
const ROW_PITCH: f32 = ROW_HEIGHT + ROW_GAP;
const OVERSCAN: usize = 8;
const DEFAULT_VIEWPORT: f32 = 900.0;
const TEXT_CHARS: usize = 70;
const ART_SIZE: f32 = 36.0;

pub struct TrackList {
    pub title: String,
    pub tracks: Vec<Track>,
    pub more: Option<More>,
    pub error: Option<String>,
}

impl From<TrackPage> for TrackList {
    fn from(page: TrackPage) -> Self {
        let mut list = Self {
            title: page.title,
            tracks: Vec::new(),
            more: None,
            error: None,
        };
        list.append(page.tracks, page.more);
        list
    }
}

impl TrackList {
    pub fn append(&mut self, tracks: Vec<Track>, more: Option<More>) {
        let room = MAX_TRACKS.saturating_sub(self.tracks.len());
        self.tracks.extend(tracks.into_iter().take(room));
        self.more = more.filter(|_| self.tracks.len() < MAX_TRACKS);
    }

    pub fn apply_page(
        &mut self,
        requested: &More,
        result: Result<(Vec<Track>, Option<More>), String>,
    ) -> Option<Vec<Track>> {
        if self.more.as_ref() != Some(requested) {
            return None;
        }
        match result {
            Ok((tracks, more)) => {
                let before = self.tracks.len();
                self.append(tracks, more);
                Some(self.tracks[before..].to_vec())
            }
            Err(error) => {
                self.fail(error);
                Some(Vec::new())
            }
        }
    }

    pub fn fail(&mut self, error: String) {
        self.more = None;
        self.error = Some(error);
    }

    pub fn is_complete(&self) -> bool {
        self.more.is_none()
    }

    pub fn status(&self) -> String {
        let count = self.tracks.len();
        match (&self.more, &self.error) {
            (Some(_), _) => format!("{count} tracks · loading more…"),
            (None, Some(error)) => format!("{count} tracks · stopped loading: {error}"),
            (None, None) => format!("{count} tracks"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Scroll {
    pub offset: f32,
    pub height: f32,
}

#[derive(Clone, Copy)]
pub struct Extras<'a> {
    pub art: Option<&'a Thumbnails>,
    pub stats: &'a HashMap<String, Popularity>,
}

pub fn visible_range(total: usize, scroll: Scroll) -> Range<usize> {
    let height = if scroll.height > 0.0 {
        scroll.height
    } else {
        DEFAULT_VIEWPORT
    };
    let first = ((scroll.offset / ROW_PITCH) as usize)
        .saturating_sub(OVERSCAN)
        .min(total);
    let last = (((scroll.offset + height) / ROW_PITCH) as usize + OVERSCAN).min(total);
    first..last
}

pub fn view<'a>(
    tracks: &'a [Track],
    source: Source,
    playing: Option<&str>,
    scroll: Scroll,
    extras: Extras<'a>,
) -> Element<'a, Message> {
    let total = tracks.len();
    let range = visible_range(total, scroll);
    let (first, last) = (range.start, range.end);
    let rows = tracks[range]
        .iter()
        .enumerate()
        .map(|(offset, track)| row_button(first + offset, track, source, playing, extras));
    let content = column![
        Space::new().height(first as f32 * ROW_PITCH),
        column(rows).spacing(ROW_GAP),
        Space::new().height((total - last) as f32 * ROW_PITCH),
    ];
    column![
        container(header(extras)).padding([0, 14]),
        scrollable(content)
            .spacing(8.0)
            .on_scroll(|viewport| Message::Scrolled(Scroll {
                offset: viewport.absolute_offset().y,
                height: viewport.bounds().height,
            }))
    ]
    .spacing(8)
    .into()
}

pub fn plain<'a>(
    tracks: &'a [Track],
    source: Source,
    playing: Option<&str>,
    extras: Extras<'a>,
) -> Element<'a, Message> {
    let rows = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| row_button(index, track, source, playing, extras));
    column(rows).spacing(ROW_GAP).into()
}

pub fn art<'a>(thumbnails: &'a Thumbnails, url: Option<&str>, size: f32) -> Element<'a, Message> {
    match url.and_then(|url| thumbnails.get(url)) {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Fill)
            .border_radius(4.0)
            .into(),
        None => container(Space::new())
            .width(size)
            .height(size)
            .style(theme::placeholder)
            .into(),
    }
}

fn row_button<'a>(
    index: usize,
    track: &'a Track,
    source: Source,
    playing: Option<&str>,
    extras: Extras<'a>,
) -> Element<'a, Message> {
    let is_playing = playing == Some(track.video_id.as_str());
    let muted = |value: &str| {
        text(clip(value, TEXT_CHARS))
            .size(14)
            .wrapping(text::Wrapping::None)
            .style(theme::muted)
    };
    let popularity = extras.stats.get(&track.video_id);
    let trend: Element<'a, Message> = match popularity.map(|p| p.monthly) {
        Some(Monthly::Measured(gain)) => text(signed_per_month(gain))
            .size(14)
            .wrapping(text::Wrapping::None)
            .style(theme::accent)
            .into(),
        Some(Monthly::Estimated(average)) => {
            muted(&format!("≈{}/mo", stats::compact(average))).into()
        }
        Some(Monthly::Unknown) | None => muted("").into(),
    };
    let cells = Cells {
        number: muted(&(index + 1).to_string()).into(),
        art: extras
            .art
            .map(|thumbnails| art(thumbnails, track.thumbnail.as_deref(), ART_SIZE)),
        title: text(clip(&track.title, TEXT_CHARS))
            .size(15)
            .wrapping(text::Wrapping::None)
            .into(),
        artist: muted(&track.artists).into(),
        album: muted(track.album.as_deref().unwrap_or_default()).into(),
        time: muted(track.duration.as_deref().unwrap_or_default()).into(),
        views: muted(&popularity.map_or_else(String::new, |p| stats::compact(p.views))).into(),
        trend,
    };
    button(
        container(cells.layout())
            .height(ROW_HEIGHT)
            .padding([0, 14])
            .align_y(Alignment::Center)
            .clip(true),
    )
    .width(Length::Fill)
    .padding(0)
    .style(theme::track_row(index, is_playing))
    .on_press(Message::PlayTrack { source, index })
    .into()
}

fn signed_per_month(gain: i64) -> String {
    let sign = if gain < 0 { "−" } else { "+" };
    format!("{sign}{}/mo", stats::compact(gain.unsigned_abs()))
}

fn header<'a>(extras: Extras<'a>) -> Element<'a, Message> {
    let label = |value: &'a str| -> Element<'a, Message> {
        text(value).size(12).style(theme::muted).into()
    };
    Cells {
        number: label("#"),
        art: extras.art.map(|_| Space::new().width(ART_SIZE).into()),
        title: label("TITLE"),
        artist: label("ARTIST"),
        album: label("ALBUM"),
        time: label("TIME"),
        views: label("VIEWS"),
        trend: label("TREND"),
    }
    .layout()
}

struct Cells<'a> {
    number: Element<'a, Message>,
    art: Option<Element<'a, Message>>,
    title: Element<'a, Message>,
    artist: Element<'a, Message>,
    album: Element<'a, Message>,
    time: Element<'a, Message>,
    views: Element<'a, Message>,
    trend: Element<'a, Message>,
}

impl<'a> Cells<'a> {
    fn layout(self) -> Element<'a, Message> {
        row![container(self.number).width(44).clip(true)]
            .push(self.art)
            .push(
                container(self.title)
                    .width(Length::FillPortion(4))
                    .clip(true),
            )
            .push(
                container(self.artist)
                    .width(Length::FillPortion(3))
                    .clip(true),
            )
            .push(
                container(self.album)
                    .width(Length::FillPortion(3))
                    .clip(true),
            )
            .push(container(self.time).width(80).clip(true))
            .push(container(self.views).width(64).clip(true))
            .push(container(self.trend).width(90).clip(true))
            .spacing(16)
            .align_y(Alignment::Center)
            .into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn page(count: usize, more: bool) -> TrackPage {
        TrackPage {
            title: "List".into(),
            tracks: (0..count)
                .map(|i| Track {
                    video_id: format!("v{i}"),
                    title: i.to_string(),
                    artists: String::new(),
                    album: None,
                    duration: None,
                    thumbnail: None,
                })
                .collect(),
            more: more.then(crate::api::More::test_token),
        }
    }

    #[test]
    fn appending_stops_at_the_cap() {
        let mut list = TrackList::from(page(MAX_TRACKS - 10, true));
        assert!(!list.is_complete());
        let next = page(100, true);
        list.append(next.tracks, next.more);
        assert_eq!(list.tracks.len(), MAX_TRACKS);
        assert!(list.is_complete());
    }

    #[test]
    fn pages_apply_only_to_the_request_they_answer() {
        let mut list = TrackList::from(page(100, true));
        let requested = list.more.clone().unwrap();
        let next = page(100, false);
        let added = list.apply_page(&requested, Ok((next.tracks.clone(), None)));
        assert_eq!(added.map(|tracks| tracks.len()), Some(100));
        assert_eq!(list.apply_page(&requested, Ok((next.tracks, None))), None);
        assert_eq!(list.tracks.len(), 200);
        assert!(list.is_complete());
    }

    #[test]
    fn failure_ends_paging_and_is_reported() {
        let mut list = TrackList::from(page(3, true));
        list.fail("timeout".into());
        assert!(list.is_complete());
        assert_eq!(list.status(), "3 tracks · stopped loading: timeout");
    }
}
