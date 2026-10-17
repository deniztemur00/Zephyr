use std::collections::{HashMap, HashSet};

use crate::api::{Track, WebStat};

pub const RESULTS: usize = 50;
const PER_CHANNEL: usize = 2;
const MIN_MONTHLY: u64 = 1_000;
const FEATURED_PLAYLISTS: usize = 2;
const COMMUNITY_PLAYLISTS: usize = 3;

// Rising share of lifetime views, dampened by log reach: a 50M/month hit scores
// only ~1.6x a 50K/month indie release, and evergreen giants (small share) sink.
pub fn indie_score(views: u64, monthly: u64) -> f64 {
    let share = (monthly as f64 / views.max(1) as f64).min(1.0);
    share.sqrt() * (monthly as f64 + 1.0).log10()
}

pub fn rank(candidates: Vec<WebStat>, monthly: impl Fn(&WebStat) -> Option<u64>) -> Vec<WebStat> {
    let mut seen = HashSet::new();
    let mut scored: Vec<(f64, WebStat)> = candidates
        .into_iter()
        .filter(|stat| seen.insert(stat.video_id.clone()))
        .filter_map(|stat| {
            let gained = monthly(&stat).filter(|&gained| gained >= MIN_MONTHLY)?;
            Some((indie_score(stat.views, gained), stat))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut per_channel: HashMap<String, usize> = HashMap::new();
    scored
        .into_iter()
        .filter(|(_, stat)| {
            let count = per_channel.entry(stat.channel.to_lowercase()).or_default();
            *count += 1;
            *count <= PER_CHANNEL
        })
        .map(|(_, stat)| stat)
        .take(RESULTS)
        .collect()
}

pub fn pick_playlists(shelves: &[(String, Vec<String>)]) -> Vec<String> {
    let mut with_playlists = shelves.iter().filter(|(_, ids)| !ids.is_empty());
    let featured = with_playlists
        .next()
        .map_or(&[][..], |(_, ids)| ids.as_slice());
    let community = with_playlists
        .next()
        .map_or(&[][..], |(_, ids)| ids.as_slice());
    featured
        .iter()
        .take(FEATURED_PLAYLISTS)
        .chain(community.iter().take(COMMUNITY_PLAYLISTS))
        .cloned()
        .collect()
}

pub fn to_track(stat: WebStat) -> Track {
    Track {
        video_id: stat.video_id,
        title: stat.title,
        artists: stat.channel,
        album: None,
        duration: None,
        thumbnail: stat.thumbnail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(id: &str, channel: &str, views: u64, age_days: u32) -> WebStat {
        WebStat {
            video_id: id.into(),
            views,
            age_days: Some(age_days),
            title: id.into(),
            channel: channel.into(),
            thumbnail: None,
        }
    }

    fn lifetime_monthly(stat: &WebStat) -> Option<u64> {
        let days = u64::from(stat.age_days?.max(1));
        Some(stat.views * 30 / days)
    }

    #[test]
    fn a_rising_indie_song_beats_an_evergreen_hit() {
        let indie = indie_score(50_000, 50_000);
        let evergreen = indie_score(1_000_000_000, 5_000_000);
        let viral = indie_score(50_000_000, 50_000_000);
        assert!(indie > evergreen, "{indie} vs {evergreen}");
        assert!(
            viral / indie < 2.0,
            "big hits stay within 2x of indie: {viral} vs {indie}"
        );
    }

    #[test]
    fn caps_each_channel_and_drops_noise() {
        let candidates = vec![
            stat("a1", "Big Star", 40_000_000, 20),
            stat("a2", "Big Star", 30_000_000, 20),
            stat("a3", "big star", 20_000_000, 20),
            stat("b1", "Indie Kid", 60_000, 30),
            stat("tiny", "Bedroom", 200, 30),
            stat("b1", "Indie Kid", 60_000, 30),
        ];
        let ranked: Vec<String> = rank(candidates, lifetime_monthly)
            .into_iter()
            .map(|s| s.video_id)
            .collect();
        assert_eq!(
            ranked.iter().filter(|id| id.starts_with('a')).count(),
            PER_CHANNEL
        );
        assert_eq!(ranked.iter().filter(|id| *id == "b1").count(), 1);
        assert!(!ranked.contains(&"tiny".to_owned()));
    }

    #[test]
    fn songs_without_a_trend_are_left_out() {
        let ranked = rank(vec![stat("x", "c", 1_000_000, 100)], |_| None);
        assert!(ranked.is_empty());
    }

    #[test]
    fn picks_featured_then_community_playlists() {
        let shelves = vec![
            ("Songs".to_owned(), Vec::new()),
            (
                "Featured".to_owned(),
                vec!["F1".into(), "F2".into(), "F3".into()],
            ),
            (
                "Community".to_owned(),
                vec!["C1".into(), "C2".into(), "C3".into(), "C4".into()],
            ),
            ("Albums".to_owned(), vec!["A1".into()]),
        ];
        assert_eq!(pick_playlists(&shelves), ["F1", "F2", "C1", "C2", "C3"]);
        assert!(pick_playlists(&[]).is_empty());
    }
}
