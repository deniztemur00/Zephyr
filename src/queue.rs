use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::Track;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    One,
    All,
}

impl Repeat {
    // The loop button's click order.
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::One,
            Self::One => Self::All,
            Self::All => Self::Off,
        }
    }

    // Pressing Next always leaves the current song, even when it is on repeat.
    pub fn for_skip(self) -> Self {
        match self {
            Self::One => Self::Off,
            other => other,
        }
    }
}

pub struct Queue {
    tracks: Vec<Track>,
    order: Vec<usize>,
    position: usize,
    shuffled: bool,
}

impl Queue {
    pub fn new(tracks: Vec<Track>, start: usize) -> Option<Self> {
        (start < tracks.len()).then(|| Self {
            order: (0..tracks.len()).collect(),
            tracks,
            position: start,
            shuffled: false,
        })
    }

    pub fn shuffled(tracks: Vec<Track>, rng: &mut Rng) -> Option<Self> {
        let mut queue = Self::new(tracks, 0)?;
        rng.shuffle(&mut queue.order);
        queue.shuffled = true;
        Some(queue)
    }

    pub fn current(&self) -> &Track {
        &self.tracks[self.order[self.position]]
    }

    pub fn upcoming(&self) -> Option<&Track> {
        let next = *self.order.get(self.position + 1)?;
        Some(&self.tracks[next])
    }

    pub fn is_shuffled(&self) -> bool {
        self.shuffled
    }

    pub fn advance(&mut self) -> Option<&Track> {
        if self.position + 1 >= self.order.len() {
            return None;
        }
        self.position += 1;
        Some(self.current())
    }

    pub fn following(&self, repeat: Repeat) -> Option<&Track> {
        match repeat {
            Repeat::One => Some(self.current()),
            Repeat::All => self
                .upcoming()
                .or_else(|| Some(&self.tracks[self.order[0]])),
            Repeat::Off => self.upcoming(),
        }
    }

    pub fn step(&mut self, repeat: Repeat) -> Option<&Track> {
        match repeat {
            Repeat::One => Some(self.current()),
            Repeat::All if self.position + 1 >= self.order.len() => {
                self.position = 0;
                Some(self.current())
            }
            _ => self.advance(),
        }
    }

    pub fn back(&mut self) -> Option<&Track> {
        self.position = self.position.checked_sub(1)?;
        Some(self.current())
    }

    pub fn extend(&mut self, tracks: Vec<Track>, rng: &mut Rng) {
        let start = self.tracks.len();
        self.tracks.extend(tracks);
        self.order.extend(start..self.tracks.len());
        if self.shuffled {
            rng.shuffle(&mut self.order[self.position + 1..]);
        }
    }

    pub fn set_shuffle(&mut self, on: bool, rng: &mut Rng) {
        let current = self.order[self.position];
        if on {
            self.order.swap(0, self.position);
            rng.shuffle(&mut self.order[1..]);
            self.position = 0;
        } else {
            self.order = (0..self.tracks.len()).collect();
            self.position = current;
        }
        self.shuffled = on;
    }
}

pub struct Rng(u64);

impl Rng {
    pub fn from_clock() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        Self(nanos as u64 ^ 0x9E37_79B9_7F4A_7C15)
    }

    #[cfg(test)]
    fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64: tiny, fast, passes BigCrush; ample for shuffling.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        let zone = u64::MAX - u64::MAX % bound;
        loop {
            let value = self.next_u64();
            if value < zone {
                return value % bound;
            }
        }
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for last in (1..items.len()).rev() {
            let pick = self.below(last as u64 + 1) as usize;
            items.swap(last, pick);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

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

    fn tracks(count: usize) -> Vec<Track> {
        (0..count).map(|i| track(&i.to_string())).collect()
    }

    fn play_order(queue: &mut Queue) -> Vec<String> {
        let mut seen = vec![queue.current().title.clone()];
        while let Some(next) = queue.advance() {
            seen.push(next.title.clone());
        }
        seen
    }

    #[test]
    fn rejects_out_of_range_start() {
        assert!(Queue::new(vec![track("a")], 1).is_none());
        assert!(Queue::new(Vec::new(), 0).is_none());
        assert!(Queue::shuffled(Vec::new(), &mut Rng::seeded(1)).is_none());
    }

    #[test]
    fn advances_and_goes_back_within_bounds() {
        let mut queue = Queue::new(tracks(3), 1).unwrap();
        assert_eq!(queue.upcoming().map(|t| t.title.as_str()), Some("2"));
        assert_eq!(queue.advance().map(|t| t.title.as_str()), Some("2"));
        assert!(queue.advance().is_none());
        assert_eq!(queue.back().map(|t| t.title.as_str()), Some("1"));
        assert_eq!(queue.back().map(|t| t.title.as_str()), Some("0"));
        assert!(queue.back().is_none());
    }

    #[test]
    fn shuffle_plays_every_track_exactly_once() {
        let mut queue = Queue::shuffled(tracks(1903), &mut Rng::seeded(7)).unwrap();
        let mut order = play_order(&mut queue);
        assert_ne!(order, play_order(&mut Queue::new(tracks(1903), 0).unwrap()));
        order.sort_by_key(|title| title.parse::<usize>().unwrap());
        let expected: Vec<String> = (0..1903).map(|i| i.to_string()).collect();
        assert_eq!(order, expected);
    }

    #[test]
    fn shuffle_is_uniform_over_all_orderings() {
        let mut rng = Rng::seeded(42);
        let mut counts: HashMap<Vec<usize>, u32> = HashMap::new();
        for _ in 0..24_000 {
            let mut items = [0, 1, 2, 3];
            rng.shuffle(&mut items);
            *counts.entry(items.to_vec()).or_default() += 1;
        }
        assert_eq!(counts.len(), 24);
        assert!(
            counts.values().all(|&n| (850..=1150).contains(&n)),
            "{counts:?}"
        );
    }

    #[test]
    fn extending_a_shuffled_queue_mixes_new_tracks_into_the_rest() {
        let mut rng = Rng::seeded(11);
        let mut queue = Queue::shuffled(tracks(100), &mut rng).unwrap();
        queue.advance();
        let played = [
            queue.back().unwrap().title.clone(),
            queue.advance().unwrap().title.clone(),
        ];
        let more: Vec<Track> = (100..300).map(|i| track(&i.to_string())).collect();
        queue.extend(more, &mut rng);
        let mut rest = play_order(&mut queue);
        assert_eq!(rest[0], played[1]);
        assert!(
            rest[1..60]
                .iter()
                .any(|t| t.parse::<usize>().unwrap() >= 100)
        );
        rest.push(played[0].clone());
        rest.sort_by_key(|t| t.parse::<usize>().unwrap());
        assert_eq!(rest, (0..300).map(|i| i.to_string()).collect::<Vec<_>>());
    }

    #[test]
    fn extending_an_ordered_queue_appends() {
        let mut queue = Queue::new(tracks(2), 1).unwrap();
        queue.extend(vec![track("2")], &mut Rng::seeded(1));
        assert_eq!(queue.advance().map(|t| t.title.as_str()), Some("2"));
    }

    fn title(track: Option<&Track>) -> Option<&str> {
        track.map(|t| t.title.as_str())
    }

    #[test]
    fn loop_button_cycles_off_one_all() {
        assert_eq!(Repeat::default(), Repeat::Off);
        assert_eq!(Repeat::Off.cycle(), Repeat::One);
        assert_eq!(Repeat::One.cycle(), Repeat::All);
        assert_eq!(Repeat::All.cycle(), Repeat::Off);
    }

    #[test]
    fn repeat_one_replays_the_current_track() {
        let mut queue = Queue::new(tracks(3), 1).unwrap();
        assert_eq!(title(queue.following(Repeat::One)), Some("1"));
        assert_eq!(title(queue.step(Repeat::One)), Some("1"));
        assert_eq!(title(queue.step(Repeat::One.for_skip())), Some("2"));
    }

    #[test]
    fn repeat_all_wraps_to_the_start_and_off_stops() {
        let mut queue = Queue::new(tracks(2), 1).unwrap();
        assert_eq!(title(queue.following(Repeat::Off)), None);
        assert_eq!(title(queue.following(Repeat::All)), Some("0"));
        assert_eq!(title(queue.step(Repeat::All)), Some("0"));
        assert_eq!(title(queue.step(Repeat::All)), Some("1"));
        assert_eq!(title(queue.step(Repeat::Off)), None);
        assert_eq!(queue.current().title, "1");
    }

    #[test]
    fn repeat_all_wraps_in_shuffled_order() {
        let mut queue = Queue::shuffled(tracks(5), &mut Rng::seeded(9)).unwrap();
        let first = queue.current().title.clone();
        while queue.step(Repeat::Off).is_some() {}
        assert_eq!(title(queue.step(Repeat::All)), Some(first.as_str()));
    }

    #[test]
    fn toggling_shuffle_keeps_the_current_track() {
        let mut rng = Rng::seeded(3);
        let mut queue = Queue::new(tracks(50), 10).unwrap();
        queue.set_shuffle(true, &mut rng);
        assert!(queue.is_shuffled());
        assert_eq!(queue.current().title, "10");
        let rest = play_order(&mut queue);
        assert_eq!(rest.len(), 50);
        let last: usize = rest.last().unwrap().parse().unwrap();
        queue.set_shuffle(false, &mut rng);
        assert!(!queue.is_shuffled());
        assert_eq!(queue.current().title, last.to_string());
        let expected_next = (last + 1 < 50).then(|| (last + 1).to_string());
        assert_eq!(queue.upcoming().map(|t| t.title.clone()), expected_next);
    }
}
