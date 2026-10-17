use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

pub const FILE: &str = "stats.json";
const SECONDS_PER_DAY: u64 = 86_400;
const MONTH_DAYS: u32 = 30;
const MAX_SAMPLES: usize = 4;
const MAX_VIDEOS: usize = 20_000;

#[derive(Debug, Error)]
pub enum StatsError {
    #[error("could not access {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: io::Error,
    },
    #[error("stats file is corrupt: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Monthly {
    Measured(i64),
    Estimated(u64),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Popularity {
    pub views: u64,
    pub monthly: Monthly,
}

#[derive(Debug, Default)]
pub struct History {
    samples: HashMap<String, Vec<(u32, u64)>>,
}

impl History {
    pub fn load(path: &Path) -> Result<Self, StatsError> {
        match fs::read(path) {
            Ok(bytes) => Ok(Self {
                samples: serde_json::from_slice(&bytes)?,
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(StatsError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    pub fn save(&mut self, path: &Path) -> Result<(), StatsError> {
        self.cap();
        let io_err = |source| StatsError::Io {
            path: path.to_owned(),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec(&self.samples)?).map_err(io_err)?;
        fs::rename(&temporary, path).map_err(io_err)
    }

    pub fn record(&mut self, video_id: &str, views: u64, today: u32) {
        let samples = self.samples.entry(video_id.to_owned()).or_default();
        match samples.last_mut() {
            Some(last) if last.0 == today => last.1 = views,
            _ => samples.push((today, views)),
        }
        // Keep one baseline that is at least a month old, plus the most recent samples.
        while samples.len() >= 2 && samples[1].0 + MONTH_DAYS <= today {
            samples.remove(0);
        }
        while samples.len() > MAX_SAMPLES {
            samples.remove(samples.len() - 2);
        }
    }

    pub fn popularity(
        &self,
        video_id: &str,
        views: u64,
        age_days: Option<u32>,
        today: u32,
    ) -> Popularity {
        let measured = self.samples.get(video_id).and_then(|samples| {
            let (day, base) = *samples.first()?;
            let span = today.checked_sub(day).filter(|&span| span > 0)?;
            let gained = views as i64 - base as i64;
            Some(gained * i64::from(MONTH_DAYS) / i64::from(span))
        });
        let estimated = age_days
            .filter(|&days| days > 0)
            .map(|days| views * u64::from(MONTH_DAYS) / u64::from(days));
        let monthly = match (measured, estimated) {
            (Some(gain), _) => Monthly::Measured(gain),
            (None, Some(average)) => Monthly::Estimated(average),
            (None, None) => Monthly::Unknown,
        };
        Popularity { views, monthly }
    }

    fn cap(&mut self) {
        if self.samples.len() <= MAX_VIDEOS {
            return;
        }
        let mut by_freshness: Vec<(u32, String)> = self
            .samples
            .iter()
            .map(|(id, samples)| (samples.last().map_or(0, |s| s.0), id.clone()))
            .collect();
        by_freshness.sort_unstable();
        let excess = self.samples.len() - MAX_VIDEOS;
        for (_, id) in by_freshness.into_iter().take(excess) {
            self.samples.remove(&id);
        }
    }
}

pub fn today() -> u32 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    (seconds / SECONDS_PER_DAY) as u32
}

pub fn compact(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => scaled(count, 1_000, "K"),
        1_000_000..1_000_000_000 => scaled(count, 1_000_000, "M"),
        _ => scaled(count, 1_000_000_000, "B"),
    }
}

fn scaled(count: u64, unit: u64, suffix: &str) -> String {
    let value = count as f64 / unit as f64;
    if value < 10.0 {
        format!("{:.1}{suffix}", (value * 10.0).floor() / 10.0)
    } else {
        format!("{}{suffix}", value.floor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_numbers_never_round_up() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_700_000), "1.7M");
        assert_eq!(compact(1_999_999), "1.9M");
        assert_eq!(compact(244_000), "244K");
        assert_eq!(compact(12_345), "12K");
        assert_eq!(compact(1_100_000_000), "1.1B");
    }

    #[test]
    fn first_visit_estimates_from_lifetime_average() {
        let history = History::default();
        let popularity = history.popularity("v", 1_700_000, Some(2190), 20_000);
        assert_eq!(popularity.monthly, Monthly::Estimated(23_287));
        assert_eq!(
            history.popularity("v", 5, None, 20_000).monthly,
            Monthly::Unknown
        );
    }

    #[test]
    fn later_visits_measure_real_growth_scaled_to_a_month() {
        let mut history = History::default();
        history.record("v", 1_000_000, 100);
        history.record("v", 1_010_000, 110);
        let popularity = history.popularity("v", 1_010_000, Some(2190), 110);
        assert_eq!(popularity.monthly, Monthly::Measured(30_000));
    }

    #[test]
    fn same_day_samples_do_not_count_as_growth() {
        let mut history = History::default();
        history.record("v", 1_000, 100);
        history.record("v", 1_200, 100);
        assert_eq!(
            history.popularity("v", 1_200, Some(10), 100).monthly,
            Monthly::Estimated(3_600)
        );
    }

    #[test]
    fn keeps_a_month_old_baseline_and_few_samples() {
        let mut history = History::default();
        for day in 0..120 {
            history.record("v", u64::from(day) * 100, day);
        }
        let samples = &history.samples["v"];
        assert!(samples.len() <= MAX_SAMPLES);
        let baseline = samples[0].0;
        assert!(
            baseline + MONTH_DAYS <= 119 && baseline + 2 * MONTH_DAYS > 119,
            "{samples:?}"
        );
        assert_eq!(samples.last(), Some(&(119, 11_900)));
    }

    #[test]
    fn caps_the_number_of_tracked_videos() {
        let mut history = History::default();
        for i in 0..(MAX_VIDEOS + 5) {
            history.record(&format!("v{i}"), 1, i as u32);
        }
        history.cap();
        assert_eq!(history.samples.len(), MAX_VIDEOS);
        assert!(!history.samples.contains_key("v0"));
        assert!(
            history
                .samples
                .contains_key(&format!("v{}", MAX_VIDEOS + 4))
        );
    }
}
