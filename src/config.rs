use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not determine the user config directory")]
    NoConfigDir,
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("data_dir must be an absolute path, got {0}")]
    RelativeDataDir(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Volume(u8);

impl Volume {
    pub const MAX: u8 = 100;

    pub fn percent(self) -> u8 {
        self.0
    }

    pub fn from_percent(percent: f32) -> Self {
        Self(percent.round().clamp(0.0, f32::from(Self::MAX)) as u8)
    }
}

impl From<Volume> for u8 {
    fn from(volume: Volume) -> Self {
        volume.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    Autumn,
    Lemons,
    Bloom,
    Forest,
}

impl ThemeName {
    pub const ALL: [ThemeName; 4] = [
        ThemeName::Autumn,
        ThemeName::Lemons,
        ThemeName::Bloom,
        ThemeName::Forest,
    ];
}

impl TryFrom<u8> for Volume {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(format!("volume must be 0..={}, got {value}", Self::MAX))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: ThemeName,
    pub volume: Volume,
    pub media_keys: bool,
    pub thumbnails: bool,
    pub image_cache_mb: u32,
    // Older configs called this credentials_dir; it held the same files.
    #[serde(skip_serializing_if = "Option::is_none", alias = "credentials_dir")]
    data_dir: Option<PathBuf>,
    pub ytdlp_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeName::default(),
            volume: Volume(80),
            media_keys: true,
            thumbnails: true,
            image_cache_mb: 4,
            data_dir: None,
            ytdlp_path: PathBuf::from("yt-dlp"),
        }
    }
}

pub fn save(path: &Path, config: &Config) -> Result<(), ConfigError> {
    let text = toml::to_string_pretty(config)?;
    let write_err = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_err)?;
    }
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, text).map_err(write_err)?;
    fs::rename(&temporary, path).map_err(write_err)
}

impl Config {
    // Everything besides this config file lives here: session, sign-in profile, stats.
    pub fn data_dir(&self) -> Result<PathBuf, ConfigError> {
        match &self.data_dir {
            Some(dir) if dir.is_absolute() => Ok(dir.clone()),
            Some(dir) => Err(ConfigError::RelativeDataDir(dir.clone())),
            None => default_data_dir(),
        }
    }

    pub fn set_data_dir(&mut self, dir: PathBuf) -> Result<(), ConfigError> {
        self.data_dir = (dir != default_data_dir()?).then_some(dir);
        Ok(())
    }
}

fn default_data_dir() -> Result<PathBuf, ConfigError> {
    let base = BaseDirs::new().ok_or(ConfigError::NoConfigDir)?;
    Ok(base.data_local_dir().join("zephyr"))
}

fn project_dirs() -> Result<ProjectDirs, ConfigError> {
    ProjectDirs::from("", "", "zephyr").ok_or(ConfigError::NoConfigDir)
}

pub fn default_path() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.config_dir().join("config.toml"))
}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

fn parse(text: &str) -> Result<Config, ConfigError> {
    Ok(toml::from_str(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn saved_settings_parse_back_identically() {
        let config = Config {
            theme: ThemeName::Bloom,
            volume: Volume(42),
            media_keys: false,
            data_dir: Some(PathBuf::from(r"C:\zephyr-data")),
            ..Config::default()
        };
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("theme = \"bloom\""));
        assert_eq!(parse(&text).unwrap(), config);
        let without_dir = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!without_dir.contains("data_dir"));
    }

    #[test]
    fn volume_from_slider_is_clamped() {
        assert_eq!(Volume::from_percent(42.6).percent(), 43);
        assert_eq!(Volume::from_percent(250.0).percent(), 100);
        assert_eq!(Volume::from_percent(-3.0).percent(), 0);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config = parse("volume = 30").unwrap();
        assert_eq!(config.volume.percent(), 30);
        assert_eq!(config.image_cache_mb, Config::default().image_cache_mb);
    }

    #[test]
    fn data_dir_override_is_used() {
        let config = parse(r#"data_dir = 'C:\zephyr-data'"#).unwrap();
        assert_eq!(config.data_dir().unwrap(), PathBuf::from(r"C:\zephyr-data"));
    }

    #[test]
    fn old_credentials_dir_key_still_works() {
        let config = parse(r#"credentials_dir = 'C:\creds'"#).unwrap();
        assert_eq!(config.data_dir().unwrap(), PathBuf::from(r"C:\creds"));
        assert!(
            toml::to_string_pretty(&config)
                .unwrap()
                .contains("data_dir")
        );
    }

    #[test]
    fn default_data_dir_is_local_app_data() {
        let mut config = Config::default();
        let default = config.data_dir().unwrap();
        assert!(default.ends_with(r"AppData\Local\zephyr"));
        config.set_data_dir(PathBuf::from(r"D:\music")).unwrap();
        assert_eq!(config.data_dir().unwrap(), PathBuf::from(r"D:\music"));
        config.set_data_dir(default).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn rejects_relative_data_dir() {
        let config = parse("data_dir = 'creds'").unwrap();
        assert!(matches!(
            config.data_dir(),
            Err(ConfigError::RelativeDataDir(_))
        ));
    }

    #[test]
    fn rejects_volume_above_max() {
        assert!(parse("volume = 101").is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(parse("volum = 30").is_err());
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(parse("volume = ").is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let path = Path::new("this/path/does/not/exist/config.toml");
        assert_eq!(load(path).unwrap(), Config::default());
    }
}
