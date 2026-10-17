use std::time::{Duration, Instant};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::TrackType;

use super::AudioError;
use super::decode::{Origin, seek_to};
use super::resolve::Resolver;

pub fn seek(resolver: &Resolver, video_id: &str, target: Duration) -> Result<(), AudioError> {
    let step = Instant::now();
    let stream = resolver.resolve(video_id)?;
    println!("resolve: {:?}", step.elapsed());

    let step = Instant::now();
    let (mut format, origin) = Origin::open(&stream, None)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(AudioError::NoTrack)?;
    let (track_id, time_base) = (track.id, track.time_base);
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or(AudioError::NoTrack)?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;
    println!("open + probe: {:?}", step.elapsed());

    let step = Instant::now();
    let landed = seek_to(&mut format, &origin, track_id, time_base, target);
    let seeked = step.elapsed();
    decoder.reset();
    while let Some(packet) = format.next_packet()? {
        if packet.track_id == track_id && decoder.decode(&packet).is_ok() {
            break;
        }
    }
    println!(
        "seek to {target:?}: landed {landed:?}, seek {seeked:?}, first audio after {:?}",
        step.elapsed()
    );
    Ok(())
}
