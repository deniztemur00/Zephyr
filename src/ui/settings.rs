use std::path::Path;

use iced::widget::{button, column, container, row, scrollable, slider, text, toggler};
use iced::{Alignment, Element, Length};

use super::{Message, theme};
use crate::audio::{self, Details, Source};
use crate::config::{Config, ThemeName};
use crate::update::Status;

const SWATCH_WIDTH: f32 = 200.0;
const REPO: &str = "https://github.com/deniztemur00/Zephyr";
const ISSUES: &str = "https://github.com/deniztemur00/Zephyr/issues";
const EMAIL: &str = "deniztemur00@gmail.com";
const MAILTO: &str = "mailto:deniztemur00@gmail.com";

pub fn view<'a>(
    config: &'a Config,
    config_path: &'a Path,
    account: Element<'a, Message>,
    data_folder: Element<'a, Message>,
    audio: Element<'a, Message>,
    update: &'a Status,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let sections = column![
        section("Account", account),
        section("Data folder", data_folder),
        section("Appearance", palettes(config.theme)),
        section("Playback", playback(config)),
        section("Audio", audio),
        section("Thumbnails", thumbnails(config)),
        section("About", about(config, config_path, update)),
    ]
    .spacing(30)
    .push(error.map(|error| text(error).size(13).style(theme::danger)));
    scrollable(sections).spacing(8.0).into()
}

fn section<'a>(title: &'a str, body: Element<'a, Message>) -> Element<'a, Message> {
    column![text(title).size(21), body].spacing(12).into()
}

fn palettes<'a>(selected: ThemeName) -> Element<'a, Message> {
    let cards = ThemeName::ALL.into_iter().map(|name| {
        let swatch = theme::swatch(name);
        let chips = row(swatch.chips.iter().map(|&color| {
            container(text(""))
                .width(36)
                .height(36)
                .style(theme::chip(color))
                .into()
        }))
        .spacing(8);
        button(column![text(swatch.label).size(15), chips].spacing(10))
            .width(SWATCH_WIDTH)
            .padding(14)
            .style(theme::card_with(name == selected))
            .on_press(Message::SetTheme(name))
            .into()
    });
    row(cards).spacing(14).wrap().vertical_spacing(14).into()
}

fn playback(config: &Config) -> Element<'_, Message> {
    column![
        toggler(config.media_keys)
            .label("Capture media keys system-wide (Play/Pause, Next, Previous)")
            .on_toggle(Message::SetMediaKeys),
        note("While on, your mouse and keyboard media keys control only this app, even in games and browsers."),
    ]
    .spacing(6)
    .into()
}

pub fn audio<'a>(
    output: Result<(u32, usize), &'a str>,
    details: Option<Details>,
) -> Element<'a, Message> {
    let device = match output {
        Ok((rate, channels)) => format!("{} {}", khz(rate), channel_name(channels)),
        Err(error) => format!("unavailable ({error})"),
    };
    let pipeline = [
        "Decoder: Symphonia (pure Rust): MP4 / fragmented-MP4 demuxer and AAC decoder".to_owned(),
        "Preferred stream: itag 140, AAC-LC at about 128 kbps".to_owned(),
        format!(
            "Resampler: rubato FFT resampler in {}-frame chunks, used only when the stream and device rates differ",
            audio::RESAMPLE_CHUNK_FRAMES
        ),
        format!(
            "Output: Windows audio (WASAPI, shared mode) via cpal at {device}, 32-bit float, {} s buffer; volume is applied in software",
            audio::BUFFER_SECONDS
        ),
    ];
    let playing = details.map_or_else(
        || vec!["Nothing playing. Details of the current song appear here.".to_owned()],
        |details| now_playing(&details),
    );
    column![
        column(pipeline.into_iter().map(|line| note(line).into())).spacing(4),
        text("Now playing").size(15),
        column(playing.into_iter().map(|line| note(line).into())).spacing(4),
    ]
    .spacing(10)
    .into()
}

fn now_playing(details: &Details) -> Vec<String> {
    let format = &details.format;
    let codec = match (format.codec_name(), format.codec.as_deref()) {
        (Some(name), Some(raw)) if name != raw => format!("{name} ({raw})"),
        (Some(name), _) => name.to_owned(),
        (None, _) => "unknown codec".to_owned(),
    };
    let stream = [
        Some(codec),
        format.itag.as_ref().map(|itag| format!("itag {itag}")),
        format.kbps.map(|kbps| format!("{kbps:.0} kbps average")),
    ];
    let source = match format.source {
        Source::Native => "YouTube directly (visionOS client)",
        Source::YtDlp => "yt-dlp (fallback)",
    };
    let conversion = if details.sample_rate == details.output_rate {
        "no resampling needed".to_owned()
    } else {
        format!("resampled to {} by rubato", khz(details.output_rate))
    };
    vec![
        format!(
            "Stream: {}",
            stream.into_iter().flatten().collect::<Vec<_>>().join(" · ")
        ),
        format!("Found by: {source}"),
        format!(
            "Decoded: {} {}, {conversion}, played as {}",
            khz(details.sample_rate),
            channel_name(details.channels),
            channel_name(details.output_channels)
        ),
    ]
}

fn khz(rate: u32) -> String {
    format!("{} kHz", f64::from(rate) / 1000.0)
}

fn channel_name(channels: usize) -> String {
    match channels {
        1 => "mono".to_owned(),
        2 => "stereo".to_owned(),
        n => format!("{n} channels"),
    }
}

fn thumbnails(config: &Config) -> Element<'_, Message> {
    let budget = row![
        text(format!(
            "Memory for thumbnails: {} MB",
            config.image_cache_mb
        ))
        .size(14),
        slider(
            1.0..=64.0,
            config.image_cache_mb as f32,
            Message::SetCacheMb
        )
        .on_release(Message::SaveSettings)
        .step(1.0_f32)
        .width(220),
    ]
    .spacing(16)
    .align_y(Alignment::Center);
    column![
        toggler(config.thumbnails)
            .label("Show thumbnails")
            .on_toggle(Message::SetThumbnails),
        budget,
        note(
            "Only on-screen thumbnails are fetched; the oldest are dropped when the budget is full."
        ),
    ]
    .spacing(10)
    .into()
}

pub fn signed_in<'a>(can_sign_out: bool) -> Element<'a, Message> {
    column![
        note("Signed in. Your session is encrypted with your Windows account and stored only on this PC."),
        button(text("Sign out").size(14))
            .padding([8, 16])
            .style(theme::pill)
            .on_press_maybe(can_sign_out.then_some(Message::SignOut)),
        note("Signing out deletes the saved session and the sign-in window's saved login, and clears this account's lists from memory."),
    ]
    .spacing(10)
    .into()
}

const PRIVACY_FACTS: [&str; 5] = [
    "You type your password into Google's own page; this app never sees it.",
    "The session is checked once with YouTube, then encrypted with your Windows account (DPAPI) and stored only on this PC.",
    "It is sent only to music.youtube.com and www.youtube.com, enforced in code. No other server ever receives it; there is no telemetry.",
    "It is never shown, logged or written anywhere in plain text. Sign out deletes it.",
    "To cut access everywhere at any time: Google Account > Security > Your devices > sign out.",
];

pub fn sign_in<'a>(status: Option<&'a str>, error: Option<&'a str>) -> Element<'a, Message> {
    let google = button(text(status.unwrap_or("Sign in with Google")).size(14))
        .padding([10, 20])
        .style(theme::pill)
        .on_press_maybe(status.is_none().then_some(Message::SignInWithGoogle));
    let privacy = column![text("What happens to your sign-in").size(15)]
        .extend(
            PRIVACY_FACTS
                .iter()
                .map(|fact| note(format!("• {fact}")).into()),
        )
        .spacing(4);
    column![
        google,
        note("Opens Google's sign-in page in a small window. Signing in there signs this app in."),
    ]
    .push(error.map(|error| text(error).size(13).style(theme::danger)))
    .push(container(privacy).padding(14).style(theme::panel))
    .spacing(16)
    .into()
}

pub fn data_folder<'a>(dir: String, moving: bool, can_change: bool) -> Element<'a, Message> {
    let change = button(text(if moving { "Moving…" } else { "Change…" }).size(14))
        .padding([8, 16])
        .style(theme::pill)
        .on_press_maybe(can_change.then_some(Message::PickDataDir));
    column![
        row![text(dir).size(14), change]
            .spacing(16)
            .align_y(Alignment::Center),
        note("Your sign-in, the sign-in window's saved login and view-count history are kept here. Changing the folder moves them; the app itself is just Zephyr.exe."),
    ]
    .spacing(8)
    .into()
}

fn about<'a>(
    config: &'a Config,
    config_path: &'a Path,
    update: &'a Status,
) -> Element<'a, Message> {
    let lines = [
        format!(
            "Streams: direct from YouTube, falling back to yt-dlp at {}",
            config.ytdlp_path.display()
        ),
        format!("Settings file: {}", config_path.display()),
    ];
    let links = row![
        link_button("GitHub", REPO),
        link_button("Report a bug", ISSUES),
        link_button(EMAIL, MAILTO),
    ]
    .spacing(10)
    .wrap()
    .vertical_spacing(10);
    column![
        text(format!("Zephyr {}", env!("CARGO_PKG_VERSION"))).size(15),
        update_row(update),
        links,
        column(lines.into_iter().map(|line| note(line).into())).spacing(4),
    ]
    .spacing(10)
    .width(Length::Fill)
    .into()
}

fn update_row(update: &Status) -> Element<'_, Message> {
    let (message, action) = match update {
        Status::Checking => ("Checking for updates…", None),
        Status::Current => ("You have the latest version.", Some("Check again")),
        Status::Failed => ("Could not check for updates.", Some("Try again")),
        Status::Available(release) => {
            return row![
                note("A new version is available."),
                button(text(format!("Download Zephyr {}", release.version)).size(14))
                    .padding([8, 16])
                    .style(theme::pill)
                    .on_press(Message::OpenLink(release.url.clone())),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into();
        }
    };
    row![note(message)]
        .push(action.map(|label| {
            button(text(label).size(13))
                .padding([6, 12])
                .style(theme::pill)
                .on_press(Message::CheckForUpdates)
        }))
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
}

fn link_button<'a>(label: &'a str, link: &str) -> Element<'a, Message> {
    button(text(label).size(14))
        .padding([8, 16])
        .style(theme::pill)
        .on_press(Message::OpenLink(link.to_owned()))
        .into()
}

fn note<'a>(value: impl text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(value).size(13).style(theme::muted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::StreamFormat;

    #[test]
    fn formats_rates_and_channels() {
        assert_eq!(khz(44_100), "44.1 kHz");
        assert_eq!(khz(48_000), "48 kHz");
        assert_eq!(channel_name(1), "mono");
        assert_eq!(channel_name(2), "stereo");
        assert_eq!(channel_name(6), "6 channels");
    }

    #[test]
    fn describes_the_current_stream() {
        let details = Details {
            format: StreamFormat {
                source: Source::Native,
                itag: Some("140".into()),
                codec: Some("mp4a.40.2".into()),
                kbps: Some(129.4),
            },
            sample_rate: 44_100,
            channels: 2,
            output_rate: 48_000,
            output_channels: 2,
        };
        assert_eq!(
            now_playing(&details),
            [
                "Stream: AAC-LC (mp4a.40.2) · itag 140 · 129 kbps average",
                "Found by: YouTube directly (visionOS client)",
                "Decoded: 44.1 kHz stereo, resampled to 48 kHz by rubato, played as stereo",
            ]
        );
        let bare = Details {
            format: StreamFormat::default(),
            output_rate: 44_100,
            ..details
        };
        assert_eq!(now_playing(&bare)[0], "Stream: unknown codec");
        assert!(now_playing(&bare)[2].contains("no resampling needed"));
    }
}
