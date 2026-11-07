# One synthesiser, many sounds

The question this document answers: *if I put this in a game, do I need a separate synthesiser
for every sound?*

No. The Geiger counter is not a synthesiser. It is three separable stages, and only the middle
one is specific to the instrument.

```
   event source              voice (a patch)                  mixer
   ------------              ----------------                 -----
   "when does it fire"  ->   "what does one event sound"  ->  overlap, gain, output
```

| stage | Geiger counter | rain bed | footsteps | weapon hit | UI tick |
|---|---|---|---|---|---|
| source | inhomogeneous Poisson + dead-time veto | Poisson | animation events | collision events | button events |
| voice | the `geiger` patch | the `raindrop` patch | the `footstep` patch | the `impact` patch | the `ui_tick` patch |
| mixer | shared | shared | shared | shared | shared |

One engine, one mixer, N patches, and a source per emitter. The crate demonstrates this today:
`cargo run --release -- --patches` renders seven sounds that span a percussive click, a pitched
drip, a footfall, a woodblock, a steady beep, a held alert tone and a noise-only rumble — all
from the same ~300 lines in `crates/geiger-core/src/patch.rs`.

## The voice model

A patch is a small physical model of a struck object:

```
event(t) = A(t) * [ sum_m  G_m * e^(-t/tau_m) * sin(2*pi*f_m(t)*t + phi_m)
                    + N(t) ]
```

| term | meaning | what it controls |
|---|---|---|
| `A(t)` | amplitude envelope, instant attack into exponential decay | whether it is a click or a swell |
| `sum_m` | the object's modes: partials that ring | the pitch and the timbre |
| `f_m(t)` | per-partial pitch sweep over the decay | drip, kick, laser, fall |
| `N(t)` | band-limited noise, filtered low / band / high | the strike, the grit, the wet contact |
| jitter | per-event variation of all of the above | body instead of machine |

Every patch is a handful of numbers. Adding a new sound is adding a `const`, not a subsystem —
see the bottom of `patch.rs` for seven worked examples with commentary on why each number is
what it is.

## The source

`Source` is anything that yields event times:

* `Poisson { seed, rate }` — memoryless. Radioactive decay, raindrops on a roof, anything whose
  intervals are exponential.
* `Regular { interval }` — a metronome, a machine, a synth arpeggio.
* `Times(Vec<f64>)` — whatever gameplay hands over: foot plants, bullet impacts, button
  presses, timeline automation.

The renderer never inspects which. A game with ten thousand footstep emitters schedules ten
thousand events from `Times` into the same engine that renders a Geiger counter.

## What changes for a real-time game

Offline, `render_stream` allocates one buffer per event and sums them. That is correct for a
file and wrong on an audio callback, where allocation is forbidden. The split is the same as
the one already used by the Geiger renderer:

1. **Voices are pooled.** Pre-allocate `max_voices` voice buffers at startup and reuse them.
   A voice is just `(patch_id, start_sample, params)` plus its render cursor.
2. **Events are scheduled, not rendered ahead.** The audio callback computes the block window
   `[t0, t0 + N)`, pulls every pending event that lands inside it, assigns each to a free voice,
   and renders the block voice by voice. Events arriving mid-block are placed at sample
   precision inside it.
3. **The source is pulled, not pushed.** A gameplay thread pushes event timestamps into a
   lock-free queue; the audio thread drains it each block. The Geiger counter does exactly this
   with its Poisson source, which is why its event stream is sample-accurate no matter how the
   host slices the buffer.
4. **Per-emitter gain is outside the voice.** The voice peak-normalises, so swapping a patch
   underneath an emitter never changes its level. Mixing is where loudness happens.

None of this is Geiger-specific. The Geiger crate is the reference implementation of the pattern
— a deterministic event stream, a pool of stateless voices, one mixer — and the seven patches
are the proof that the pattern generalises.

## Why not one synthesiser per sound

* Every extra synthesiser is another allocation policy, another set of voices to pool, another
  place for a click on the audio thread, another thing to keep in tune with the mixer's gain
  staging.
* Sounds that share an engine can share their variation: the same jitter that keeps a Geiger
  click from ossifying also keeps ten thousand footsteps from sounding like one.
* A patch is data. Data can be authored, diffed, hot-reloaded and unit-tested. A subsystem
  cannot.

## Recorded samples

For a shipping game the strongest option for a one-shot is still a recorded sample plus this
event scheduler: the scheduler already handles timing, polyphony and gain, so a patch can be
swapped for a sample without touching the other two stages. Synthesis is the fallback for sounds
where no clean recording exists, for sounds that must vary continuously, and for procedurally
generated content. The architecture above makes that a per-patch decision rather than a
per-sound one.
