#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod audio;
mod auth;
mod config;
mod credentials;
mod data_folder;
mod dpapi;
mod explore;
mod folder_picker;
mod links;
mod media_keys;
mod queue;
mod sha1;
mod stats;
mod ui;
mod update;
mod web_sign_in;

use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::api::BrowseId;
use crate::audio::{Player, Resolver, State};
use crate::auth::Session;
use crate::config::Config;

fn main() -> anyhow::Result<()> {
    let config_path = config::default_path()?;
    let config = config::load(&config_path)?;
    let owned: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    if !args.is_empty() {
        attach_parent_console();
    }
    match args.as_slice() {
        [] => open_window(config, config_path),
        ["status"] => status(&config),
        ["login"] => login(&config),
        ["capture", name] => capture(&config, name),
        ["play", video_id] => play(&config, video_id),
        ["resolve", video_id] => resolve(&config, video_id),
        ["bench-seek", video_id, secs] => {
            let target = Duration::from_secs(secs.parse().context("seconds must be a number")?);
            let resolver = Resolver::new(config.ytdlp_path.clone());
            Ok(audio::bench::seek(&resolver, video_id, target)?)
        }
        _ => {
            bail!(
                "usage: zephyr [status | login | capture <home|playlists|liked> | play <id> | resolve <id>]"
            )
        }
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: plain Win32 call; failure (e.g. no parent console) leaves stdio unchanged.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_parent_console() {}

fn open_window(config: Config, config_path: PathBuf) -> anyhow::Result<()> {
    // An unreadable session (other Windows user, corrupt file) just means signing in again.
    let client = credentials::load(&config.data_dir()?)
        .ok()
        .flatten()
        .map(api::Client::new)
        .transpose()?;
    let player = new_player(&config).map_err(|err| err.to_string());
    ui::run(client, player, config, config_path)?;
    Ok(())
}

fn new_player(config: &Config) -> Result<Player, audio::AudioError> {
    Player::new(
        config.ytdlp_path.clone(),
        f32::from(config.volume.percent()) / 100.0,
    )
}

fn resolve(config: &Config, video_id: &str) -> anyhow::Result<()> {
    let resolver = Resolver::new(config.ytdlp_path.clone());
    let started = Instant::now();
    resolver.warm_up();
    println!("warm-up: {:?}", started.elapsed());
    let started = Instant::now();
    match resolver.resolve_native(video_id) {
        Ok(_) => println!("native: resolved in {:?}", started.elapsed()),
        Err(error) => {
            println!("native failed after {:?}: {error}", started.elapsed());
            let started = Instant::now();
            resolver.resolve(video_id)?;
            println!("yt-dlp fallback: resolved in {:?}", started.elapsed());
        }
    }
    Ok(())
}

fn play(config: &Config, video_id: &str) -> anyhow::Result<()> {
    let mut player = new_player(config)?;
    let started = Instant::now();
    player.play(video_id);
    let mut last = State::Idle;
    loop {
        let status = player.status();
        if status.state != last {
            println!(
                "{:>6.2}s {:?} position={:.1}s duration={:?}",
                started.elapsed().as_secs_f32(),
                status.state,
                status.position.as_secs_f32(),
                status.duration
            );
            last = status.state.clone();
        }
        match status.state {
            State::Ended => return Ok(()),
            State::Failed(error) => bail!(error),
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn status(config: &Config) -> anyhow::Result<()> {
    let signed_in = credentials::load(&config.data_dir()?)?.is_some();
    println!(
        "zephyr: volume {}%, image cache {} MB, signed in: {signed_in}",
        config.volume.percent(),
        config.image_cache_mb
    );
    Ok(())
}

fn login(config: &Config) -> anyhow::Result<()> {
    println!(
        "Paste the request headers from music.youtube.com, then press Enter on an empty line:"
    );
    let pasted = read_until_blank_line()?;
    let session = Session::from_pasted_headers(&pasted)?;
    let path = credentials::save(&config.data_dir()?, &session)?;
    println!("Session encrypted and saved to {}", path.display());
    Ok(())
}

fn read_until_blank_line() -> io::Result<String> {
    let mut text = String::new();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            if text.is_empty() {
                continue;
            }
            break;
        }
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

fn capture(config: &Config, name: &str) -> anyhow::Result<()> {
    let is_raw_id = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let id = match BrowseId::parse(name) {
        Some(known) => known.as_str(),
        None if is_raw_id && !name.is_empty() => name,
        None => bail!("unknown page `{name}`"),
    };
    let dir = config.data_dir()?;
    let session = credentials::load(&dir)?.context("not signed in; run `zephyr login` first")?;
    let client = api::Client::new(session)?;
    let body = runtime()?.block_on(client.browse(id))?;
    let path = dir.join(data_folder::CAPTURES).join(format!("{name}.json"));
    write_file(&path, &body)?;
    println!("Saved {} bytes to {}", body.len(), path.display());
    Ok(())
}

fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn write_file(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
