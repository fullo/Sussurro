import { describe, expect, it } from "vitest";
import { BACKGROUND_EVENTS, PAGE_EVENTS, RateLimiter, pageSeq } from "./ratelimit";

describe("RateLimiter", () => {
  it("allows a burst, then the rate", () => {
    let t = 0;
    const r = new RateLimiter(3, 2, () => t);
    expect([r.allow(), r.allow(), r.allow(), r.allow()]).toEqual([true, true, true, false]);
    t = 400;
    expect(r.allow()).toBe(false);
    t = 500;
    expect(r.allow()).toBe(true);
    expect(r.allow()).toBe(false);
    // A long pause refills to the burst, never beyond.
    t = 60_000;
    expect(Array.from({ length: 10 }, () => r.allow()).filter(Boolean)).toHaveLength(3);
    // A clock going backwards gives nothing.
    t = 1_000;
    expect(r.allow()).toBe(false);
  });

  it("caps a flood from the page to what the app accepts", () => {
    let t = 0;
    const r = new RateLimiter(PAGE_EVENTS.burst, PAGE_EVENTS.perSec, () => t);
    let sent = 0;
    // 10 000 events a second for 10 s.
    for (let i = 0; i < 100_000; i++) {
      t = i / 10;
      if (r.allow()) sent++;
    }
    expect(sent).toBeLessThanOrEqual(PAGE_EVENTS.burst + 10 * PAGE_EVENTS.perSec + 1);
    expect(PAGE_EVENTS.burst).toBeLessThanOrEqual(BACKGROUND_EVENTS.burst);
    expect(BACKGROUND_EVENTS).toEqual({ burst: 400, perSec: 20 }); // api/live.rs
  });
});

describe("pageSeq", () => {
  it("takes non-negative safe integers only", () => {
    expect(pageSeq(0)).toBe(0);
    expect(pageSeq(123)).toBe(123);
    for (const bad of [-1, 1.5, NaN, Infinity, 2 ** 60, "3", null, undefined, {}]) expect(pageSeq(bad)).toBeNull();
  });
});
