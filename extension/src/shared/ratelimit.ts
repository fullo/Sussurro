/* Rate caps on what the meeting page can send (#217). The MAIN-world hook
 * runs in the page's own world, so a page script (or an XSS on the page)
 * can post anything on its port while a capture runs: the ISOLATED relay
 * and the background both cap it, and the app caps it again per
 * connection (`api/live.rs`). Pure — unit tested. */

/** A token bucket: `burst` at once, then `perSec`. `now` in ms. */
export class RateLimiter {
  private readonly burst: number;
  private readonly perSec: number;
  private readonly now: () => number;
  private tokens: number;
  private last: number;

  constructor(burst: number, perSec: number, now: () => number = () => performance.now()) {
    this.burst = burst;
    this.perSec = perSec;
    this.now = now;
    this.tokens = burst;
    this.last = now();
  }

  /** Take one token if there is one. */
  allow(): boolean {
    const t = this.now();
    const dt = Math.max(0, t - this.last) / 1000;
    this.last = Math.max(this.last, t);
    this.tokens = Math.min(this.burst, this.tokens + dt * this.perSec);
    if (this.tokens >= 1) {
      this.tokens -= 1;
      return true;
    }
    return false;
  }
}

/** Speaker events (#131) from the page: the app takes 400 at once and 20
 *  per second (`EVENTS_BURST`, `EVENTS_PER_SEC`); the relay allows less. */
export const PAGE_EVENTS = { burst: 200, perSec: 20 } as const;
export const BACKGROUND_EVENTS = { burst: 400, perSec: 20 } as const;
/** State and arming messages from the page. */
export const PAGE_CONTROL = { burst: 50, perSec: 10 } as const;
/** Audio blocks from the page's worklet: ~23 per second at 48 kHz
 *  (FRAME_SAMPLES = 2048), ~47 at 96 kHz. A dropped block becomes a gap
 *  the app fills with silence. */
export const PAGE_AUDIO = { burst: 100, perSec: 60 } as const;
/** Bytes of one channel of one audio block (2048 i16 samples are 4 KiB). */
export const MAX_BLOCK_BYTES = 64 * 1024;
/** Characters of an error the page reports. */
export const MAX_ERROR_CHARS = 300;

/** A page frame counter: a non-negative safe integer, else null. */
export function pageSeq(v: unknown): number | null {
  return typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : null;
}
