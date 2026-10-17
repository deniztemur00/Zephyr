mod breeze;
mod settings;
mod theme;
mod thumbnails;
mod track_list;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use iced::alignment::Horizontal;
use iced::futures::channel::oneshot;
use iced::widget::image::Handle;
use iced::widget::{button, column, container, image, row, scrollable, slider, text};
use iced::{Alignment, Element, Font, Length, Subscription, Task, Theme};

use self::thumbnails::Thumbnails;
use self::track_list::{Extras, Scroll, TrackList};
use crate::api::{
    ApiError, Card, Client, Genre, HomePage, More, Shelf, StatsPage, Target, Track, TrackPage,
    WebStat,
};
use crate::audio::{Player, State, Status};
use crate::auth::{self, Session};
use crate::config::{self, Config, ThemeName, Volume};
use crate::credentials;
use crate::data_folder;
use crate::explore;
use crate::folder_picker;
use crate::links;
use crate::media_keys::{self, MediaKey};
use crate::queue::{Queue, Repeat, Rng};
use crate::stats::{self, History, Popularity};
use crate::update;
use crate::web_sign_in::{self, Mode};

const BREEZE_PNG: &[u8] = include_bytes!("../assets/zephyr_breeze.png");
const BREEZE_WIDTH: f32 = 176.0;
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/zephyr_icon.png");
const CARD_WIDTH: f32 = 184.0;
const CARD_WIDTH_WITH_ART: f32 = 260.0;
const CARD_HEIGHT: f32 = 120.0;
const CARD_ART: f32 = 72.0;
const NOW_PLAYING_ART: f32 = 48.0;
const ICON_SLOT: f32 = 32.0;
const CARD_TEXT_CHARS: usize = 48;
const ROW_TEXT_CHARS: usize = 60;
const SCROLLBAR_GAP: f32 = 8.0;
const TICK: Duration = Duration::from_millis(250);
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
const LIKED_ID: &str = "VLLM";
const RECOMMENDATION_SEEDS: usize = 4;
const MAX_STATS: usize = 20_000;
const MAX_HOME_PAGES: usize = 5;
// shortcut: Windows-only icon font; bundle an icon font or SVGs for Linux.
const ICON_FONT: Font = Font::with_name("Segoe MDL2 Assets");
const ICON_PREVIOUS: char = '\u{E892}';
const ICON_NEXT: char = '\u{E893}';
const ICON_REPEAT_ALL: char = '\u{E8EE}';
const ICON_REPEAT_ONE: char = '\u{E8ED}';
const ICON_PLAY: char = '\u{E768}';
const ICON_PAUSE: char = '\u{E769}';
const ICON_VOLUME: char = '\u{E767}';
const ICON_BACK: char = '\u{E72B}';
const ICON_SHUFFLE: char = '\u{E8B1}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Explore,
    Recommendations,
    Liked,
    Library,
    Podcasts,
    Settings,
}

impl Page {
    const ALL: [Page; 7] = [
        Page::Home,
        Page::Explore,
        Page::Recommendations,
        Page::Liked,
        Page::Library,
        Page::Podcasts,
        Page::Settings,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Explore => "Explore",
            Page::Recommendations => "Recommendations",
            Page::Liked => "Liked songs",
            Page::Library => "Library",
            Page::Podcasts => "Podcasts",
            Page::Settings => "Settings",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Liked,
    Opened,
    Explore,
    Recommendation(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKey {
    Liked,
    Opened(String),
}

enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    fn needs_fetch(&self) -> bool {
        matches!(self, Load::Idle | Load::Failed(_))
    }

    fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }

    fn ready_mut(&mut self) -> Option<&mut T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

impl<T> From<Result<T, String>> for Load<T> {
    fn from(result: Result<T, String>) -> Self {
        result.map_or_else(Load::Failed, Load::Ready)
    }
}

struct Opened {
    browse_id: String,
    title: String,
    list: Load<TrackList>,
}

#[derive(Debug, Clone)]
struct Recommendation {
    title: String,
    tracks: Vec<Track>,
}

type Paged = Result<(Vec<Track>, Option<More>), String>;

#[derive(Debug, Clone)]
enum Message {
    Navigate(Page),
    HomeLoaded(Result<HomePage, String>),
    HomeMore(Result<HomePage, String>),
    GenresLoaded(Result<Vec<Genre>, String>),
    SelectGenre(usize),
    ExploreLoaded {
        genre: usize,
        result: Result<Vec<WebStat>, String>,
    },
    PlaylistsLoaded(Result<Vec<Card>, String>),
    PodcastsLoaded(Result<Vec<Card>, String>),
    RecommendationsLoaded(Result<Vec<Recommendation>, String>),
    FirstPage {
        key: ListKey,
        result: Result<TrackPage, String>,
    },
    NextPage {
        key: ListKey,
        requested: More,
        result: Paged,
    },
    Open {
        browse_id: String,
        title: String,
    },
    Back,
    Scrolled(Scroll),
    PlayTrack {
        source: Source,
        index: usize,
    },
    PlayCard {
        shelf: usize,
        card: usize,
    },
    ShuffleAll(Source),
    ToggleShuffle,
    CycleRepeat,
    TogglePause,
    Next,
    Previous,
    Tick,
    SeekDrag(f32),
    SeekRelease,
    Volume(f32),
    MediaKey(media_keys::Event),
    SetTheme(ThemeName),
    SetMediaKeys(bool),
    SetThumbnails(bool),
    SetCacheMb(f32),
    SaveSettings,
    SignInWithGoogle,
    WebSignedIn(Result<Pasted, String>),
    SessionExpired,
    SessionRenewed(Result<Pasted, String>),
    RenewalChecked(Result<(), String>),
    SignedIn(Result<HomePage, String>),
    SignOut,
    PickDataDir,
    OpenLink(String),
    CheckForUpdates,
    UpdateChecked(Result<Option<update::Release>, String>),
    DataDirPicked(Result<Option<PathBuf>, String>),
    DataCopied(Result<PathBuf, String>),
    OldDataRemoved(Result<(), String>),
    Thumbnail {
        url: String,
        result: Result<Vec<u8>, String>,
    },
    Stats {
        key: ListKey,
        playlist: String,
        offset: usize,
        result: Result<StatsPage, String>,
    },
}

struct App {
    theme: Theme,
    page: Page,
    opened: Option<Opened>,
    scroll: Scroll,
    client: Option<Arc<Client>>,
    player: Result<Player, String>,
    rng: Rng,
    queue: Option<Queue>,
    queue_source: Option<ListKey>,
    repeat: Repeat,
    status: Option<Status>,
    dragging: Option<f32>,
    volume: f32,
    config: Config,
    config_path: PathBuf,
    settings_error: Option<String>,
    update: update::Status,
    media_keys_note: Option<String>,
    home: Load<Vec<Shelf>>,
    home_pages: usize,
    genres: Load<Vec<Genre>>,
    genre: Option<usize>,
    explore: Load<Vec<Track>>,
    recommendations: Load<Vec<Recommendation>>,
    playlists: Load<Vec<Card>>,
    podcasts: Load<Vec<Card>>,
    liked: Load<TrackList>,
    http: Option<reqwest::Client>,
    thumbnails: Thumbnails,
    history: History,
    stats: HashMap<String, Popularity>,
    stats_path: Option<PathBuf>,
    signing_in: Option<Arc<Client>>,
    sign_in_error: Option<String>,
    web_sign_in_open: bool,
    moving_data: bool,
    renewing: Option<Renewal>,
    breeze: Vec<Handle>,
    breeze_frame: usize,
}

// A silent session renewal in progress; `Checking` holds the new client until YouTube accepts it.
enum Renewal {
    Reading,
    Checking(Arc<Client>),
}

// Holds a cookie in transit; Debug never prints it and the bytes are wiped on drop.
#[derive(Clone)]
struct Pasted(String);

impl std::fmt::Debug for Pasted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pasted(<redacted>)")
    }
}

impl Drop for Pasted {
    fn drop(&mut self) {
        auth::wipe(&mut self.0);
    }
}

pub fn run(
    client: Option<Client>,
    player: Result<Player, String>,
    config: Config,
    config_path: PathBuf,
) -> iced::Result {
    let client = client.map(Arc::new);
    // iced may call boot more than once; the player can only be handed over once.
    let player = Mutex::new(Some(player));
    iced::application(
        move || {
            let player = player
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .unwrap_or_else(|| Err("audio player already in use".into()));
            App::boot(client.clone(), player, config.clone(), config_path.clone())
        },
        App::update,
        App::view,
    )
    .title("Zephyr")
    .theme(|app: &App| app.theme.clone())
    .subscription(App::subscription)
    .window(iced::window::Settings {
        size: iced::Size::new(1180.0, 760.0),
        icon: window_icon(),
        ..iced::window::Settings::default()
    })
    .run()
}

// For blocking work that needs a thread of its own (COM dialogs, file copies).
fn on_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    message: impl FnOnce(Result<T, String>) -> Message + Send + 'static,
) -> Task<Message> {
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    Task::perform(
        async move {
            receiver
                .await
                .unwrap_or_else(|_| Err("a background task stopped unexpectedly".into()))
        },
        message,
    )
}

// Every API call goes through here, so an expired session is noticed whichever page hits it.
fn perform<T: Send + 'static>(
    request: impl Future<Output = Result<T, ApiError>> + Send + 'static,
    message: impl FnOnce(Result<T, String>) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(request, move |result| {
        let expired = matches!(result, Err(ApiError::SignedOut));
        (message(result.map_err(|err| err.to_string())), expired)
    })
    .then(|(reply, expired)| {
        let reply = Task::done(reply);
        if expired {
            reply.chain(Task::done(Message::SessionExpired))
        } else {
            reply
        }
    })
}

impl App {
    fn boot(
        client: Option<Arc<Client>>,
        player: Result<Player, String>,
        config: Config,
        config_path: PathBuf,
    ) -> (Self, Task<Message>) {
        let config_mb = config.image_cache_mb;
        let stats_path = config.data_dir().ok().map(|dir| dir.join(stats::FILE));
        let mut app = Self {
            theme: theme::build(config.theme),
            page: Page::Home,
            opened: None,
            scroll: Scroll::default(),
            client,
            player,
            rng: Rng::from_clock(),
            queue: None,
            queue_source: None,
            repeat: Repeat::Off,
            status: None,
            dragging: None,
            volume: f32::from(config.volume.percent()),
            config,
            config_path,
            settings_error: None,
            update: update::Status::Checking,
            media_keys_note: None,
            home: Load::Idle,
            home_pages: 0,
            genres: Load::Idle,
            genre: None,
            explore: Load::Idle,
            recommendations: Load::Idle,
            playlists: Load::Idle,
            podcasts: Load::Idle,
            liked: Load::Idle,
            http: reqwest::Client::builder().build().ok(),
            thumbnails: Thumbnails::new(config_mb),
            history: stats_path
                .as_deref()
                .and_then(|path| History::load(path).ok())
                .unwrap_or_default(),
            stats: HashMap::new(),
            stats_path,
            signing_in: None,
            sign_in_error: None,
            web_sign_in_open: false,
            moving_data: false,
            renewing: None,
            breeze: breeze::frames(BREEZE_PNG),
            breeze_frame: 0,
        };
        let home = app.fetch(Page::Home);
        let task = Task::batch([home, app.check_for_updates()]);
        (app, task)
    }

    fn subscription(&self) -> Subscription<Message> {
        let keys = self
            .config
            .media_keys
            .then(|| Subscription::run(media_keys::listen).map(Message::MediaKey));
        let tick = self
            .queue
            .is_some()
            .then(|| iced::time::every(TICK).map(|_| Message::Tick));
        Subscription::batch(keys.into_iter().chain(tick))
    }

    fn check_for_updates(&mut self) -> Task<Message> {
        let Some(http) = self.http.clone() else {
            self.update = update::Status::Failed;
            return Task::none();
        };
        self.update = update::Status::Checking;
        Task::perform(update::check(http), Message::UpdateChecked)
    }

    fn save_settings(&mut self) {
        self.settings_error = config::save(&self.config_path, &self.config)
            .err()
            .map(|err| err.to_string());
    }

    fn media_key(&mut self, event: media_keys::Event) {
        match event {
            media_keys::Event::Pressed(MediaKey::PlayPause) => {
                if let (Ok(player), Some(_)) = (&self.player, &self.queue) {
                    player.toggle_pause();
                }
            }
            media_keys::Event::Pressed(MediaKey::Next) => self.skip(self.repeat.for_skip()),
            media_keys::Event::Pressed(MediaKey::Previous) => {
                if self.queue.is_some() {
                    self.previous();
                }
            }
            media_keys::Event::Unavailable(keys) => {
                self.media_keys_note =
                    Some(format!("Another app holds these media keys: {keys:?}"));
            }
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        Task::batch([task, self.fetch_thumbnails()])
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Navigate(page) => {
                self.page = page;
                self.opened = None;
                self.scroll = Scroll::default();
                return self.fetch(page);
            }
            Message::HomeLoaded(result) => {
                let (shelves, next) = split_home(result);
                self.home = shelves.map_err(explain_auth_error).into();
                self.home_pages = 1;
                return self.more_home(next);
            }
            Message::HomeMore(result) => {
                let (shelves, next) = split_home(result);
                if let (Some(home), Ok(shelves)) = (self.home.ready_mut(), shelves) {
                    home.extend(shelves);
                }
                self.home_pages += 1;
                return self.more_home(next);
            }
            Message::GenresLoaded(result) => self.genres = result.into(),
            Message::SelectGenre(index) => return self.select_genre(index),
            Message::ExploreLoaded { genre, result } => self.explore_loaded(genre, result),
            Message::PlaylistsLoaded(result) => self.playlists = result.into(),
            Message::PodcastsLoaded(result) => self.podcasts = result.into(),
            Message::RecommendationsLoaded(result) => self.recommendations = result.into(),
            Message::FirstPage { key, result } => return self.first_page(key, result),
            Message::NextPage {
                key,
                requested,
                result,
            } => return self.next_page(key, &requested, result),
            Message::Open { browse_id, title } => return self.open(browse_id, title),
            Message::Back => {
                self.opened = None;
                self.scroll = Scroll::default();
            }
            Message::Scrolled(scroll) => self.scroll = scroll,
            Message::PlayTrack { source, index } => {
                let tracks = self.source_tracks(source).to_vec();
                self.start_queue(Queue::new(tracks, index), self.list_key(source));
            }
            Message::PlayCard { shelf, card } => {
                let track = self
                    .home
                    .ready()
                    .and_then(|shelves| shelves.get(shelf)?.cards.get(card))
                    .and_then(card_track);
                self.start_queue(track.and_then(|track| Queue::new(vec![track], 0)), None);
            }
            Message::ShuffleAll(source) => {
                let tracks = self.source_tracks(source).to_vec();
                let queue = Queue::shuffled(tracks, &mut self.rng);
                self.start_queue(queue, self.list_key(source));
            }
            Message::ToggleShuffle => {
                if let Some(queue) = self.queue.as_mut() {
                    queue.set_shuffle(!queue.is_shuffled(), &mut self.rng);
                    self.prefetch_upcoming();
                }
            }
            Message::TogglePause => {
                if let Ok(player) = &self.player {
                    player.toggle_pause();
                }
            }
            Message::CycleRepeat => {
                self.repeat = self.repeat.cycle();
                self.prefetch_upcoming();
            }
            Message::Next => self.skip(self.repeat.for_skip()),
            Message::Previous => self.previous(),
            Message::Tick => self.tick(),
            Message::SeekDrag(secs) => self.dragging = Some(secs),
            Message::SeekRelease => {
                if let (Some(secs), Ok(player)) = (self.dragging.take(), &self.player) {
                    player.seek(Duration::from_secs_f32(secs));
                    self.status = Some(player.status());
                }
            }
            Message::Volume(percent) => {
                self.volume = percent;
                self.config.volume = Volume::from_percent(percent);
                if let Ok(player) = &self.player {
                    player.set_volume(percent / 100.0);
                }
            }
            Message::MediaKey(event) => self.media_key(event),
            Message::SetTheme(name) => {
                self.config.theme = name;
                self.theme = theme::build(name);
                self.save_settings();
            }
            Message::SetMediaKeys(on) => {
                self.config.media_keys = on;
                self.media_keys_note = None;
                self.save_settings();
            }
            Message::SetThumbnails(on) => {
                self.config.thumbnails = on;
                self.save_settings();
            }
            Message::SetCacheMb(megabytes) => {
                self.config.image_cache_mb = megabytes.round().clamp(1.0, 64.0) as u32;
                self.thumbnails.set_budget(self.config.image_cache_mb);
            }
            Message::Thumbnail { url, result } => self.thumbnails.finish(url, result),
            Message::Stats {
                key,
                playlist,
                offset,
                result,
            } => return self.stats_page(key, playlist, offset, result),
            Message::SaveSettings => self.save_settings(),
            Message::SignInWithGoogle => return self.open_web_sign_in(),
            Message::WebSignedIn(result) => return self.web_signed_in(result),
            Message::SessionExpired => return self.renew_session(),
            Message::SessionRenewed(result) => return self.session_renewed(result),
            Message::RenewalChecked(result) => return self.renewal_checked(result),
            Message::SignedIn(result) => return self.signed_in(result),
            Message::SignOut => self.sign_out(),
            Message::PickDataDir => return self.pick_data_dir(),
            Message::OpenLink(link) => self.settings_error = links::open_link(&link).err(),
            Message::CheckForUpdates => return self.check_for_updates(),
            Message::UpdateChecked(result) => {
                self.update = match result {
                    Ok(Some(release)) => update::Status::Available(release),
                    Ok(None) => update::Status::Current,
                    Err(_) => update::Status::Failed,
                };
            }
            Message::DataDirPicked(result) => return self.data_dir_picked(result),
            Message::DataCopied(result) => return self.data_copied(result),
            Message::OldDataRemoved(result) => {
                self.moving_data = false;
                self.settings_error = result.err();
            }
        }
        Task::none()
    }

    fn verify_cookie(&mut self, cookie: Pasted) -> Task<Message> {
        let client = Session::from_cookie(&cookie.0)
            .map_err(|err| err.to_string())
            .and_then(|session| Client::new(session).map_err(|err| err.to_string()));
        drop(cookie);
        match client {
            Err(error) => {
                self.sign_in_error = Some(error);
                Task::none()
            }
            Ok(client) => {
                let client = Arc::new(client);
                self.signing_in = Some(client.clone());
                self.sign_in_error = None;
                perform(async move { client.home().await }, Message::SignedIn)
            }
        }
    }

    fn open_web_sign_in(&mut self) -> Task<Message> {
        let profile = match self.config.data_dir() {
            Ok(dir) => credentials::web_profile_dir(&dir),
            Err(err) => {
                self.sign_in_error = Some(err.to_string());
                return Task::none();
            }
        };
        self.web_sign_in_open = true;
        self.sign_in_error = None;
        on_thread(
            move || {
                web_sign_in::cookie_from_sign_in(&profile, Mode::Interactive)
                    .map(Pasted)
                    .map_err(|err| err.to_string())
            },
            Message::WebSignedIn,
        )
    }

    fn web_signed_in(&mut self, result: Result<Pasted, String>) -> Task<Message> {
        self.web_sign_in_open = false;
        match result {
            Ok(cookie) => self.verify_cookie(cookie),
            Err(error) => {
                self.sign_in_error = Some(error);
                Task::none()
            }
        }
    }

    // The snapshot cookie goes stale; the sign-in window's own profile stays signed in to
    // Google, so a hidden window can fetch a fresh one without asking the user.
    fn renew_session(&mut self) -> Task<Message> {
        // A data move holds the sign-in profile; the next expired response retries.
        if self.renewing.is_some() || self.client.is_none() || self.moving_data {
            return Task::none();
        }
        let profile = match self.config.data_dir() {
            Ok(dir) => credentials::web_profile_dir(&dir),
            Err(err) => return self.renewal_failed(err.to_string()),
        };
        self.renewing = Some(Renewal::Reading);
        on_thread(
            move || {
                web_sign_in::cookie_from_sign_in(&profile, Mode::Silent)
                    .map(Pasted)
                    .map_err(|err| err.to_string())
            },
            Message::SessionRenewed,
        )
    }

    fn session_renewed(&mut self, result: Result<Pasted, String>) -> Task<Message> {
        if self.renewing.is_none() {
            return Task::none();
        }
        let client = result.and_then(|cookie| {
            let session = Session::from_cookie(&cookie.0).map_err(|err| err.to_string())?;
            Client::new(session).map_err(|err| err.to_string())
        });
        match client {
            Ok(client) => {
                let client = Arc::new(client);
                self.renewing = Some(Renewal::Checking(client.clone()));
                perform(
                    async move { client.home().await.map(drop) },
                    Message::RenewalChecked,
                )
            }
            Err(error) => self.renewal_failed(error),
        }
    }

    fn renewal_checked(&mut self, result: Result<(), String>) -> Task<Message> {
        match (result, self.renewing.take()) {
            (Ok(()), Some(Renewal::Checking(client))) => {
                self.settings_error = self
                    .config
                    .data_dir()
                    .map_err(|err| err.to_string())
                    .and_then(|dir| {
                        credentials::save(&dir, client.session())
                            .map(drop)
                            .map_err(|err| err.to_string())
                    })
                    .err();
                self.client = Some(client);
                self.fetch(self.page)
            }
            (Err(error), Some(_)) => self.renewal_failed(error),
            _ => Task::none(),
        }
    }

    // Keeps the saved files: the user only has to finish Google's prompt in the window.
    fn renewal_failed(&mut self, reason: String) -> Task<Message> {
        self.renewing = None;
        self.client = None;
        self.reset_account();
        self.page = Page::Settings;
        self.sign_in_error = Some(format!(
            "Your YouTube session expired and could not be renewed automatically ({reason}). Sign in again."
        ));
        Task::none()
    }

    // The cookie is saved only after YouTube has accepted it.
    fn signed_in(&mut self, result: Result<HomePage, String>) -> Task<Message> {
        let Some(client) = self.signing_in.take() else {
            return Task::none();
        };
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                self.sign_in_error = Some(format!(
                    "YouTube did not accept the sign-in ({error}). Please try again."
                ));
                return Task::none();
            }
        };
        let saved = self
            .config
            .data_dir()
            .map_err(|err| err.to_string())
            .and_then(|dir| {
                credentials::save(&dir, client.session())
                    .map(drop)
                    .map_err(|err| err.to_string())
            });
        self.sign_in_error = saved
            .err()
            .map(|error| format!("Signed in for this session, but it could not be saved: {error}"));
        self.reset_account();
        self.client = Some(client);
        self.page = Page::Home;
        self.home = Load::Ready(page.shelves);
        self.home_pages = 1;
        self.more_home(page.next)
    }

    fn sign_out(&mut self) {
        if let Ok(player) = &mut self.player {
            player.stop();
        }
        self.queue = None;
        self.queue_source = None;
        self.status = None;
        self.client = None;
        self.signing_in = None;
        self.renewing = None;
        self.reset_account();
        let removed = self
            .config
            .data_dir()
            .map_err(|err| err.to_string())
            .and_then(|dir| credentials::remove(&dir).map_err(|err| err.to_string()));
        self.settings_error = removed.err();
    }

    fn reset_account(&mut self) {
        self.home = Load::Idle;
        self.home_pages = 0;
        self.genres = Load::Idle;
        self.genre = None;
        self.explore = Load::Idle;
        self.recommendations = Load::Idle;
        self.playlists = Load::Idle;
        self.podcasts = Load::Idle;
        self.liked = Load::Idle;
        self.opened = None;
        self.scroll = Scroll::default();
        self.stats.clear();
        self.thumbnails = Thumbnails::new(self.config.image_cache_mb);
    }

    fn account_view(&self) -> Element<'_, Message> {
        if self.client.is_some() {
            return settings::signed_in(!self.moving_data && self.renewing.is_none());
        }
        let status = if self.web_sign_in_open {
            Some("Waiting for sign-in…")
        } else if self.signing_in.is_some() {
            Some("Checking…")
        } else if self.moving_data {
            Some("Moving data…")
        } else {
            None
        };
        settings::sign_in(status, self.sign_in_error.as_deref())
    }

    fn audio_view(&self) -> Element<'_, Message> {
        match &self.player {
            Ok(player) => settings::audio(Ok(player.output_format()), player.details()),
            Err(error) => settings::audio(Err(error), None),
        }
    }

    fn data_folder_view(&self) -> Element<'_, Message> {
        let dir = self
            .config
            .data_dir()
            .map_or_else(|err| err.to_string(), |dir| dir.display().to_string());
        let idle = !self.moving_data
            && !self.web_sign_in_open
            && self.signing_in.is_none()
            && self.renewing.is_none();
        settings::data_folder(dir, self.moving_data, idle)
    }

    fn pick_data_dir(&mut self) -> Task<Message> {
        let current = match self.config.data_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.settings_error = Some(err.to_string());
                return Task::none();
            }
        };
        self.moving_data = true;
        on_thread(
            move || folder_picker::pick_folder(&current).map_err(|err| err.message()),
            Message::DataDirPicked,
        )
    }

    fn data_dir_picked(&mut self, picked: Result<Option<PathBuf>, String>) -> Task<Message> {
        let planned = picked.and_then(|to| {
            let from = self.config.data_dir().map_err(|err| err.to_string())?;
            Ok(to.filter(|to| *to != from).map(|to| (from, to)))
        });
        match planned {
            Ok(Some((from, to))) => {
                self.settings_error = None;
                on_thread(
                    move || {
                        data_folder::copy(&from, &to)
                            .map(|()| to)
                            .map_err(|err| err.to_string())
                    },
                    Message::DataCopied,
                )
            }
            Ok(None) => {
                self.moving_data = false;
                Task::none()
            }
            Err(error) => {
                self.moving_data = false;
                self.settings_error = Some(error);
                Task::none()
            }
        }
    }

    // The old files are deleted only after the config points at the new copies.
    fn data_copied(&mut self, copied: Result<PathBuf, String>) -> Task<Message> {
        let switched = copied.and_then(|to| {
            let from = self.config.data_dir().map_err(|err| err.to_string())?;
            let mut config = self.config.clone();
            config
                .set_data_dir(to.clone())
                .map_err(|err| err.to_string())?;
            if let Err(err) = config::save(&self.config_path, &config) {
                let _ = data_folder::remove(&to);
                return Err(err.to_string());
            }
            self.config = config;
            self.stats_path = Some(to.join(stats::FILE));
            Ok(from)
        });
        match switched {
            Ok(from) => on_thread(
                move || {
                    data_folder::remove(&from).map_err(|err| {
                        format!(
                            "Moved, but some old files in {} could not be deleted: {err}",
                            from.display()
                        )
                    })
                },
                Message::OldDataRemoved,
            ),
            Err(error) => {
                self.moving_data = false;
                self.settings_error = Some(error);
                Task::none()
            }
        }
    }

    fn fetch(&mut self, page: Page) -> Task<Message> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        match page {
            Page::Home if self.home.needs_fetch() => {
                self.home = Load::Loading;
                perform(async move { client.home().await }, Message::HomeLoaded)
            }
            Page::Library if self.playlists.needs_fetch() => {
                self.playlists = Load::Loading;
                perform(
                    async move { client.library_playlists().await },
                    Message::PlaylistsLoaded,
                )
            }
            Page::Podcasts if self.podcasts.needs_fetch() => {
                self.podcasts = Load::Loading;
                perform(
                    async move { client.library_podcasts().await },
                    Message::PodcastsLoaded,
                )
            }
            Page::Liked if self.liked.needs_fetch() => {
                self.liked = Load::Loading;
                first_page_task(client, ListKey::Liked, LIKED_ID.to_owned())
            }
            Page::Explore if self.genres.needs_fetch() => {
                self.genres = Load::Loading;
                perform(async move { client.genres().await }, Message::GenresLoaded)
            }
            Page::Recommendations if self.recommendations.needs_fetch() => {
                self.recommendations = Load::Loading;
                perform(
                    async move { recommendations(&client).await },
                    Message::RecommendationsLoaded,
                )
            }
            _ => Task::none(),
        }
    }

    fn open(&mut self, browse_id: String, title: String) -> Task<Message> {
        self.scroll = Scroll::default();
        if !has_track_list(&browse_id) {
            self.opened = Some(Opened {
                browse_id,
                title,
                list: Load::Failed("Artist pages aren't supported yet.".into()),
            });
            return Task::none();
        }
        self.opened = Some(Opened {
            browse_id: browse_id.clone(),
            title,
            list: Load::Loading,
        });
        match self.client.clone() {
            Some(client) => first_page_task(client, ListKey::Opened(browse_id.clone()), browse_id),
            None => Task::none(),
        }
    }

    fn list_mut(&mut self, key: &ListKey) -> Option<&mut Load<TrackList>> {
        match key {
            ListKey::Liked => Some(&mut self.liked),
            ListKey::Opened(browse_id) => self
                .opened
                .as_mut()
                .filter(|opened| &opened.browse_id == browse_id)
                .map(|opened| &mut opened.list),
        }
    }

    fn first_page(&mut self, key: ListKey, result: Result<TrackPage, String>) -> Task<Message> {
        let Some(slot) = self.list_mut(&key) else {
            return Task::none();
        };
        *slot = result.map(TrackList::from).into();
        // Position-matched stats must wait until every row exists; see continue_paging.
        let stats = if matched_by_position(&key) {
            Task::none()
        } else {
            self.start_stats(&key)
        };
        Task::batch([self.continue_paging(key), stats])
    }

    fn more_home(&self, next: Option<String>) -> Task<Message> {
        match (next, self.client.clone()) {
            (Some(token), Some(client)) if self.home_pages < MAX_HOME_PAGES => perform(
                async move { client.more_home(&token).await },
                Message::HomeMore,
            ),
            _ => Task::none(),
        }
    }

    fn select_genre(&mut self, index: usize) -> Task<Message> {
        let params = self
            .genres
            .ready()
            .and_then(|genres| genres.get(index))
            .map(|genre| genre.params.clone());
        let (Some(params), Some(client)) = (params, self.client.clone()) else {
            return Task::none();
        };
        self.genre = Some(index);
        self.explore = Load::Loading;
        perform(
            async move { explore_candidates(&client, &params).await },
            move |result| Message::ExploreLoaded {
                genre: index,
                result,
            },
        )
    }

    fn explore_loaded(&mut self, genre: usize, result: Result<Vec<WebStat>, String>) {
        if self.genre != Some(genre) {
            return;
        }
        let candidates = match result {
            Ok(candidates) => candidates,
            Err(error) => {
                self.explore = Load::Failed(error);
                return;
            }
        };
        let today = stats::today();
        let history = &self.history;
        let monthly = |stat: &WebStat| match history
            .popularity(&stat.video_id, stat.views, stat.age_days, today)
            .monthly
        {
            stats::Monthly::Measured(gain) => u64::try_from(gain).ok(),
            stats::Monthly::Estimated(average) => Some(average),
            stats::Monthly::Unknown => None,
        };
        let ranked = explore::rank(candidates, monthly);
        for stat in &ranked {
            let popularity =
                self.history
                    .popularity(&stat.video_id, stat.views, stat.age_days, today);
            self.history.record(&stat.video_id, stat.views, today);
            self.stats.insert(stat.video_id.clone(), popularity);
        }
        self.save_history();
        self.explore = Load::Ready(ranked.into_iter().map(explore::to_track).collect());
    }

    fn start_stats(&self, key: &ListKey) -> Task<Message> {
        match (stats_playlist(key), self.client.clone()) {
            (Some(playlist), Some(client)) => {
                let (key, id) = (key.clone(), playlist.clone());
                perform(
                    async move { client.video_stats(&id).await },
                    move |result| Message::Stats {
                        key,
                        playlist,
                        offset: 0,
                        result,
                    },
                )
            }
            _ => Task::none(),
        }
    }

    fn stats_page(
        &mut self,
        key: ListKey,
        playlist: String,
        offset: usize,
        result: Result<StatsPage, String>,
    ) -> Task<Message> {
        let Some(list) = self.list_mut(&key).and_then(|slot| slot.ready()) else {
            return Task::none();
        };
        let Ok(page) = result else {
            self.save_history();
            return Task::none();
        };
        let today = stats::today();
        let matched = match_stats(&page.stats, &list.tracks, offset, playlist == LIKED_WEB_ID);
        let seen = offset + page.stats.len();
        if self.stats.len() > MAX_STATS {
            self.stats.clear();
        }
        for (id, views, age_days) in matched {
            let popularity = self.history.popularity(&id, views, age_days, today);
            self.history.record(&id, views, today);
            self.stats.insert(id, popularity);
        }
        match (page.next, self.client.clone()) {
            (Some(token), Some(client)) if seen < track_list::MAX_TRACKS => perform(
                async move { client.more_video_stats(&token).await },
                move |result| Message::Stats {
                    key,
                    playlist,
                    offset: seen,
                    result,
                },
            ),
            _ => {
                self.save_history();
                Task::none()
            }
        }
    }

    fn save_history(&mut self) {
        if let Some(path) = &self.stats_path
            && let Err(error) = self.history.save(path)
        {
            self.settings_error = Some(error.to_string());
        }
    }

    fn fetch_thumbnails(&mut self) -> Task<Message> {
        let Some(http) = self.http.clone().filter(|_| self.config.thumbnails) else {
            return Task::none();
        };
        let visible = self.visible_thumbnails();
        let urls = self.thumbnails.wanted(visible.iter().map(String::as_str));
        Task::batch(urls.into_iter().map(|url| {
            let http = http.clone();
            let request = url.clone();
            Task::perform(thumbnails::download(http, request), move |result| {
                Message::Thumbnail {
                    url: url.clone(),
                    result,
                }
            })
        }))
    }

    fn visible_thumbnails(&self) -> Vec<String> {
        let tracks = |tracks: &[Track]| -> Vec<Option<String>> {
            tracks[track_list::visible_range(tracks.len(), self.scroll)]
                .iter()
                .map(|track| track.thumbnail.clone())
                .collect()
        };
        let cards = |cards: &[Card]| -> Vec<Option<String>> {
            cards.iter().map(|card| card.thumbnail.clone()).collect()
        };
        let mut urls = match (&self.opened, self.page) {
            (Some(opened), _) => opened
                .list
                .ready()
                .map(|l| tracks(&l.tracks))
                .unwrap_or_default(),
            (None, Page::Home) => self
                .home
                .ready()
                .map(|shelves| shelves.iter().flat_map(|s| cards(&s.cards)).collect())
                .unwrap_or_default(),
            (None, Page::Library) => self.playlists.ready().map(|c| cards(c)).unwrap_or_default(),
            (None, Page::Podcasts) => self.podcasts.ready().map(|c| cards(c)).unwrap_or_default(),
            (None, Page::Liked) => self
                .liked
                .ready()
                .map(|l| tracks(&l.tracks))
                .unwrap_or_default(),
            (None, Page::Recommendations) => self
                .recommendations
                .ready()
                .map(|sections| {
                    sections
                        .iter()
                        .flat_map(|s| s.tracks.iter().map(|t| t.thumbnail.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            (None, Page::Settings) => Vec::new(),
            (None, Page::Explore) => self
                .explore
                .ready()
                .map(|tracks| tracks.iter().map(|t| t.thumbnail.clone()).collect())
                .unwrap_or_default(),
        };
        urls.push(
            self.queue
                .as_ref()
                .and_then(|q| q.current().thumbnail.clone()),
        );
        urls.into_iter().flatten().collect()
    }

    fn extras(&self) -> Extras<'_> {
        Extras {
            art: self.config.thumbnails.then_some(&self.thumbnails),
            stats: &self.stats,
        }
    }

    fn next_page(&mut self, key: ListKey, requested: &More, result: Paged) -> Task<Message> {
        let Some(added) = self
            .list_mut(&key)
            .and_then(Load::ready_mut)
            .and_then(|list| list.apply_page(requested, result))
        else {
            return Task::none();
        };
        if self.queue_source.as_ref() == Some(&key)
            && let Some(queue) = self.queue.as_mut()
        {
            queue.extend(added, &mut self.rng);
        }
        self.continue_paging(key)
    }

    fn continue_paging(&mut self, key: ListKey) -> Task<Message> {
        let Some(more) = self
            .list_mut(&key)
            .and_then(Load::ready_mut)
            .map(|list| list.more.clone())
        else {
            return Task::none();
        };
        match (more, self.client.clone()) {
            (None, _) if matched_by_position(&key) => self.start_stats(&key),
            (Some(more), Some(client)) => {
                let requested = more.clone();
                perform(async move { client.more(&more).await }, move |result| {
                    Message::NextPage {
                        key,
                        requested,
                        result,
                    }
                })
            }
            _ => Task::none(),
        }
    }

    fn source_tracks(&self, source: Source) -> &[Track] {
        let list = match source {
            Source::Liked => self.liked.ready(),
            Source::Opened => self.opened.as_ref().and_then(|opened| opened.list.ready()),
            Source::Explore => return self.explore.ready().map_or(&[], Vec::as_slice),
            Source::Recommendation(index) => {
                return self
                    .recommendations
                    .ready()
                    .and_then(|sections| sections.get(index))
                    .map_or(&[], |section| section.tracks.as_slice());
            }
        };
        list.map_or(&[], |list| list.tracks.as_slice())
    }

    fn list_key(&self, source: Source) -> Option<ListKey> {
        match source {
            Source::Liked => Some(ListKey::Liked),
            Source::Opened => self
                .opened
                .as_ref()
                .map(|opened| ListKey::Opened(opened.browse_id.clone())),
            Source::Explore | Source::Recommendation(_) => None,
        }
    }

    fn start_queue(&mut self, queue: Option<Queue>, source: Option<ListKey>) {
        if queue.is_some() {
            self.queue = queue;
            self.queue_source = source;
            self.play_current();
        }
    }

    fn play_current(&mut self) {
        let (Ok(player), Some(queue)) = (&mut self.player, &self.queue) else {
            return;
        };
        player.play(&queue.current().video_id);
        self.status = Some(player.status());
        self.prefetch_upcoming();
    }

    fn prefetch_upcoming(&self) {
        if let (Ok(player), Some(next)) = (
            &self.player,
            self.queue
                .as_ref()
                .and_then(|queue| queue.following(self.repeat)),
        ) {
            player.prefetch(&next.video_id);
        }
    }

    fn skip(&mut self, repeat: Repeat) {
        if self
            .queue
            .as_mut()
            .and_then(|queue| queue.step(repeat))
            .is_some()
        {
            self.play_current();
        }
    }

    fn previous(&mut self) {
        let near_start = self
            .status
            .as_ref()
            .is_some_and(|status| status.position < RESTART_THRESHOLD);
        if near_start && let Some(queue) = self.queue.as_mut() {
            queue.back();
        }
        self.play_current();
    }

    fn tick(&mut self) {
        let Ok(player) = &self.player else {
            return;
        };
        let status = player.status();
        let ended = status.state == State::Ended;
        if status.state == State::Playing {
            self.breeze_frame = (self.breeze_frame + 1) % self.breeze.len().max(1);
        }
        self.status = Some(status);
        if ended {
            self.skip(self.repeat);
        }
    }

    fn playing_id(&self) -> Option<&str> {
        self.queue
            .as_ref()
            .map(|queue| queue.current().video_id.as_str())
    }

    fn view(&self) -> Element<'_, Message> {
        column![
            row![self.sidebar(), self.content()].height(Length::Fill),
            self.now_playing()
        ]
        .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let nav = Page::ALL.into_iter().map(|page| {
            let selected = page == self.page && self.opened.is_none();
            button(text(page.title()).size(15))
                .width(Length::Fill)
                .padding([10, 14])
                .style(theme::nav(selected))
                .on_press(Message::Navigate(page))
                .into()
        });
        let brand = text("Zephyr").size(30).style(theme::accent);
        let note = self
            .media_keys_note
            .as_deref()
            .map(|note| text(note).size(12).style(theme::muted));
        let items = column![brand, column(nav).spacing(6)]
            .push(iced::widget::Space::new().height(Length::Fill))
            .push(
                self.breeze
                    .get(self.breeze_frame)
                    .map(|frame| image(frame.clone()).width(BREEZE_WIDTH)),
            )
            .push(note);
        container(items.spacing(28).padding(22))
            .width(220)
            .height(Length::Fill)
            .style(theme::sidebar)
            .into()
    }

    fn content(&self) -> Element<'_, Message> {
        let page = match (&self.client, &self.opened) {
            (None, _) if self.page == Page::Settings => self.page_view(),
            (None, _) => scrollable(titled(
                heading("Sign in to YouTube Music"),
                self.account_view(),
            ))
            .spacing(SCROLLBAR_GAP)
            .into(),
            (Some(_), Some(opened)) => self.opened_view(opened),
            (Some(_), None) => self.page_view(),
        };
        container(page)
            .padding([30, 36])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn page_view(&self) -> Element<'_, Message> {
        let title = heading(self.page.title());
        let playing = self.playing_id();
        let extras = self.extras();
        let art = extras.art;
        match self.page {
            Page::Home => titled(title, loaded(&self.home, |shelves| home_view(shelves, art))),
            Page::Library => titled(
                title,
                loaded(&self.playlists, |cards| card_grid(cards, art)),
            ),
            Page::Podcasts => titled(title, loaded(&self.podcasts, |cards| card_grid(cards, art))),
            Page::Settings => titled(
                title,
                settings::view(
                    &self.config,
                    &self.config_path,
                    self.account_view(),
                    self.data_folder_view(),
                    self.audio_view(),
                    &self.update,
                    self.settings_error.as_deref(),
                ),
            ),
            Page::Liked => match self.liked.ready() {
                Some(list) => self.list_page(title, list, Source::Liked),
                None => titled(title, loaded(&self.liked, |_| text("").into())),
            },
            Page::Explore => titled(
                title,
                loaded(&self.genres, |genres| {
                    explore_view(genres, self.genre, &self.explore, playing, extras)
                }),
            ),
            Page::Recommendations => titled(
                title,
                loaded(&self.recommendations, |sections| {
                    recommendations_view(sections, playing, extras)
                }),
            ),
        }
    }

    fn opened_view<'a>(&'a self, opened: &'a Opened) -> Element<'a, Message> {
        let back = button(icon(ICON_BACK, 16.0))
            .padding(8)
            .style(theme::icon_button(false))
            .on_press(Message::Back);
        let name = opened
            .list
            .ready()
            .map_or(opened.title.as_str(), |list| list.title.as_str());
        let title = row![back, heading(name)]
            .spacing(12)
            .align_y(Alignment::Center)
            .into();
        match opened.list.ready() {
            Some(list) => self.list_page(title, list, Source::Opened),
            None => titled(title, loaded(&opened.list, |_| text("").into())),
        }
    }

    fn list_page<'a>(
        &'a self,
        title: Element<'a, Message>,
        list: &'a TrackList,
        source: Source,
    ) -> Element<'a, Message> {
        let shuffle = button(
            row![
                icon(ICON_SHUFFLE, 14.0),
                text(format!("Shuffle all {}", list.tracks.len())).size(14)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([8, 16])
        .style(theme::pill)
        .on_press_maybe(
            (list.is_complete() && !list.tracks.is_empty()).then_some(Message::ShuffleAll(source)),
        );
        let summary = row![text(list.status()).size(13).style(theme::muted), shuffle]
            .spacing(18)
            .align_y(Alignment::Center);
        column![
            title,
            summary,
            track_list::view(
                &list.tracks,
                source,
                self.playing_id(),
                self.scroll,
                self.extras()
            )
        ]
        .spacing(16)
        .into()
    }

    fn now_playing(&self) -> Element<'_, Message> {
        let bar = row![
            container(self.track_info()).width(Length::FillPortion(3)),
            container(self.transport()).width(Length::FillPortion(4)),
            container(self.volume_control())
                .width(Length::FillPortion(3))
                .align_x(Horizontal::Right),
        ]
        .spacing(24)
        .align_y(Alignment::Center);
        container(bar)
            .padding([12, 22])
            .width(Length::Fill)
            .style(theme::now_playing_bar)
            .into()
    }

    fn track_info(&self) -> Element<'_, Message> {
        let (Ok(_), Some(queue)) = (&self.player, &self.queue) else {
            let message = match &self.player {
                Err(error) => text(format!("Audio unavailable: {error}")).style(theme::danger),
                Ok(_) => text("Nothing playing").style(theme::muted),
            };
            return message.into();
        };
        let track = queue.current();
        let note = match self.status.as_ref().map(|status| &status.state) {
            Some(State::Failed(error)) => text(clip(error, ROW_TEXT_CHARS))
                .size(12)
                .style(theme::danger),
            Some(State::Loading) | None => text("Loading…").size(12).style(theme::muted),
            _ => text(clip(&track.artists, ROW_TEXT_CHARS))
                .size(13)
                .style(theme::muted),
        };
        let info = column![
            text(clip(&track.title, ROW_TEXT_CHARS))
                .size(15)
                .style(theme::accent),
            note
        ]
        .spacing(3);
        let art = self.extras().art.map(|thumbnails| {
            track_list::art(thumbnails, track.thumbnail.as_deref(), NOW_PLAYING_ART)
        });
        row![]
            .push(art)
            .push(info)
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
    }

    fn transport(&self) -> Element<'_, Message> {
        let active = self.queue.is_some().then_some(());
        let shuffled = self.queue.as_ref().is_some_and(Queue::is_shuffled);
        let paused = self
            .status
            .as_ref()
            .is_some_and(|status| status.state == State::Paused);
        let toggle_icon = if paused { ICON_PLAY } else { ICON_PAUSE };
        let buttons = row![
            icon_button(
                ICON_SHUFFLE,
                shuffled,
                active.map(|()| Message::ToggleShuffle)
            ),
            icon_button(ICON_PREVIOUS, false, active.map(|()| Message::Previous)),
            button(icon(toggle_icon, 18.0))
                .padding(14)
                .style(theme::play_button)
                .on_press_maybe(active.map(|()| Message::TogglePause)),
            icon_button(ICON_NEXT, false, active.map(|()| Message::Next)),
            icon_button(
                repeat_icon(self.repeat),
                self.repeat != Repeat::Off,
                Some(Message::CycleRepeat)
            ),
        ]
        .spacing(18)
        .align_y(Alignment::Center);
        column![buttons, self.seek_bar()]
            .spacing(6)
            .align_x(Alignment::Center)
            .into()
    }

    fn seek_bar(&self) -> Element<'_, Message> {
        let (position, total) = self.status.as_ref().map_or((0.0, 0.0), |status| {
            let total = status.duration.map_or(0.0, |d| d.as_secs_f32());
            (status.position.as_secs_f32(), total)
        });
        let shown = self.dragging.unwrap_or(position).min(total);
        let time = |secs: f32| {
            text(format_time(Duration::from_secs_f32(secs)))
                .size(12)
                .style(theme::muted)
        };
        row![
            time(shown),
            slider(0.0..=total.max(1.0), shown, Message::SeekDrag)
                .on_release(Message::SeekRelease)
                .step(1.0_f32)
                .width(Length::Fill),
            time(total),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    }

    fn volume_control(&self) -> Element<'_, Message> {
        row![
            icon(ICON_VOLUME, 16.0),
            slider(0.0..=100.0, self.volume, Message::Volume)
                .on_release(Message::SaveSettings)
                .step(1.0_f32)
                .width(110),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    }
}

// youtube.com's Liked list holds the music-video twins of YouTube Music's audio
// tracks (different ids, same order), so it is matched to rows by position.
const LIKED_WEB_ID: &str = "LM";

fn stats_playlist(key: &ListKey) -> Option<String> {
    match key {
        ListKey::Liked => Some(LIKED_WEB_ID.to_owned()),
        ListKey::Opened(browse_id) => web_playlist_id(browse_id),
    }
}

fn matched_by_position(key: &ListKey) -> bool {
    stats_playlist(key).as_deref() == Some(LIKED_WEB_ID)
}

fn match_stats(
    stats: &[WebStat],
    tracks: &[Track],
    offset: usize,
    by_position: bool,
) -> Vec<(String, u64, Option<u32>)> {
    stats
        .iter()
        .enumerate()
        .filter_map(|(index, stat)| {
            let id = if by_position {
                tracks.get(offset + index)?.video_id.clone()
            } else {
                stat.video_id.clone()
            };
            Some((id, stat.views, stat.age_days))
        })
        .collect()
}

fn explain_auth_error(error: String) -> String {
    if error.contains("401") || error.contains("403") {
        format!("{error}. Your session may have expired: sign out and sign in again in Settings.")
    } else {
        error
    }
}

fn split_home(result: Result<HomePage, String>) -> (Result<Vec<Shelf>, String>, Option<String>) {
    match result {
        Ok(page) => (Ok(page.shelves), page.next),
        Err(error) => (Err(error), None),
    }
}

async fn explore_candidates(client: &Client, params: &str) -> Result<Vec<WebStat>, ApiError> {
    let shelves = client.genre_playlists(params).await?;
    let mut candidates = Vec::new();
    for playlist in explore::pick_playlists(&shelves) {
        if let Ok(page) = client.video_stats(&playlist).await {
            candidates.extend(page.stats);
        }
    }
    Ok(candidates)
}

fn first_page_task(client: Arc<Client>, key: ListKey, browse_id: String) -> Task<Message> {
    perform(
        async move { client.tracks(&browse_id).await },
        move |result| Message::FirstPage { key, result },
    )
}

async fn recommendations(client: &Client) -> Result<Vec<Recommendation>, ApiError> {
    let playlists = client.library_playlists().await?;
    let mut seeds = vec![("Liked songs".to_owned(), "LM".to_owned())];
    seeds.extend(
        playlists
            .into_iter()
            .filter_map(|card| match card.target {
                Target::Page(id) if id.starts_with("VLPL") => {
                    Some((card.title, id[2..].to_owned()))
                }
                _ => None,
            })
            .take(RECOMMENDATION_SEEDS - 1),
    );
    let mut seen = HashSet::new();
    let mut sections = Vec::new();
    let mut last_error = None;
    for (title, playlist_id) in seeds {
        let radio = match client.radio(&playlist_id).await {
            Ok(radio) => radio,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let tracks: Vec<Track> = radio
            .into_iter()
            .filter(|track| seen.insert(track.video_id.clone()))
            .collect();
        if !tracks.is_empty() {
            sections.push(Recommendation {
                title: format!("Because you like {title}"),
                tracks,
            });
        }
    }
    match last_error {
        Some(error) if sections.is_empty() => Err(error),
        _ => Ok(sections),
    }
}

fn web_playlist_id(browse_id: &str) -> Option<String> {
    browse_id
        .strip_prefix("VL")
        .or_else(|| browse_id.strip_prefix("MPSP"))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn has_track_list(browse_id: &str) -> bool {
    ["VL", "MPRE", "MPSP"]
        .iter()
        .any(|prefix| browse_id.starts_with(prefix))
}

fn heading<'a>(title: &str) -> Element<'a, Message> {
    text(clip(title, ROW_TEXT_CHARS)).size(32).into()
}

fn titled<'a>(title: Element<'a, Message>, body: Element<'a, Message>) -> Element<'a, Message> {
    column![title, body].spacing(22).into()
}

fn icon<'a>(glyph: char, size: f32) -> Element<'a, Message> {
    text(glyph).font(ICON_FONT).size(size).into()
}

// iced is built without image codecs, so embedded PNGs are decoded with the image crate.
fn window_icon() -> Option<iced::window::Icon> {
    let icon = ::image::load_from_memory_with_format(WINDOW_ICON_PNG, ::image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = icon.dimensions();
    iced::window::icon::from_rgba(icon.into_raw(), width, height).ok()
}

fn repeat_icon(repeat: Repeat) -> char {
    match repeat {
        Repeat::One => ICON_REPEAT_ONE,
        Repeat::Off | Repeat::All => ICON_REPEAT_ALL,
    }
}

fn icon_button<'a>(glyph: char, active: bool, on_press: Option<Message>) -> Element<'a, Message> {
    button(icon(glyph, 16.0))
        .width(ICON_SLOT)
        .padding(8)
        .style(theme::icon_button(active))
        .on_press_maybe(on_press)
        .into()
}

fn loaded<'a, T>(
    state: &'a Load<T>,
    view: impl FnOnce(&'a T) -> Element<'a, Message>,
) -> Element<'a, Message> {
    match state {
        Load::Idle | Load::Loading => text("Loading…").style(theme::muted).into(),
        Load::Failed(error) => text(error.as_str()).style(theme::danger).into(),
        Load::Ready(value) => view(value),
    }
}

fn recommendations_view<'a>(
    sections: &'a [Recommendation],
    playing: Option<&str>,
    extras: Extras<'a>,
) -> Element<'a, Message> {
    if sections.is_empty() {
        return text("No recommendations yet. Like some songs or add playlists first.")
            .style(theme::muted)
            .into();
    }
    let sections = sections.iter().enumerate().map(|(index, section)| {
        column![
            text(&section.title).size(21),
            track_list::plain(
                &section.tracks,
                Source::Recommendation(index),
                playing,
                extras
            )
        ]
        .spacing(10)
        .into()
    });
    scrollable(column(sections).spacing(30))
        .spacing(SCROLLBAR_GAP)
        .into()
}

fn explore_view<'a>(
    genres: &'a [Genre],
    selected: Option<usize>,
    results: &'a Load<Vec<Track>>,
    playing: Option<&str>,
    extras: Extras<'a>,
) -> Element<'a, Message> {
    let chips = genres.iter().enumerate().map(|(index, genre)| {
        button(text(&genre.name).size(13))
            .padding([6, 12])
            .style(theme::nav(selected == Some(index)))
            .on_press(Message::SelectGenre(index))
            .into()
    });
    let chips = row(chips).spacing(8).wrap().vertical_spacing(8);
    let body: Element<'a, Message> = match selected {
        None => text("Pick a genre to see what is rising in it.")
            .style(theme::muted)
            .into(),
        Some(_) => loaded(results, |tracks| {
            if tracks.is_empty() {
                return text("Nothing rising here right now.")
                    .style(theme::muted)
                    .into();
            }
            column![
                text(
                    "Rising songs: views gained this month relative to each song's size, \
                     at most 2 per artist, so newer and smaller artists get a fair shot."
                )
                .size(13)
                .style(theme::muted),
                track_list::plain(tracks, Source::Explore, playing, extras)
            ]
            .spacing(12)
            .into()
        }),
    };
    scrollable(column![chips, body].spacing(24))
        .spacing(SCROLLBAR_GAP)
        .into()
}

fn home_view<'a>(shelves: &'a [Shelf], art: Option<&'a Thumbnails>) -> Element<'a, Message> {
    let shelves = shelves.iter().enumerate().map(|(shelf_index, shelf)| {
        let cards = shelf.cards.iter().enumerate().map(|(card_index, item)| {
            let on_press = match &item.target {
                Target::Song(_) => Message::PlayCard {
                    shelf: shelf_index,
                    card: card_index,
                },
                Target::Page(browse_id) => Message::Open {
                    browse_id: browse_id.clone(),
                    title: item.title.clone(),
                },
            };
            card(item, on_press, art)
        });
        column![
            text(&shelf.title).size(21),
            scrollable(row(cards).spacing(14))
                .horizontal()
                .spacing(SCROLLBAR_GAP)
        ]
        .spacing(12)
        .into()
    });
    scrollable(column(shelves).spacing(30))
        .spacing(SCROLLBAR_GAP)
        .into()
}

fn card_grid<'a>(cards: &'a [Card], art: Option<&'a Thumbnails>) -> Element<'a, Message> {
    let tiles = cards.iter().filter_map(|item| {
        let Target::Page(browse_id) = &item.target else {
            return None;
        };
        Some(card(
            item,
            Message::Open {
                browse_id: browse_id.clone(),
                title: item.title.clone(),
            },
            art,
        ))
    });
    let grid = row(tiles).spacing(14).wrap().vertical_spacing(14);
    scrollable(grid).spacing(SCROLLBAR_GAP).into()
}

fn card<'a>(
    card: &'a Card,
    on_press: Message,
    art: Option<&'a Thumbnails>,
) -> Element<'a, Message> {
    let body = column![
        text(clip(&card.title, CARD_TEXT_CHARS)).size(15),
        text(clip(&card.subtitle, CARD_TEXT_CHARS))
            .size(13)
            .style(theme::muted)
    ]
    .spacing(6);
    let content: Element<'a, Message> = match art {
        Some(thumbnails) => row![
            track_list::art(thumbnails, card.thumbnail.as_deref(), CARD_ART),
            body
        ]
        .spacing(12)
        .into(),
        None => body.into(),
    };
    let width = if art.is_some() {
        CARD_WIDTH_WITH_ART
    } else {
        CARD_WIDTH
    };
    button(
        container(content)
            .width(width)
            .height(CARD_HEIGHT)
            .padding(14)
            .clip(true),
    )
    .padding(0)
    .style(theme::card)
    .on_press(on_press)
    .into()
}

fn card_track(card: &Card) -> Option<Track> {
    let Target::Song(video_id) = &card.target else {
        return None;
    };
    Some(Track {
        video_id: video_id.clone(),
        title: card.title.clone(),
        artists: card.subtitle.clone(),
        album: None,
        duration: None,
        thumbnail: card.thumbnail.clone(),
    })
}

fn clip(value: &str, max_chars: usize) -> String {
    match value.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", value[..cut].trim_end()),
        None => value.to_owned(),
    }
}

fn format_time(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_short_text() {
        assert_eq!(clip("Şarkı", 5), "Şarkı");
    }

    #[test]
    fn clip_cuts_on_char_boundary() {
        assert_eq!(clip("Neşet Ertaş", 4), "Neşe…");
        assert_eq!(clip("ab cd", 3), "ab…");
    }

    #[test]
    fn formats_times() {
        assert_eq!(format_time(Duration::from_secs(0)), "0:00");
        assert_eq!(format_time(Duration::from_secs(189)), "3:09");
        assert_eq!(format_time(Duration::from_secs(11_961)), "3:19:21");
    }

    fn web_stat(id: &str, views: u64) -> WebStat {
        WebStat {
            video_id: id.into(),
            views,
            age_days: Some(10),
            title: String::new(),
            channel: String::new(),
            thumbnail: None,
        }
    }

    fn track(id: &str) -> Track {
        Track {
            video_id: id.into(),
            title: id.into(),
            artists: String::new(),
            album: None,
            duration: None,
            thumbnail: None,
        }
    }

    #[test]
    fn embedded_window_icon_decodes() {
        assert!(window_icon().is_some());
    }

    #[test]
    fn pasted_credentials_never_show_in_debug_output() {
        let pasted = Pasted("SAPISID=secret-value".into());
        assert!(!format!("{pasted:?}").contains("secret"));
        assert!(!format!("{:?}", Message::WebSignedIn(Ok(pasted))).contains("secret"));
    }

    #[test]
    fn rejected_sessions_point_to_signing_in_again() {
        assert!(
            explain_auth_error("HTTP status client error (401 Unauthorized)".into())
                .contains("sign in again")
        );
        assert_eq!(explain_auth_error("timeout".into()), "timeout");
    }

    #[test]
    fn stats_match_rows_by_id_or_by_position() {
        let stats = [
            web_stat("omv1", 10),
            web_stat("omv2", 20),
            web_stat("omv3", 30),
        ];
        let tracks = [track("a1"), track("a2"), track("a3"), track("a4")];
        let by_position = match_stats(&stats, &tracks, 1, true);
        let ids: Vec<&str> = by_position.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(ids, ["a2", "a3", "a4"]);
        assert_eq!(by_position[0].1, 10);
        let short = match_stats(&stats, &tracks[..2], 0, true);
        assert_eq!(short.len(), 2, "rows that don't exist yet are skipped");
        let by_id = match_stats(&stats, &tracks, 0, false);
        assert_eq!(by_id[2].0, "omv3");
    }

    #[test]
    fn only_liked_lists_are_matched_by_position() {
        assert!(matched_by_position(&ListKey::Liked));
        assert!(matched_by_position(&ListKey::Opened("VLLM".into())));
        assert!(!matched_by_position(&ListKey::Opened("VLPL9a".into())));
        assert!(!matched_by_position(&ListKey::Opened("MPREb_x".into())));
    }

    #[test]
    fn stats_come_from_the_matching_youtube_playlist() {
        assert_eq!(web_playlist_id("VLPL9a").as_deref(), Some("PL9a"));
        assert_eq!(web_playlist_id("VLLM").as_deref(), Some("LM"));
        assert_eq!(web_playlist_id("MPSPPLshow").as_deref(), Some("PLshow"));
        assert_eq!(web_playlist_id("MPREb_album"), None);
        assert_eq!(web_playlist_id("VL"), None);
    }

    #[test]
    fn playlists_albums_and_podcasts_open_as_track_lists() {
        assert!(has_track_list("VLPL123"));
        assert!(has_track_list("MPREb_abc"));
        assert!(has_track_list("MPSPPLshow"));
        assert!(!has_track_list("UCartist"));
    }
}
