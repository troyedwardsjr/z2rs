// z2rs audio ring buffer — AudioWorkletProcessor, no dependencies.
//
// The main thread posts Float32Array mono chunks (44.1 kHz) as the emulator
// steps frames; this processor FIFO-queues them and emits silence on
// underrun. About four times a second it reports `{underruns, queued, peak}`
// back (peak = loudest output sample since the last report), so the status
// line and QA scripts can tell real sound from a silent pipeline.

const REPORT_EVERY = 86; // process() calls: 86 * 128 / 44100 ≈ 0.25 s

class Z2Ring extends AudioWorkletProcessor {
  constructor() {
    super();
    this.queue = [];
    this.queued = 0;
    this.headOff = 0;
    this.underruns = 0;
    this.peak = 0;
    this.calls = 0;
    this.port.onmessage = (e) => {
      const chunk = e.data;
      if (chunk && chunk.length) {
        this.queue.push(chunk);
        this.queued += chunk.length;
        // Drop oldest audio past ~2 s so a stalled tab can't grow memory.
        // (Dropping the head invalidates headOff, so it resets too.)
        let droppedHead = false;
        while (this.queued > 88200 && this.queue.length > 1) {
          const dropped = this.queue.shift();
          this.queued -= dropped.length;
          droppedHead = true;
        }
        if (droppedHead) this.headOff = 0;
      }
    };
  }

  process(_inputs, outputs) {
    const out = outputs[0][0]; // mono
    let i = 0;
    while (i < out.length) {
      const head = this.queue[0];
      if (!head) {
        out.fill(0, i);
        this.underruns += 1;
        break;
      }
      const take = Math.min(head.length - this.headOff, out.length - i);
      out.set(head.subarray(this.headOff, this.headOff + take), i);
      i += take;
      this.headOff += take;
      if (this.headOff >= head.length) {
        this.queue.shift();
        this.headOff = 0;
        this.queued -= head.length;
      }
    }
    for (let k = 0; k < out.length; k++) {
      const v = Math.abs(out[k]);
      if (v > this.peak) this.peak = v;
    }
    this.calls += 1;
    if (this.calls >= REPORT_EVERY) {
      // `queued` counts whole chunks; the head's played part is not pending.
      const pending = this.queued - this.headOff;
      this.port.postMessage({ underruns: this.underruns, queued: pending, peak: this.peak });
      this.calls = 0;
      this.peak = 0;
    }
    return true;
  }
}

registerProcessor('z2-ring', Z2Ring);
