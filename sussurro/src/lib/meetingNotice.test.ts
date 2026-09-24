import { describe, expect, it } from "vitest";
import { needsMeetingNotice, recordsOthers, settingsAfterNotice, withNoticeReset } from "./meetingNotice";
import { DONT_SHOW_AGAIN_DEFAULT, RECORDING_NOTICE, RECORDING_PRIVACY_URL, noticeAnswer, noticeNeeded } from "./recordingNotice";
import { runsReducer, initialRuns } from "./engineRuns";

describe("recording notice: first time (#136)", () => {
  it("asks on a fresh install and after the settings were cleared", () => {
    expect(needsMeetingNotice({})).toBe(true);
    expect(needsMeetingNotice({ meeting_notice_seen: undefined })).toBe(true);
    expect(needsMeetingNotice({ meeting_notice_seen: false })).toBe(true);
    // Settings read back from a file without the key (cleared or older).
    const cleared = JSON.parse(JSON.stringify({ hotkey: "Alt+Space" })) as { meeting_notice_seen?: boolean };
    expect(needsMeetingNotice(cleared)).toBe(true);
  });

  it("stops asking once acknowledged with “Don't show this again”", () => {
    const first = { hotkey: "x", meeting_notice_seen: false };
    const next = settingsAfterNotice(first, true, true);
    expect(next).toEqual({ hotkey: "x", meeting_notice_seen: true });
    expect(needsMeetingNotice(next!)).toBe(false);
    // Nothing more to save once remembered.
    expect(settingsAfterNotice(next!, true, true)).toBeNull();
  });

  it("keeps asking when unticked, and never remembers a Cancel", () => {
    const first = { meeting_notice_seen: false };
    expect(settingsAfterNotice(first, true, false)).toBeNull();
    expect(settingsAfterNotice(first, false, true)).toBeNull();
    expect(settingsAfterNotice(first, false, false)).toBeNull();
    expect(needsMeetingNotice(first)).toBe(true);
  });

  it("shows again after “Show the notice again” in Settings", () => {
    const seen = { api_port: 4525, meeting_notice_seen: true };
    const reset = withNoticeReset(seen);
    expect(reset).toEqual({ api_port: 4525, meeting_notice_seen: false });
    expect(needsMeetingNotice(reset)).toBe(true);
  });

  it("shared rules: only a stored true counts; proceed + tick remembers", () => {
    expect(noticeNeeded(true)).toBe(false);
    for (const v of [undefined, null, false, "true", 1, {}]) expect(noticeNeeded(v)).toBe(true);
    expect(noticeAnswer(true, true)).toBe(true);
    expect(noticeAnswer(true, false)).toBeNull();
    expect(noticeAnswer(false, true)).toBeNull();
    expect(DONT_SHOW_AGAIN_DEFAULT).toBe(true);
  });

  it("wording stays neutral and links the README section", () => {
    expect(RECORDING_PRIVACY_URL).toBe("https://github.com/fullo/Sussurro#recording-meetings-and-consent");
    const all = Object.values(RECORDING_NOTICE).flat().join(" ");
    // No legal advice and no claims about specific jurisdictions.
    expect(all).not.toMatch(/\b(illegal|legal advice|lawyer|you must|country|state)\b/i);
    expect(all).not.toMatch(/\b(GDPR|EU|US|UK|CCPA)\b/);
    expect(all).toMatch(/may/);
  });
});

describe("reminder line: which runs record other people", () => {
  it("System audio + mic always; the mic only as a meeting in the room", () => {
    expect(recordsOthers(null)).toBe(false);
    expect(recordsOthers({ kind: "system" })).toBe(true);
    expect(recordsOthers({ kind: "mic", itemType: "meeting" })).toBe(true);
    expect(recordsOthers({ kind: "mic", itemType: "note" })).toBe(false);
    expect(recordsOthers({ kind: "mic" })).toBe(false);
    expect(recordsOthers({ kind: "file", itemType: "meeting" })).toBe(false);
    expect(recordsOthers({ kind: "link", itemType: "transcription" })).toBe(false);
  });

  it("the run learns its type when started and from engine-started", () => {
    let s = runsReducer(initialRuns, { type: "started", kind: "mic", sessionId: 3, label: "", now: 0, itemType: "meeting" });
    expect(recordsOthers(s.mic)).toBe(true);
    s = runsReducer(initialRuns, { type: "started", kind: "mic", sessionId: 4, label: "", now: 0 });
    expect(recordsOthers(s.mic)).toBe(false);
    s = runsReducer(s, {
      type: "engine-started",
      payload: { session_id: 4, item_id: "2026-09-25-room", item_type: "meeting", title: "", source: "mic" },
    });
    expect(s.mic?.itemType).toBe("meeting");
    expect(recordsOthers(s.mic)).toBe(true);
  });
});
