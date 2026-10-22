//! Decode once; the device callback only reads an immutable PCM ring, independently
//! of the renderer/streamer. No filesystem access, decoding, or engine locks there.
use anyhow::{Context, Result};
use rodio::{cpal::BufferSize, Decoder, OutputStream, OutputStreamBuilder, Sink, Source};
use std::{
    io::Cursor,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const TRACK: &[u8] = include_bytes!("../assets/audio/quiet_resonance.mp3");
#[derive(Default)]
pub struct Health {
    reset_timing: AtomicBool,
    paused: AtomicBool,
    samples: AtomicU64,
    max_gap_us: AtomicU64,
    late_gaps: AtomicU64,
    errors: AtomicU64,
}
struct LoopPcm {
    pcm: Arc<[f32]>,
    pos: usize,
    channels: u16,
    rate: u32,
    health: Arc<Health>,
    last: Option<Instant>,
    probe: usize,
    deadline_us: u64,
}
impl Iterator for LoopPcm {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let value = self.pcm[self.pos];
        self.pos += 1;
        if self.pos == self.pcm.len() {
            self.pos = 0;
        }
        self.probe += 1;
        if self.probe == 1024 * self.channels as usize {
            let now = Instant::now();
            if self.health.reset_timing.swap(false, Ordering::Relaxed) {
                self.last = None;
            }
            if let Some(last) = self.last {
                let gap = now.duration_since(last).as_micros() as u64;
                self.health.max_gap_us.fetch_max(gap, Ordering::Relaxed);
                if gap > self.deadline_us {
                    self.health.late_gaps.fetch_add(1, Ordering::Relaxed);
                }
            }
            self.last = Some(now);
            self.health
                .samples
                .fetch_add(self.probe as u64, Ordering::Relaxed);
            self.probe = 0;
        }
        Some(value)
    }
}
impl Source for LoopPcm {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
pub struct Audio {
    sink: Sink,
    _stream: OutputStream,
    health: Arc<Health>,
    rate: u32,
    channels: u16,
    pub playing: bool,
    pub volume: f32,
    pub duration: f32,
    pub deadline_ms: f32,
}
impl Audio {
    pub fn start() -> Result<Self> {
        let decoder =
            Decoder::try_from(Cursor::new(TRACK)).context("Decode Quiet Resonance.mp3")?;
        let rate = decoder.sample_rate();
        let channels = decoder.channels();
        let pcm: Vec<f32> = decoder.collect();
        anyhow::ensure!(
            !pcm.is_empty() && pcm.iter().all(|v| v.is_finite()),
            "Invalid BGM samples"
        );
        let duration = pcm.len() as f32 / (rate as f32 * channels as f32);
        let health = Arc::new(Health::default());
        let errors = health.clone();
        let mut stream = OutputStreamBuilder::from_default_device()?
            .with_buffer_size(BufferSize::Fixed(2048))
            .with_error_callback(move |error| {
                errors.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("Audio backend: {error}");
            })
            .open_stream_or_fallback()
            .context("Open audio output")?;
        stream.log_on_drop(false);
        let frames = match stream.config().buffer_size() {
            BufferSize::Fixed(n) => *n,
            BufferSize::Default => 2048,
        };
        let deadline_ms =
            (frames as f32 / stream.config().sample_rate() as f32 * 2500.0).max(100.0);
        let sink = Sink::connect_new(stream.mixer());
        sink.set_volume(0.3);
        sink.append(LoopPcm {
            pcm: pcm.into(),
            pos: 0,
            channels,
            rate,
            health: health.clone(),
            last: None,
            probe: 0,
            deadline_us: (deadline_ms * 1000.0) as u64,
        });
        println!(
            "BGM: Quiet Resonance | {duration:.1}s decoded PCM | {:?}",
            stream.config()
        );
        Ok(Self {
            sink,
            _stream: stream,
            health,
            rate,
            channels,
            playing: true,
            volume: 0.3,
            duration,
            deadline_ms,
        })
    }
    pub fn update(&self) {
        if self.health.paused.swap(!self.playing, Ordering::Relaxed) == self.playing {
            self.health.reset_timing.store(true, Ordering::Relaxed);
        }
        self.sink.set_volume(self.volume);
        if self.playing {
            self.sink.play();
        } else {
            self.sink.pause();
        }
    }
    pub fn stats(&self) -> (f32, f32, u64, u64) {
        (
            self.health.samples.load(Ordering::Relaxed) as f32
                / (self.rate as f32 * self.channels as f32),
            self.health.max_gap_us.load(Ordering::Relaxed) as f32 / 1000.0,
            self.health.late_gaps.load(Ordering::Relaxed),
            self.health.errors.load(Ordering::Relaxed),
        )
    }
    pub fn report(&self) {
        let (played, gap, late, errors) = self.stats();
        println!("Audio: consumed {played:.2}s PCM | max feed gap {gap:.2} ms | gaps >{:.1}ms {late} | backend errors {errors}",self.deadline_ms);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn looping_pcm_is_continuous_at_track_boundary() {
        let mut source = LoopPcm {
            pcm: Arc::from([0.1, -0.2, 0.3, -0.4]),
            pos: 0,
            channels: 2,
            rate: 44100,
            health: Arc::new(Health::default()),
            last: None,
            probe: 0,
            deadline_us: 100000,
        };
        assert_eq!(
            source.by_ref().take(12).collect::<Vec<_>>(),
            [0.1, -0.2, 0.3, -0.4].repeat(3)
        );
    }
}
