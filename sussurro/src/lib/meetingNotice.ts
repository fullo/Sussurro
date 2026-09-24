/* The app side of the notice before recording other people (#136): when
   New asks first, what it stores, and where the reminder line shows. The
   wording lives in `recordingNotice.ts`, shared with the extension. Pure. */

import { noticeAnswer, noticeNeeded } from "./recordingNotice";
import type { Run } from "./engineRuns";
import type { Settings } from "./types";

type NoticeSettings = Pick<Settings, "meeting_notice_seen">;

/** Whether starting a recording of other people asks first: until the
 *  notice is acknowledged with "Don't show this again". Settings without
 *  the key (fresh install, cleared or pre-#136 settings) ask. */
export function needsMeetingNotice(settings: NoticeSettings): boolean {
  return noticeNeeded(settings.meeting_notice_seen);
}

/** The settings to save after the notice was answered, or null when there
 *  is nothing to save (cancelled, "Don't show this again" unticked, or
 *  already remembered). */
export function settingsAfterNotice<S extends NoticeSettings>(settings: S, proceed: boolean, dontShowAgain: boolean): S | null {
  if (noticeAnswer(proceed, dontShowAgain) === null || settings.meeting_notice_seen === true) return null;
  return { ...settings, meeting_notice_seen: true };
}

/** Settings → "Show the notice again". */
export function withNoticeReset<S extends NoticeSettings>(settings: S): S {
  return { ...settings, meeting_notice_seen: false };
}

/** Whether a capture run records other people, for the reminder line: a
 *  System audio + mic session always, a microphone session when it is a
 *  meeting in the room (#130). Files and links are not recordings. */
export function recordsOthers(run: Pick<Run, "kind" | "itemType"> | null): boolean {
  if (!run) return false;
  if (run.kind === "system") return true;
  return run.kind === "mic" && run.itemType === "meeting";
}
