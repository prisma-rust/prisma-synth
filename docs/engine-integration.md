# Getting this into Unity and Unreal

The short version: **the hard part is not the engines, it is the language boundary**, and the
boundary is small enough that there are three viable routes. Pick one per project, not per
sound.

| route | when it is right | cost | runtime flexibility |
|---|---|---|---|
| A. Offline assets | fixed sounds, no reactive audio | lowest | none |
| B. Port the kernel | reactive audio, shipping on console/mobile | one afternoon | full |
| C. Native plugin (C ABI) | you want one implementation everywhere | per-platform builds | full |

The engine is ~300 lines of arithmetic with no dependencies. That number is the whole argument:
porting it is a mechanical translation, not a rewrite.

---

## Route A — render offline, import as assets

If the radiation level does not need to drive the sound continuously, render the sweep once and
ship a WAV.

```bash
cargo run --release            # out/geiger-60s.wav
```

* Unity: drop the WAV in `Assets/`, it becomes an `AudioClip`.
* Unreal: import as `USoundWave`.

The `analysis.csv` next to it gives `t -> CPM`, so gameplay can drive a **parameter** (pitch,
low-pass, a second layer) from the same curve without needing the synth at runtime. This is the
cheapest route and covers a surprising number of cases.

Not covered: audio that must react to a value the designer changes at runtime, or to many
independent emitters.

---

## Route B — port the kernel

This is the recommended route for anything reactive. The kernel is three pieces, and none of
them depend on Rust:

1. `Source` — event times. 20 lines.
2. `render_event` — one event to samples. 60 lines.
3. The voice pool and mix. 50 lines, and both engines already give you the loop.

### The Rust kernel, for reference

```rust
// geiger-core/src/patch.rs, render_event
for (i, out) in buf.iter_mut().enumerate() {
    let t = i as f64 / sr;
    let a = (1.0 - (-t / attack).exp()) * (-t / decay).exp();   // A(t)
    let mut osc = 0.0;
    for m in modes[..count].iter_mut() {                        // sum_m
        let progress = 1.0 - (-t / m.tau).exp();
        let f = m.hz * (1.0 + (m.sweep - 1.0) * progress);      // f_m(t)
        m.phase_acc += TAU * f / sr;
        osc += m.gain * (-t / m.tau).exp() * (m.phase + m.phase_acc).sin();
    }
    let nz = (-t / noise_tau).exp() * noise_svf(rng.bipolar()); // N(t)
    *out = (a * (osc + nz)) as f32;
}
```

### Unity (C#)

Unity hands you a buffer on the audio thread:

```csharp
using UnityEngine;

/// One voice: the same arithmetic as render_event, allocation-free.
public sealed class SynthVoice
{
    // Patch: partials as flat arrays so there is no per-event allocation.
    public float[] Hz, Tau, Gain, Sweep;
    public float Attack, Decay;
    public NoiseSpec Noise;            // can be null
    public float LengthSeconds;

    float[] _buf;                      // preallocated
    public float[] Buffer => _buf;

    struct NoiseSpec { public float Hz, Q, Tau, Gain; public int Mode; } // 0=low 1=band 2=high

    public SynthVoice(int sampleRate, float lengthSeconds, int maxModes = 8)
    {
        LengthSeconds = lengthSeconds;
        _buf = new float[Mathf.CeilToInt(lengthSeconds * sampleRate)];
    }

    public ulong Rng;                  // caller-owned SplitMix64 state

    float NextFloat()                  // [0,1)
    {
        Rng += 0x9E3779B97F4A7C15UL;
        ulong z = Rng;
        z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL;
        z = (z ^ (z >> 27)) * 0x94D049BB133111EBUL;
        return (z ^ (z >> 31)) * (1.0f / 18446744073709551616.0f);
    }

    /// Fill Buffer with one event. Call on the audio thread; allocates nothing.
    public void Render(float sampleRate, float pitchJitter)
    {
        int n = _buf.Length;
        System.Array.Clear(_buf, 0, n);

        const float TAU = 6.2831853f;
        int nm = Hz.Length;
        var f = new float[nm]; var ph = new float[nm];
        float body = 1f; // (jitter omitted here for brevity; see the Rust for the full form)
        for (int m = 0; m < nm; m++) { f[m] = Hz[m] * body; ph[m] = NextFloat() * TAU; }

        // Noise state-variable filter.
        bool hasNoise = Noise.Gain > 0f;
        float svfF = 2f * Mathf.Sin(Mathf.PI * Noise.Hz / sampleRate);
        float svfQ = 1f / Mathf.Max(Noise.Q, 0.05f);
        float low = 0f, band = 0f, hp = 0f;

        for (int i = 0; i < n; i++)
        {
            float t = i / sampleRate;
            float a = (1f - Mathf.Exp(-t / Attack)) * Mathf.Exp(-t / Decay);

            float osc = 0f;
            for (int m = 0; m < nm; m++)
            {
                float progress = 1f - Mathf.Exp(-t / Tau[m]);
                float fm = f[m] * (1f + (Sweep[m] - 1f) * progress);
                ph[m] += TAU * fm / sampleRate;
                osc += Gain[m] * Mathf.Exp(-t / Tau[m]) * Mathf.Sin(ph[m]);
            }

            float nz = 0f;
            if (hasNoise)
            {
                float white = NextFloat() * 2f - 1f;
                low += svfF * band;
                hp = white - low - svfQ * band;
                band += svfF * hp;
                float shaped = Noise.Mode == 0 ? low : Noise.Mode == 1 ? band : hp;
                nz = Mathf.Exp(-t / Noise.Tau) * Noise.Gain * shaped;
            }

            _buf[i] = a * (osc + nz);
        }

        // DC blocker + peak normalise, same as the Rust.
        float r = Mathf.Exp(-TAU * 120f / sampleRate), y1 = 0f, x1 = 0f;
        for (int i = 0; i < n; i++)
        {
            float v = _buf[i];
            float y = v - x1 + r * y1;
            x1 = v; y1 = y; _buf[i] = y;
        }
        float peak = 0f;
        for (int i = 0; i < n; i++) peak = Mathf.Max(peak, Mathf.Abs(_buf[i]));
        if (peak > 0f) { float g = 1f / peak; for (int i = 0; i < n; i++) _buf[i] *= g; }
    }
}
```

The emitter then owns a pool and a scheduler:

```csharp
public sealed class SynthEmitter : MonoBehaviour
{
    const int SampleRate = 48000;
    const int MaxVoices = 24;

    [Range(0f, 5000f)] public float Cpm = 20f;   // driven by gameplay

    SynthVoice[] _voices;                        // preallocated
    float[] _started;                            // per-voice start time in seconds
    double _nextEvent;                           // Poisson clock
    System.Random _rng = new System.Random(1);

    void Awake()
    {
        _voices = new SynthVoice[MaxVoices];
        _started = new float[MaxVoices];
        for (int i = 0; i < MaxVoices; i++)
        {
            _voices[i] = new SynthVoice(SampleRate, 0.008f);
            _voices[i].Hz = new[] { 2600f, 1250f };
            _voices[i].Tau = new[] { 0.0012f, 0.0016f };
            _voices[i].Gain = new[] { 0.60f, 0.27f };
            _voices[i].Sweep = new[] { 1f, 1f };
            _voices[i].Attack = 0.00006f;
            _voices[i].Decay = 0.0012f;
            _started[i] = -1f;
        }
    }

    // Unity's per-filter callback: runs on the audio thread, must not allocate or block.
    void OnAudioFilterRead(float[] data, int channels)
    {
        int frames = data.Length / channels;
        float dt = 1f / SampleRate;
        double now = AudioSettings.dspTime;

        // 1. Schedule: draw events for this block.
        while (_nextEvent < now + frames * dt)
        {
            float t = (float)(_nextEvent - now);
            int slot = FindFreeVoice(t);
            if (slot >= 0) { _voices[slot].Render(SampleRate, 0.18f); _started[slot] = t; }
            double rate = Cpm / 60.0;
            _nextEvent += -System.Math.Log(1.0 - _rng.NextDouble()) / rate;
        }

        // 2. Mix: add every live voice into the block at its frame offset.
        for (int v = 0; v < MaxVoices; v++)
        {
            if (_started[v] < 0f) continue;
            int off = Mathf.RoundToInt(_started[v] * SampleRate);
            var buf = _voices[v].Buffer;
            for (int i = 0; i < buf.Length; i++)
            {
                int frame = off + i;
                if (frame < 0) continue;
                if (frame >= frames) break;
                float s = buf[i] * 0.5f;
                for (int c = 0; c < channels; c++) data[frame * channels + c] += s;
            }
            if (off + buf.Length <= frames) _started[v] = -1f;   // voice finished
        }
    }

    int FindFreeVoice(float at) { /* first free, or steal the oldest */ return 0; }
}
```

Notes that matter:

* `OnAudioFilterRead` runs on a **background thread**, not the main thread. Do not touch
  `transform`, do not call `Debug.Log`, do not allocate.
* `AudioSettings.dspTime` is the audio clock. Use it, not `Time.time`; `Time.time` is the render
  clock and will drift against the buffer.
* Unity's `OnAudioFilterRead` buffers are typically 256-1024 frames. At 5000 cpm (83 events/s)
  that is at most one event per block, so the scheduling loop almost never runs more than once.
* If you would rather not write C# DSP at all, this whole route becomes Route C.

### Unreal (C++)

Unreal's recommended path is `ISoundGenerator` (or `USynthComponent` on top of it):

```cpp
// SynthVoice.h — the same arithmetic, in C++.
#pragma once
#include "CoreMinimal.h"

struct FSynthPartial { float Hz, Tau, Gain, Sweep; };

struct FSynthNoise { float Hz, Q, Tau, Gain; int32 Mode; };

class FSynthVoice
{
public:
    void Init(int32 SampleRate, float LengthSeconds);
    /** Render one event into the internal buffer. Audio thread; no allocation. */
    void RenderEvent(float SampleRate, uint64 Rng[1]);

    const TArray<float>& Buffer() const { return Buf; }

    TArray<FSynthPartial> Partials;
    FSynthNoise Noise;                 // Gain == 0 means "none"
    float Attack = 0.00006f, Decay = 0.0012f;

private:
    TArray<float> Buf;                 // preallocated, never resized after Init
    TArray<float> Phase, PhaseAcc, Freq;
};
```

```cpp
// SynthVoice.cpp
#include "SynthVoice.h"

static FORCEINLINE float NextFloat(uint64 Rng[1])
{
    Rng[0] += 0x9E3779B97F4A7C15ull;
    uint64 z = Rng[0];
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return float((z ^ (z >> 31)) >> 40) * (1.0f / 16777216.0f);
}

void FSynthVoice::Init(int32 SampleRate, float LengthSeconds)
{
    Buf.SetNumZeroed(FMath::CeilToInt(LengthSeconds * SampleRate));
    Phase.SetNum(Partials.Num());
    PhaseAcc.SetNumZeroed(Partials.Num());
    Freq.SetNum(Partials.Num());
}

void FSynthVoice::RenderEvent(float SampleRate, uint64 Rng[1])
{
    const int32 n = Buf.Num();
    const int32 nm = Partials.Num();
    for (int32 m = 0; m < nm; ++m)
    {
        Freq[m] = Partials[m].Hz;
        Phase[m] = NextFloat(Rng) * 2.0f * PI;
        PhaseAcc[m] = 0.0f;
    }

    const bool bNoise = Noise.Gain > 0.0f;
    const float SvfF = 2.0f * FMath::Sin(PI * Noise.Hz / SampleRate);
    const float SvfQ = 1.0f / FMath::Max(Noise.Q, 0.05f);
    float Low = 0.0f, Band = 0.0f, High = 0.0f;

    for (int32 i = 0; i < n; ++i)
    {
        const float t = i / SampleRate;
        const float A = (1.0f - FMath::Exp(-t / Attack)) * FMath::Exp(-t / Decay);

        float Osc = 0.0f;
        for (int32 m = 0; m < nm; ++m)
        {
            const float Progress = 1.0f - FMath::Exp(-t / Partials[m].Tau);
            const float F = Freq[m] * (1.0f + (Partials[m].Sweep - 1.0f) * Progress);
            PhaseAcc[m] += 2.0f * PI * F / SampleRate;
            Osc += Partials[m].Gain * FMath::Exp(-t / Partials[m].Tau)
                 * FMath::Sin(Phase[m] + PhaseAcc[m]);
        }

        float Nz = 0.0f;
        if (bNoise)
        {
            const float White = NextFloat(Rng) * 2.0f - 1.0f;
            Low += SvfF * Band;
            High = White - Low - SvfQ * Band;
            Band += SvfF * High;
            const float Shaped = Noise.Mode == 0 ? Low : Noise.Mode == 1 ? Band : High;
            Nz = FMath::Exp(-t / Noise.Tau) * Noise.Gain * Shaped;
        }

        Buf[i] = A * (Osc + Nz);
    }

    // DC block + peak normalise.
    const float R = FMath::Exp(-2.0f * PI * 120.0f / SampleRate);
    float Y1 = 0.0f, X1 = 0.0f;
    for (int32 i = 0; i < n; ++i)
    {
        const float V = Buf[i];
        const float Y = V - X1 + R * Y1;
        X1 = V; Y1 = Y; Buf[i] = Y;
    }
    float Peak = 0.0f;
    for (float S : Buf) Peak = FMath::Max(Peak, FMath::Abs(S));
    if (Peak > 0.0f) { const float G = 1.0f / Peak; for (float& S : Buf) S *= G; }
}
```

And the generator:

```cpp
// GeigerSynthComponent.h
#pragma once
#include "Components/SynthComponent.h"
#include "GeigerSynthComponent.generated.h"

UCLASS(ClassGroup = Synth, meta = (BlueprintSpawnableComponent))
class UGeigerSynthComponent : public USynthComponent
{
    GENERATED_BODY()
public:
    UPROPERTY(BlueprintReadWrite, EditAnywhere, Category = "Geiger")
    float Cpm = 20.0f;

protected:
    virtual bool Init(int32& SampleRate) override;
    virtual int32 OnGenerateAudio(float* OutAudio, int32 NumSamples) override;

private:
    static constexpr int32 MaxVoices = 24;
    TArray<FSynthVoice> Voices;
    TArray<float> StartedAt;      // seconds, relative to the block; negative = free
    double NextEvent = 0.0;
    uint64 Rng = 0x27062026ull;
    int32 SampleRateHz = 48000;
    double BlockTime = 0.0;       // monotonic audio clock, advanced per block
};
```

```cpp
// GeigerSynthComponent.cpp
#include "GeigerSynthComponent.h"

bool UGeigerSynthComponent::Init(int32& SampleRate)
{
    SampleRateHz = SampleRate;
    NumChannels = 1;
    Voices.SetNum(MaxVoices);
    StartedAt.SetNum(MaxVoices);
    for (int32 i = 0; i < MaxVoices; ++i)
    {
        Voices[i].Partials = { { 2600.0f, 0.0012f, 0.60f, 1.0f },
                               { 1250.0f, 0.0016f, 0.27f, 1.0f } };
        Voices[i].Noise = { 3000.0f, 1.1f, 0.0012f, 0.55f, 1 };
        Voices[i].Attack = 0.00006f;
        Voices[i].Decay = 0.0012f;
        Voices[i].Init(SampleRate, 0.008f);
        StartedAt[i] = -1.0f;
    }
    return true;
}

int32 UGeigerSynthComponent::OnGenerateAudio(float* OutAudio, int32 NumSamples)
{
    FMemory::Memzero(OutAudio, NumSamples * sizeof(float));
    const double Dt = 1.0 / SampleRateHz;
    const double BlockEnd = BlockTime + NumSamples * Dt;

    // Schedule the events that land inside this block.
    while (NextEvent < BlockEnd)
    {
        const float At = float(NextEvent - BlockTime);
        int32 Slot = -1;
        for (int32 i = 0; i < MaxVoices; ++i) if (StartedAt[i] < 0.0f) { Slot = i; break; }
        if (Slot >= 0)
        {
            Voices[Slot].RenderEvent(SampleRateHz, &Rng);
            StartedAt[Slot] = At;
        }
        const double Rate = FMath::Max(Cpm, 1.0f) / 60.0;
        NextEvent += -FMath::Loge(1.0 - FMath::FRand()) / Rate;   // or the same SplitMix64
    }

    // Mix live voices.
    for (int32 v = 0; v < MaxVoices; ++v)
    {
        if (StartedAt[v] < 0.0f) continue;
        const int32 Off = FMath::RoundToInt(StartedAt[v] * SampleRateHz);
        const TArray<float>& B = Voices[v].Buffer();
        for (int32 i = 0; i < B.Num(); ++i)
        {
            const int32 F = Off + i;
            if (F < 0) continue;
            if (F >= NumSamples) break;
            OutAudio[F] += B[i] * 0.5f;
        }
        if (Off + B.Num() <= NumSamples) StartedAt[v] = -1.0f;
    }

    BlockTime = BlockEnd;
    return NumSamples;
}
```

Unreal-specific notes:

* `OnGenerateAudio` runs on the **audio render thread**. Same rules as Unity: no allocation, no
  UObject access, no logging.
* `NumChannels = 1` and a mono buffer keeps the arithmetic identical to the Rust; let the
  engine's spatialiser pan it.
* `USynthComponent` handles attenuation, concurrency and submixes for free, which is a real
  advantage over a raw plugin.
* **MetaSounds** is the alternative if you prefer a node graph. It is excellent for subtractive
  and sample-based work, but Poisson event generation is awkward there — you would end up with a
  custom node that runs exactly the code above, so treat MetaSounds as the wrapper, not the
  engine.
* Determinism: keep the `Rng` word per emitter if you want reproducible takes; use the engine's
  random stream if you do not.

---

## Route C — embed the Rust via the C ABI

`crates/synth-ffi` is a ready `cdylib`/`staticlib` with a flat C surface, no allocation on the
hot path and no Rust types in any signature:

```c
uint32_t     synth_voice_len(int32_t patch, double sr);
uint32_t     synth_render_event(int32_t patch, double sr, uint64_t *rng, float *out, uint32_t len);
uint32_t     synth_patch_count(void);
const char  *synth_patch_name(uint32_t index);
int32_t      synth_patch_index(const char *name);
```

Verified from straight C on this machine:

```
$ gcc -O2 ctest.c -Ltarget/release -lsynth_ffi -lm -lpthread -ldl
$ LD_LIBRARY_PATH=target/release ./ctest
patches: 7
  [0] geiger    short broadband click with a 2.6 kHz plastic box ring
  ...
geiger voice: 385 samples, peak 1.0000, energy 12.6438, rng advanced=1
```

* Unity: `[DllImport("synth_ffi")]`, drop `libsynth_ffi.so` / `.dylib` / `.dll` in
  `Assets/Plugins/<platform>/`. ~30 lines of binding, and the DSP stays in Rust.
* Unreal: a `ThirdParty` module linking `libsynth_ffi.a` (static) or the shared library, plus a
  `Build.cs` entry. Same calls.

The cost is the one thing that is genuinely hard here: **per-platform native builds**. Android
(NDK), iOS, and each console SDK need their own toolchain and their own binary, and console
platforms usually require the vendor SDK to link at all. If that is in scope, Route C is worth
it. If it is not, Route B has none of it.

---

## What is actually hard

Nothing about the synthesis. The engine is small, dependency-free, allocation-free on the event
path, and already verified against a C caller. The real costs, in order:

1. **Native plugin build chains** (Route C only) — one toolchain per platform, plus console SDKs.
2. **Voice pool sizing and stealing policy** — a design decision per emitter, not a porting one.
   `synth_patch_hint()` returns a starting point.
3. **Clock discipline** — use `AudioSettings.dspTime` / the engine's audio clock, never the frame
   clock, or the event train drifts.
4. **Thread discipline** — no allocation, no engine API calls, no locks on the audio thread.

## Recommendation

Start with Route B. The kernel is small enough to port in an afternoon, it removes the native
build chain entirely, and it keeps the *design* — which is the valuable part — intact. Keep the
Rust crate as the offline renderer and the reference implementation, so any change to a patch can
be heard and measured before it is ported. Reach for Route C only if you end up maintaining many
patches and want a single source of truth across platforms.
