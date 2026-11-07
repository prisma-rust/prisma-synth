# Geiger synth

An event-driven procedural synthesiser, and the Geiger-Muller counter it was built to model.

The project started as a question — *can a Geiger counter be synthesised convincingly instead of
sampled?* — and ended as a small general engine for percussive and one-shot sounds. The counter
is its reference patch, not its purpose: `cargo run --release -- --patches` renders seven
different sounds from the same ~300 lines.

## Quick start

```bash
cargo run --release                 # 60 s counter ramp -> out/geiger-60s.wav and .mp3
cargo run --release -- --patches    # one demo per patch in the library
cargo run --release -- --styles     # the Geiger click, four ways, for comparison
cargo run --release -- --patch wood # one patch, with its parameters printed
cargo run --release --bin diag -- --click geiger   # inspect one event's envelope
```

Requires `ffmpeg` on `PATH` for the MP3 side-car; the WAV, the spectrogram and the CSV are
produced without it.

### What the main render produces

| file | what it is |
|---|---|
| `out/geiger-60s.wav` | 48 kHz, 24-bit mono master |
| `out/geiger-60s.mp3` | 320 kbps listen copy |
| `out/spectrogram.png` | portrait spectrogram, 10 s ticks along the time axis |
| `out/analysis.csv` | per 0.25 s: incident CPM, displayed CPM, clicks |
| `out/stats.txt` | the measured numbers backing the design notes |

## The clip

Sixty seconds, natural background to a source three times hotter than the tube can count.

| t | incident rate | what is heard |
|---|---|---|
| 0–10 s | 20 cpm | background: an isolated click every few seconds |
| 10–56 s | climbing | an accelerando over 4.9 decades of rate |
| ~50 s+ | ≥ ceiling | the tube starts discarding more than it passes |
| 56–58.4 s | 20 kcps | saturated: the display pins at 400 kcpm and stops moving |
| 58.4–60 s | falling | the recovery, heard one click at a time |

The interesting part is the end of the climb. A Geiger tube has a dead time — after each
avalanche the quenching gas needs to recombine, and events that land in that window are not
counted. The model is the non-paralyzable one, `r_obs = r_true / (1 + r_true * tau)` with
`tau = 150 us`, so the observed rate saturates at `1/tau = 400 kcpm` no matter how hot the source
gets. At the top of the ramp the tube is rejecting three events in four. The click train turns
from a rattle into a texture, and the display stops telling the truth. That transition is the
whole point of the clip.

## The two layers

```
   event source              voice (a patch)                  mixer
   ------------              ----------------                 -----
   "when does it fire"  ->   "what does one event sound"  ->  overlap, gain, output
```

* `crates/geiger-core/src/poisson.rs` — the detector: an inhomogeneous Poisson process for the
  incident events, plus the dead-time veto.
* `crates/geiger-core/src/ramp.rs` — how the incident rate moves over the clip.
* `crates/geiger-core/src/patch.rs` — the general voice engine and the patch library.
* `crates/geiger-core/src/click.rs` — the Geiger click, as named styles over that engine.
* `crates/geiger-core/src/track.rs` — offline rendering, gain staging, the analysis side-cars.
* `crates/geiger-core/src/loud.rs` — EBU R128 / BS.1770 loudness, so the mix can be verified.
* `crates/synth-ffi` — a C ABI, for hosts that would rather embed than port.
* `crates/geiger-cli` — the `geiger` binary.

Everything is deterministic from a seed: same seed, same samples, bit for bit.

## The voice model

A patch is a small physical model of a struck object:

```
event(t) = A(t) * [ sum_m  G_m * e^(-t/tau_m) * sin(2*pi*f_m(t)*t + phi_m)
                    + N(t) ]
```

`A(t)` is the envelope, the sum is the object's ringing modes, `f_m(t)` is an optional pitch sweep
per mode, and `N(t)` is band-limited noise for the strike. Every parameter jitters per event,
because a body never hits the same way twice.

Two failure modes bracket the target, and this project hit both before finding the middle:

* **modes only** — a decaying sine, heard as a short *beep*. The first version sounded like a
  tone with clicks on top, because a fixed resonant frequency is a pitch the ear will find.
* **noise only** — a burst of white noise, heard as a *hiss*. The second version lost the
  instrument's identity entirely.

A real counter is neither. It is a sharp broadband edge followed by a very short, heavily damped
resonance from the box. The edge carries the attack; the ring carries the instrument. The balance
between them is a listening decision, so it lives in named styles (`clack`, `hard`, `ring`, `dry`)
that can be rendered side by side rather than in magic constants.

`diag --click <patch>` measures that balance for you: the Geiger click puts 99.5 % of its energy
in the first 2 ms and is 80 dB down by 8 ms, while `raindrop` and `ui_tick` sit at 18 % and
`rumble` at 0 %.

## Tests

```bash
cargo test --release --workspace
```

23 tests, covering the detector model (exponential intervals, saturation at `1/tau`, the inverse
correction, dead-time efficiency against the simulated veto), the ramp, the gain staging, the
patch engine (every patch renders one event at exactly full scale, deterministically, including a
patch with no partials at all, and all three event sources), and the C ABI from Rust.

The C ABI has also been exercised from straight C:

```sh
gcc -O2 ctest.c -Ltarget/release -lsynth_ffi -lm -lpthread -ldl
```

## Using this in a game

One engine, N patches, one source per emitter — not one synthesiser per sound. See
[`docs/synth-architecture.md`](docs/synth-architecture.md) for the pattern and the seven worked
patches, and [`docs/engine-integration.md`](docs/engine-integration.md) for Unity and Unreal,
including a decision tree, the C# and C++ ports of the kernel, and the C ABI route.

## What this is not

Not a general-purpose synthesiser. There are no per-voice filters, LFOs, modulation matrices or
note tracking, and no continuous control. It is the smallest thing that covers percussive and
one-shot sounds — the ones a game needs by the thousand — while keeping the property that makes
that possible: one event in, a bounded amount of work, and no hidden state between events.

## License

MIT OR Apache-2.0.
