import math

path = '/data/data/com.termux/files/home/projects/prisma/geiger-synth/out/geiger-60s.wav'
raw = open(path, 'rb').read()
i = raw.find(b'data') + 8
n = (len(raw) - i) // 3
sr = 48000

xs = []
for k in range(n):
    p = i + 3 * k
    v = int.from_bytes(raw[p:p + 3], 'little', signed=True)
    xs.append(v / 8388607.0)


def rms(a, b):
    s = xs[int(a * sr):int(b * sr)]
    if not s:
        return -120
    m = sum(x * x for x in s) / len(s)
    return 10 * math.log10(m + 1e-12)


for t in range(0, 60):
    r = rms(t, t + 1)
    peak = max(abs(x) for x in xs[int(t * sr):int((t + 1) * sr)])
    crest = 20 * math.log10(peak) - r if peak > 0 else 0
    print(f'{t:2d}  {r:7.1f} dBFS   peak {20 * math.log10(peak + 1e-9):6.1f}   crest {crest:5.1f}')
