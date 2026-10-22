# Background music and audio scheduling

The supplied **Quiet Resonance.mp3** is copied to
`assets/audio/quiet_resonance.mp3` and embedded in the executable. The original
Downloads file is unchanged. Windowed startup plays it at 30% volume; headless
renderer tests remain silent. `--no-audio` disables it, and `--audio-test` opens the
audio output for a four-second playback check. ImGui offers BGM pause and volume.
If no output device is available, the game continues and reports the audio error.

The Rust implementation uses Rodio 0.21 / CPAL with the Symphonia MP3 decoder.
On Linux it needs ALSA development libraries at build time (`libasound2-dev` and
`pkg-config` on Debian/Ubuntu). Cargo.lock pins the dependency versions.

The complete 214-second MP3 is decoded **before** world/renderer initialization.
This deliberately trades roughly 72 MiB of stereo float PCM memory for predictable
playback. CPAL's device callback runs independently of the rendering/event and
world-streaming threads. The custom infinite source reads an immutable `Arc` PCM
slice and wraps its index at the track boundary. It performs no decoding, disk
reads, engine mutex locking, or per-loop PCM allocation. The decoder is not kept
in the real-time path. Device buffering requests 2,048 frames (about 46 ms at
44.1 kHz); unsupported configurations fall back to a supported device format.

Atomic diagnostics report consumed PCM seconds, maximum source-feed gap, late
feed events and backend error callbacks. Timing is sampled once per 1,024 frames,
with a late threshold of max(100 ms, 2.5 times configured buffer duration). Pausing
and resuming resets the timing probe so an intentional pause is not counted as a
late event. The callback does not log; the backend error handler reports errors.
These metrics detect scheduling problems, but do not measure the physical speaker
signal or guarantee absence of every OS/device underrun.

To exercise playback while generation and GPU uploads are busy:

```sh
target/release/blockworld --water-view --stream-test --fly --frames 1500
```

The exit log includes audio diagnostics alongside renderer/streaming timings.
The automated test turns continuously, moves and switches between 24- and
2-chunk view radii under an 8 MiB chunk-cache budget. F11 may hide the overlay;
playback and diagnostics continue independently.

A native RTX 2060 SUPER test completed 1,500 frames, supplied 13.06 seconds of PCM,
and reported a maximum 43.31 ms feed gap, zero gaps above its 116.1 ms threshold,
and zero backend errors. Its largest video-frame interval after warmup was
91.016 ms. A separate four-second output check supplied 3.97 seconds of PCM with
42.56 ms maximum gap and zero late events/backend errors. These are local
scheduling observations, not a speaker-loopback analysis.
