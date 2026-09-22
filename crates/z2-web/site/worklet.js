// z2rs audio ring buffer — AudioWorkletProcessor, no dependencies.
//
// The main thread posts Float32Array mono chunks as the emulator steps
// frames; this processor FIFO-queues them and emits silence on underrun.
// About four times a second it reports `{underruns, queued, peak, dropped}`
// back (peak = loudest output sample since the last report), so the status
// line and QA scripts can tell real sound from a silent pipeline.
//
// ## Why the queue is capped hard (the 1-2 s delay this fixes)
//
// The queue *is* the latency: a sample posted here is not heard until the
// audio thread has walked past everything already in front of it, so a queue
// N samples deep means the sound trails the picture by N / sampleRate
// seconds.
//
// Production and consumption run at the same long-term rate — the page steps
// 60.0988 NES frames per wall-clock second and each frame renders
// `sampleRate / 60.0988` samples, so it posts exactly `sampleRate` samples
// per second, and `process()` consumes exactly `sampleRate` samples per
// second. Equal rates mean the depth has no restoring force of its own:
// whatever backlog it happens to be carrying, it carries forever.
//
// And the depth only ever ratchets *up*. Every underrun — a main-thread
// stall, a GC pause, a slow first paint — emits silence in place of the
// samples that had not arrived yet, but those samples are not dropped: they
// arrive a moment later and queue up behind the gap. So each dropout adds
// its own length to the delay, permanently: the page's rAF accumulator still
// owes those frames (capped at half a second per stalled tick), and it posts
// them once it runs again, while the audio thread spent the stall emitting
// silence it never gets back. A few stalls in a row and the delay walks up to
// the only bound the old code had — a 2 s memory guard — which is exactly the
// second or two of lag a long session ended up with.
//
// The fix is a latency target rather than a memory guard: once the queue
// passes MAX_MS, drop the oldest samples until it is back at TARGET_MS. One
// resync costs a few tens of milliseconds of audio (a click at worst) and
// buys back every second that would otherwise have followed it.

const REPORT_EVERY = 86; // process() calls: 86 * 128 / 44100 ≈ 0.25 s

// Steady-state depth to aim for. Three NES frames (~50 ms) covers the
// 128-sample render quantum (2.9 ms), a missed display frame and the jitter
// of posting a whole frame's samples at a time, without being audible as lag.
const TARGET_MS = 50;
// Resync threshold. Above this the backlog is real drift, not jitter.
const MAX_MS = 90;

class Z2Ring extends AudioWorkletProcessor {
  constructor(options) {
    super();
    // `sampleRate` is the AudioWorkletGlobalScope's context rate; the page
    // passes it explicitly too so the two can never disagree.
    const rate = (options && options.processorOptions && options.processorOptions.rate) || sampleRate;
    this.target = Math.round((rate * TARGET_MS) / 1000);
    this.max = Math.round((rate * MAX_MS) / 1000);
    this.queue = [];
    // Unplayed samples across the whole queue (the head's already-played part
    // is not counted).
    this.pending = 0;
    this.headOff = 0;
    this.underruns = 0;
    this.dropped = 0;
    // Hold output at silence until the first TARGET samples are in hand.
    // Starting against an empty queue would underrun on every quantum until
    // the page's first post lands, and those are counted underruns that say
    // nothing about the run's health.
    this.priming = true;
    this.peak = 0;
    this.calls = 0;
    this.port.onmessage = (e) => {
      const chunk = e.data;
      if (chunk && chunk.length) {
        this.queue.push(chunk);
        this.pending += chunk.length;
        if (this.pending > this.max) this.dropOldest(this.pending - this.target);
      }
    };
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

  process(_inputs, outputs) {
    const out = outputs[0][0]; // mono
    if (this.priming) {
      if (this.pending < this.target) {
        out.fill(0);
        return true;
      }
      this.priming = false;
    }
    let i = 0;
    while (i < out.length) {
      const head = this.queue[0];
      if (!head) {
        out.fill(0, i);
        this.underruns += 1;
        // Fully drained: rebuild the target depth before playing again.
        // Playing on from an empty queue leaves it hovering at zero, which
        // is an underrun every 2.9 ms quantum — continuous crackle. Priming
        // costs one stretch of silence and cannot ratchet the delay up,
        // because it refills to TARGET, not to TARGET past where it was.
        this.priming = true;
        break;
      }
      const take = Math.min(head.length - this.headOff, out.length - i);
      out.set(head.subarray(this.headOff, this.headOff + take), i);
      i += take;
      this.headOff += take;
      this.pending -= take;
      if (this.headOff >= head.length) {
        this.queue.shift();
        this.headOff = 0;
      }
    }
    for (let k = 0; k < out.length; k++) {
      const v = Math.abs(out[k]);
      if (v > this.peak) this.peak = v;
    }
    this.calls += 1;
    if (this.calls >= REPORT_EVERY) {
      this.port.postMessage({
        underruns: this.underruns,
        queued: this.pending,
        peak: this.peak,
        dropped: this.dropped,
      });
      this.calls = 0;
      this.peak = 0;
    }
    return true;
  }
}

registerProcessor('z2-ring', Z2Ring);
