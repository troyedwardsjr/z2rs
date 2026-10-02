// z2rs audio ring buffer — AudioWorkletProcessor, no dependencies.
//
// The main thread posts Float32Array mono chunks as the emulator steps
// frames; this processor FIFO-queues them and emits silence on underrun.
// About four times a second it reports its counters back (see `report()`),
// so the status line, `z2.ext.audio()` and QA scripts can tell real sound
// from a silent pipeline, and a rate mismatch from a healthy run.
//
// The ring itself is the `Ring` class, exported so the page can run the same
// code on the main thread behind a ScriptProcessorNode where AudioWorklet is
// missing (Safari before 14.1, or a page that is not a secure context).
//
// ## Why the queue is capped hard (the 1-2 s delay this fixes)
//
// The queue *is* the latency: a sample posted here is not heard until the
// audio thread has walked past everything already in front of it, so a queue
// N samples deep means the sound trails the picture by N / rate seconds.
//
// Production and consumption run at the same long-term rate — the page steps
// 60.0988 NES frames per wall-clock second and each frame renders
// `rate / 60.0988` samples, so it posts exactly `rate` samples per second,
// and `process()` consumes exactly `rate` samples per second. Equal rates
// mean the depth has no restoring force of its own: whatever backlog it
// happens to be carrying, it carries forever.
//
// And the depth only ever ratchets *up*. Every underrun — a main-thread
// stall, a GC pause, a slow first paint — emits silence in place of the
// samples that had not arrived yet, but those samples are not dropped: they
// arrive a moment later and queue up behind the gap. So each dropout adds
// its own length to the delay, permanently.
//
// The fix is a latency target rather than a memory guard: once the queue
// passes the cap, drop the oldest samples until it is back at the target.
// One resync costs a few tens of milliseconds of audio (a click at worst)
// and buys back every second that would otherwise have followed it. In a
// healthy run `dropped` stays at 0: a steadily climbing `dropped` means the
// page produces more than the device plays (a rate mismatch), a steadily
// climbing `underruns` the reverse (or a page too slow to keep 60 fps).
//
// ## Rates
//
// `srcRate` is what the synth renders at, `outRate` what the context plays
// at. The page asks the synth for the context's own rate, so the two are
// normally equal and samples are copied straight through. If they ever
// differ (a context rate outside the synth's 8-192 kHz range, or a browser
// that ignored the rate it was asked for), the ring resamples linearly
// instead of playing the samples at the wrong speed — the old failure mode
// was music played sharp and fast with constant underruns.

// Steady-state depth to aim for. Three NES frames (~50 ms) covers the
// 128-sample render quantum (2.9 ms), a missed display frame and the jitter
// of posting a whole frame's samples at a time, without being audible as lag.
const TARGET_MS = 50;
// Each underrun after playback started raises the target by this much (a
// device with a big callback buffer, or a display slower than 60 Hz, needs
// more headroom), up to TARGET_MAX_MS.
const TARGET_STEP_MS = 10;
const TARGET_MAX_MS = 150;
// Resync slack above the target. Above target + slack the backlog is real
// drift, not jitter. The slack also covers twice the biggest recent chunk
// the page posted (a 30 Hz display posts two frames at a time), capped at
// SLACK_MAX_MS, so a one-off big post (a QA script stepping frames by hand)
// is still trimmed straight back to the target.
const SLACK_MS = 40;
const SLACK_MAX_MS = 100;
// Clock trim. The page paces frames by the system clock and the device plays
// by its own crystal; the two differ by tens of ppm, so even with exactly
// matched rates the queue creeps, and every hard resync is an audible skip.
// When the smoothed depth wanders more than TRIM_BAND_MS from its centre,
// play 0.1% faster or slower (1.7 cents, inaudible) until it is back within
// a third of the band. Inside the band samples are copied untouched.
const TRIM_BAND_MS = 15;
const TRIM = 0.001;

export class Ring {
  constructor(srcRate, outRate) {
    this.srcRate = srcRate;
    this.outRate = outRate;
    this.step = srcRate / outRate; // source samples per output sample
    this.resampling = srcRate !== outRate;
    this.target = Math.round((srcRate * TARGET_MS) / 1000);
    this.targetMax = Math.round((srcRate * TARGET_MAX_MS) / 1000);
    this.targetStep = Math.round((srcRate * TARGET_STEP_MS) / 1000);
    this.slack = Math.round((srcRate * SLACK_MS) / 1000);
    this.slackMax = Math.round((srcRate * SLACK_MAX_MS) / 1000);
    this.biggest = 0; // biggest chunk posted recently (decays at each report)
    this.band = Math.round((srcRate * TRIM_BAND_MS) / 1000);
    this.level = 0; // smoothed queue depth
    this.trim = 0; // -1, 0, +1: playing slower / exact / faster
    this.trimmed = 0; // render calls spent trimming
    this.queue = [];
    // Unplayed samples across the whole queue (the head's already-played part
    // is not counted).
    this.pending = 0;
    this.headOff = 0;
    this.underruns = 0;
    this.dropped = 0;
    this.skipped = 0; // trimmed before playback started (silent)
    this.received = 0; // source samples posted in
    this.consumed = 0; // source samples played out (or resampled from)
    this.rendered = 0; // output frames produced, silence included
    this.played = false; // left priming at least once
    // Hold output at silence until the first `target` samples are in hand.
    // Starting against an empty queue would underrun on every quantum until
    // the page's first post lands, and those are counted underruns that say
    // nothing about the run's health.
    this.priming = true;
    this.peak = 0;
    // Linear-interpolation state (resampling only).
    this.frac = 0;
    this.prev = 0;
    this.next = 0;
  }

  push(chunk) {
    if (!chunk || !chunk.length) return;
    this.queue.push(chunk);
    this.pending += chunk.length;
    this.received += chunk.length;
    if (chunk.length > this.biggest) this.biggest = chunk.length;
    const cap = this.target + Math.min(this.slackMax, Math.max(this.slack, 2 * this.biggest));
    if (this.pending > cap) {
      const before = this.dropped;
      this.dropOldest(this.pending - this.target);
      // In the first second (the port delivers the posts that queued up
      // while the processor was being built in one go, and the page's first
      // frames land in a burst) the trim is the start-up settling, not a
      // fault: count it apart, so `dropped` means skips in a running game.
      if (this.rendered < this.outRate) { this.skipped += this.dropped - before; this.dropped = before; }
    }
  }

  // Discard the `n` oldest unplayed samples: whole chunks while they fit,
  // then part of the head.
  dropOldest(n) {
    while (n > 0 && this.queue.length) {
      const head = this.queue[0];
      const avail = head.length - this.headOff;
      if (avail > n) {
        this.headOff += n;
        this.pending -= n;
        this.dropped += n;
        return;
      }
      this.queue.shift();
      this.headOff = 0;
      this.pending -= avail;
      this.dropped += avail;
      n -= avail;
    }
  }

  underrun(out, i) {
    out.fill(0, i);
    this.underruns += 1;
    // Fully drained: rebuild the target depth before playing again. Playing
    // on from an empty queue leaves it hovering at zero, which is an
    // underrun every 2.9 ms quantum — continuous crackle. Priming costs one
    // stretch of silence and cannot ratchet the delay up, because it refills
    // to the target, not to the target past where it was.
    this.priming = true;
    if (this.played) this.target = Math.min(this.targetMax, this.target + this.targetStep);
  }

  // One source sample, or undefined when the queue is dry.
  pop() {
    const head = this.queue[0];
    if (!head) return undefined;
    const v = head[this.headOff++];
    this.pending -= 1;
    this.consumed += 1;
    if (this.headOff >= head.length) {
      this.queue.shift();
      this.headOff = 0;
    }
    return v;
  }

  // Fill `out` (one channel, any length) with the next output samples.
  render(out) {
    this.rendered += out.length;
    if (this.priming) {
      if (this.pending < this.target) {
        out.fill(0);
        return;
      }
      this.priming = false;
      this.played = true;
      this.level = this.pending;
      this.trim = 0;
    }
    // Clock trim (see TRIM_BAND_MS). The depth saw-tooths by one posted
    // chunk between posts, so its centre is target + half a chunk.
    this.level += (this.pending - this.level) * 0.02;
    const err = this.level - (this.target + this.biggest / 2);
    if (this.trim === 0) {
      if (err > this.band) this.trim = 1;
      else if (err < -this.band) this.trim = -1;
    } else if (Math.abs(err) < this.band / 3) {
      this.trim = 0;
    }
    if (this.trim) this.trimmed += 1;
    const step = this.step * (1 + this.trim * TRIM);
    let i = 0;
    if (step === 1) {
      while (i < out.length) {
        const head = this.queue[0];
        if (!head) { this.underrun(out, i); break; }
        const take = Math.min(head.length - this.headOff, out.length - i);
        out.set(head.subarray(this.headOff, this.headOff + take), i);
        i += take;
        this.headOff += take;
        this.pending -= take;
        this.consumed += take;
        if (this.headOff >= head.length) {
          this.queue.shift();
          this.headOff = 0;
        }
      }
      // Hand the interpolator a continuous starting point.
      if (i > 0) { this.next = out[i - 1]; this.prev = this.next; this.frac = 0; }
    } else {
      for (; i < out.length; i++) {
        this.frac += step;
        let dry = false;
        while (this.frac >= 1) {
          const v = this.pop();
          if (v === undefined) { dry = true; break; }
          this.prev = this.next;
          this.next = v;
          this.frac -= 1;
        }
        if (dry) { this.frac = 0; this.underrun(out, i); break; }
        out[i] = this.prev + (this.next - this.prev) * this.frac;
      }
    }
    for (let k = 0; k < out.length; k++) {
      const v = Math.abs(out[k]);
      if (v > this.peak) this.peak = v;
    }
  }

  // Counters for the page. `peak` is the loudest output sample since the
  // last report; everything else is cumulative except `queued`/`target`.
  report() {
    const r = {
      underruns: this.underruns,
      queued: this.pending,
      peak: this.peak,
      dropped: this.dropped,
      skipped: this.skipped,
      received: this.received,
      consumed: this.consumed,
      rendered: this.rendered,
      target: this.target,
      srcRate: this.srcRate,
      outRate: this.outRate,
      resampling: this.resampling,
      trim: this.trim,
      trimmed: this.trimmed,
    };
    this.peak = 0;
    // Let the burst allowance relax again after a one-off catch-up post.
    this.biggest = Math.floor(this.biggest * 0.9);
    return r;
  }
}

// Only inside an AudioWorkletGlobalScope: the page imports this same file on
// the main thread for the ScriptProcessor fallback, where neither exists.
if (typeof AudioWorkletProcessor !== 'undefined' && typeof registerProcessor === 'function') {
  class Z2Ring extends AudioWorkletProcessor {
    constructor(options) {
      super();
      const o = (options && options.processorOptions) || {};
      // `sampleRate` is the AudioWorkletGlobalScope's real context rate; the
      // page passes the rate the synth renders at.
      this.ring = new Ring(o.srcRate || o.rate || sampleRate, sampleRate);
      this.reportEvery = Math.max(1, Math.round((sampleRate * 0.25) / 128)); // ≈ 0.25 s
      this.calls = 0;
      this.port.onmessage = (e) => this.ring.push(e.data);
    }

    process(_inputs, outputs) {
      const out = outputs[0][0]; // mono
      this.ring.render(out);
      if (++this.calls >= this.reportEvery) {
        this.calls = 0;
        this.port.postMessage(this.ring.report());
      }
      return true;
    }
  }
  registerProcessor('z2-ring', Z2Ring);
}
