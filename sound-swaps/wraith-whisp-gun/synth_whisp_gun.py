#!/usr/bin/env python3
"""Synthesize whispy/futuristic energy-pistol shots for Wraith's gun swap.

Pure numpy DSP -> 16-bit PCM WAV; encode to MP3 happens in the shell wrapper
(ffmpeg). No external sample sources, no licensing. Two variants:

  fire_first  fuller, slightly lower "thunk" - the first shot of a trigger pull
  fire_main   tighter, brighter - sustained fire, kept short so rapid shots
              layer cleanly instead of muddying

Layers per shot:
  1. core zap     downward freq sweep (sine + octave) = the "laser" body
  2. swept noise  white noise through a time-varying state-variable bandpass
                  whose center sweeps down = the airy "whisp/zip"
  3. air tail     gentle highpassed noise, slow decay = lingering breath
  4. sub thump    low sine, very fast decay = weight/impact
Sum -> soft saturation -> normalize -> de-click fades.
"""
import math
import struct
import wave
import sys
import numpy as np

SR = 44100


def exp_decay(n, tau_ms):
    t = np.arange(n) / SR
    return np.exp(-t / (tau_ms / 1000.0))


def attack(n, attack_ms):
    t = np.arange(n) / SR
    return np.clip(t / (attack_ms / 1000.0), 0.0, 1.0)


def svf_bandpass_swept(x, fc_curve, q):
    """Chamberlin state-variable filter, bandpass tap, per-sample center freq.

    fc_curve: array same length as x (Hz). q: resonance (damping = 1/q).
    """
    n = len(x)
    out = np.empty(n)
    low = band = 0.0
    damp = 1.0 / q
    for i in range(n):
        f = 2.0 * math.sin(math.pi * min(fc_curve[i], SR / 2.5) / SR)
        high = x[i] - low - damp * band
        band = f * high + band
        low = f * band + low
        out[i] = band
    return out


def sweep(n, f0, f1, sweep_ms):
    """Exponential frequency curve f0 -> f1 over sweep_ms, then hold f1."""
    t = np.arange(n) / SR
    k = np.minimum(t / (sweep_ms / 1000.0), 1.0)
    return f0 * (f1 / f0) ** k


def pew(dur_ms, seed, *, zap_f0, zap_f1, zap_sweep, noise_f0, noise_f1,
        noise_q, sub_f, sub_amp, zap_amp, noise_amp, air_amp, drive=2.6):
    rng = np.random.default_rng(seed)
    n = int(SR * dur_ms / 1000.0)
    out = np.zeros(n)

    # 1) core zap: downward sweep, sine + octave harmonic
    freq = sweep(n, zap_f0, zap_f1, zap_sweep)
    phase = 2 * np.pi * np.cumsum(freq) / SR
    zap = np.sin(phase) + 0.35 * np.sin(2 * phase) + 0.12 * np.sin(3 * phase)
    zap *= attack(n, 1.5) * exp_decay(n, dur_ms * 0.32)
    out += zap_amp * zap

    # 2) swept airy noise (the whisp): noise -> time-varying bandpass sweeping
    # down. Decays slow so the *falling* band carries the tail (keeps the shot
    # reading as a downward zap, not opening up).
    noise = rng.standard_normal(n)
    fc = sweep(n, noise_f0, noise_f1, dur_ms * 0.70)
    whisp = svf_bandpass_swept(noise, fc, noise_q)
    whisp *= attack(n, 2.0) * exp_decay(n, dur_ms * 0.60)
    # normalize the filtered noise so layer levels are predictable
    peak = np.max(np.abs(whisp)) or 1.0
    whisp /= peak
    out += noise_amp * whisp

    # 3) air tail: gentle mid-high band noise (the "breath"), kept well below
    # the zap so it textures rather than dominates the back half
    air = rng.standard_normal(n)
    air_fc = np.full(n, 3800.0)
    air = svf_bandpass_swept(air, air_fc, 1.2)
    air *= attack(n, 4.0) * exp_decay(n, dur_ms * 0.5)
    apeak = np.max(np.abs(air)) or 1.0
    air /= apeak
    out += air_amp * air

    # 4) sub thump for weight
    t = np.arange(n) / SR
    sub = np.sin(2 * np.pi * sub_f * t) * exp_decay(n, 45)
    out += sub_amp * sub

    # glue + density: soft saturation. Higher drive compresses peaks so the
    # post-normalize RMS rises (perceived loudness) without hard clipping -
    # this is what makes the shot read as loud as the game's impact sounds.
    out = np.tanh(drive * out)

    # normalize to -1 dBFS
    peak = np.max(np.abs(out)) or 1.0
    out = out / peak * (10 ** (-1.0 / 20))

    # de-click fades
    fa = int(SR * 0.001)
    fb = int(SR * 0.010)
    out[:fa] *= np.linspace(0, 1, fa)
    out[-fb:] *= np.linspace(1, 0, fb)
    return out.astype(np.float32)


def norm_fades(out, peak_db=-1.0, fade_in_ms=1.0, fade_out_ms=12.0):
    """Normalize to a target peak (dBFS) and de-click both ends."""
    peak = np.max(np.abs(out)) or 1.0
    out = out / peak * (10 ** (peak_db / 20))
    fa = max(1, int(SR * fade_in_ms / 1000))
    fb = max(1, int(SR * fade_out_ms / 1000))
    out[:fa] *= np.linspace(0, 1, fa)
    out[-fb:] *= np.linspace(1, 0, fb)
    return out.astype(np.float32)


def nz(n, rng):
    return rng.standard_normal(n)


def unit(x):
    return x / (np.max(np.abs(x)) or 1.0)


def click(n, at_ms, dur_ms, freq, amp, seed):
    """A soft electronic transient (tone + noise, fast decay) placed at at_ms."""
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    start = int(SR * at_ms / 1000)
    ln = int(SR * dur_ms / 1000)
    if start >= n:
        return out
    ln = min(ln, n - start)
    t = np.arange(ln) / SR
    env = np.exp(-t / (max(dur_ms * 0.3, 3) / 1000))
    body = (np.sin(2 * np.pi * freq * t) + 0.5 * rng.standard_normal(ln)) * env
    out[start:start + ln] += amp * body
    return out


def whizby(dur_ms=500, seed=5):
    """Bullet pass-by: a doppler whoosh that swells in, peaks, and falls away,
    its band sweeping downward (approaching->departing). Airy, soft."""
    n = int(SR * dur_ms / 1000)
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    t = np.linspace(0, 1, n)
    # asymmetric swell: rise to ~42%, exp fall after (the pass)
    peak_at = 0.42
    env = np.where(t < peak_at, t / peak_at,
                   np.exp(-(t - peak_at) / 0.22)) ** 1.3
    # descending band noise (doppler)
    fc = sweep(n, 4200, 1050, dur_ms * 0.9)
    out += 0.85 * unit(svf_bandpass_swept(nz(n, rng), fc, 4.0)) * env
    # faint tonal core, same descent
    freq = sweep(n, 1500, 520, dur_ms * 0.9)
    out += 0.30 * np.sin(2 * np.pi * np.cumsum(freq) / SR) * env
    # high air
    out += 0.12 * unit(svf_bandpass_swept(nz(n, rng), np.full(n, 6000.0), 1.2)) * env
    out = np.tanh(1.7 * out)
    return norm_fades(out, peak_db=-3.0, fade_out_ms=22)


def reload_start(dur_ms=2400, seed=31):
    """Reload begin: power-down whine + pneumatic release + a low charge hum
    that builds for the rest of the sequence, plus a soft unlatch."""
    n = int(SR * dur_ms / 1000)
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    t = np.arange(n) / SR
    # power-down descending tone (first 0.7s)
    pn = int(SR * 0.7)
    freq = sweep(pn, 900, 160, 700)
    ph = 2 * np.pi * np.cumsum(freq) / SR
    out[:pn] += 0.5 * (np.sin(ph) + 0.3 * np.sin(2 * ph)) * exp_decay(pn, 360)
    # pneumatic hiss release (~0.15-0.7s)
    hiss = unit(svf_bandpass_swept(nz(n, rng), np.full(n, 5000.0), 1.0))
    henv = np.zeros(n)
    h0, h1 = int(SR * 0.15), int(SR * 0.7)
    henv[h0:h1] = np.hanning(h1 - h0)
    out += 0.28 * hiss * henv
    # building charge hum (rises through the back half)
    hf = sweep(n, 70, 135, dur_ms)
    hph = 2 * np.pi * np.cumsum(hf) / SR
    hum_env = np.clip((t - 0.6) / (dur_ms / 1000 - 0.6), 0, 1) ** 1.4
    out += 0.32 * (np.sin(hph) + 0.4 * np.sin(2 * hph)) * hum_env
    out += click(n, 100, 70, 180, 0.45, seed + 1)  # unlatch
    out = np.tanh(1.5 * out)
    return norm_fades(out, peak_db=-5.0, fade_out_ms=50)


def reload_clip_out(dur_ms=600, seed=33):
    """Cell eject: descending servo whir + airy down-whoosh + soft release tk."""
    n = int(SR * dur_ms / 1000)
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    freq = sweep(n, 700, 250, dur_ms * 0.8)
    ph = 2 * np.pi * np.cumsum(freq) / SR
    servo = (np.sin(ph) + 0.25 * np.sin(3 * ph)) * exp_decay(n, dur_ms * 0.5)
    out += 0.45 * servo
    fc = sweep(n, 3500, 850, dur_ms * 0.9)
    out += 0.5 * unit(svf_bandpass_swept(nz(n, rng), fc, 3.0)) * exp_decay(n, dur_ms * 0.6)
    out += click(n, dur_ms * 0.78, 60, 150, 0.5, seed + 1)  # cell clears
    out = np.tanh(1.5 * out)
    return norm_fades(out, peak_db=-5.0, fade_out_ms=25)


def reload_clip_in(dur_ms=1000, seed=35):
    """Cell insert: rising servo whir + firm seat 'k-chunk' + charge confirm tick."""
    n = int(SR * dur_ms / 1000)
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    freq = sweep(n, 240, 640, dur_ms * 0.65)
    ph = 2 * np.pi * np.cumsum(freq) / SR
    senv = np.clip(np.linspace(0, 1, n) * 1.6, 0, 1)
    out += 0.4 * (np.sin(ph) + 0.25 * np.sin(2 * ph)) * senv
    out += 0.4 * unit(svf_bandpass_swept(nz(n, rng), sweep(n, 1200, 3200, dur_ms * 0.65), 2.5)) * senv
    out += click(n, dur_ms * 0.66, 90, 130, 0.7, seed + 1)  # seat k-chunk
    out += click(n, dur_ms * 0.90, 40, 2200, 0.3, seed + 2)  # charge tick
    out = np.tanh(1.5 * out)
    return norm_fades(out, peak_db=-5.0, fade_out_ms=30)


def reload_end(dur_ms=1350, seed=37):
    """Weapon ready: charge-up swell resolving to a bright two-note ready blip."""
    n = int(SR * dur_ms / 1000)
    rng = np.random.default_rng(seed)
    out = np.zeros(n)
    t = np.arange(n) / SR
    # rising charge: filtered noise + rising tone over the first ~70%
    chg_end = 0.72
    cenv = np.clip(t / (dur_ms / 1000 * chg_end), 0, 1) ** 2
    out += 0.4 * unit(svf_bandpass_swept(nz(n, rng), sweep(n, 800, 4200, dur_ms * chg_end), 2.0)) * cenv
    rf = sweep(n, 180, 900, dur_ms * chg_end)
    out += 0.3 * np.sin(2 * np.pi * np.cumsum(rf) / SR) * cenv
    # ready blip: two short tones at the end (a "powered, set" cue)
    out += click(n, dur_ms * chg_end, 90, 880, 0.6, seed + 1)
    out += click(n, dur_ms * chg_end + 110, 120, 1320, 0.6, seed + 2)
    out = np.tanh(1.5 * out)
    return norm_fades(out, peak_db=-4.0, fade_out_ms=40)


def write_wav(path, sig):
    pcm = np.clip(sig, -1, 1)
    pcm = (pcm * 32767.0).astype('<i2')
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


def report(name, sig):
    rms = math.sqrt(float(np.mean(sig.astype(np.float64) ** 2)))
    peak = float(np.max(np.abs(sig)))
    # rough spectral centroid in first vs second half (sweep should fall)
    def centroid(x):
        sp = np.abs(np.fft.rfft(x * np.hanning(len(x))))
        fr = np.fft.rfftfreq(len(x), 1 / SR)
        return float(np.sum(fr * sp) / (np.sum(sp) or 1))
    half = len(sig) // 2
    print(f"  {name:11s} {len(sig)/SR*1000:6.1f}ms  peak={peak:.3f} "
          f"rms={rms:.3f}  centroid {centroid(sig[:half]):.0f}->{centroid(sig[half:]):.0f}Hz")


if __name__ == '__main__':
    # First shot: fuller, lower landing, a touch more sub
    first = pew(300, seed=11,
                zap_f0=2300, zap_f1=360, zap_sweep=70,
                noise_f0=6500, noise_f1=950, noise_q=5.5,
                sub_f=88, sub_amp=0.55,
                zap_amp=0.52, noise_amp=0.60, air_amp=0.11)
    # Sustained: tighter + a touch brighter so it cuts through rapid fire
    main = pew(225, seed=23,
               zap_f0=2600, zap_f1=480, zap_sweep=55,
               noise_f0=7000, noise_f1=1450, noise_q=6.0,
               sub_f=100, sub_amp=0.38,
               zap_amp=0.48, noise_amp=0.62, air_amp=0.09)

    sounds = {
        'fire_first': first,
        'fire_main': main,
        'whizby': whizby(500, seed=5),
        'reload_start': reload_start(2400, seed=31),
        'reload_clip_out': reload_clip_out(600, seed=33),
        'reload_clip_in': reload_clip_in(1000, seed=35),
        'reload_end': reload_end(1350, seed=37),
    }
    print("synthesized (mono 44.1k; fire hot at -1 dBFS, foley mixed lower):")
    for name, sig in sounds.items():
        write_wav(f'{name}.wav', sig)
        report(name, sig)
