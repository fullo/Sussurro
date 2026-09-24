/* The notice before the first Start (#136): the side panel shows it before
   capture starts, until "Don't show this again" is ticked; the options page
   brings it back. The wording is the app's (`@sussurro/notice`, one place
   for both); the acknowledgement lives in `storage.local` under one key,
   read and written only here. Clearing the extension's storage (or
   reinstalling it) shows the notice again. */
import browser from "webextension-polyfill";
import { noticeAnswer, noticeNeeded } from "@sussurro/notice";

export { DONT_SHOW_AGAIN_DEFAULT, RECORDING_NOTICE, RECORDING_PRIVACY_URL } from "@sussurro/notice";

/** `storage.local` key of the acknowledgement (`true` once remembered). */
export const NOTICE_KEY = "recordingNoticeSeen";

/** The subset of `storage.local` used here (injectable for tests). */
export interface NoticeStorage {
  get(keys: string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove(keys: string[]): Promise<void>;
}

const local = (): NoticeStorage => browser.storage.local;

/** Whether the next Start must show the notice. A storage that can't be
 *  read asks (never skips the notice by accident). */
export async function isNoticeNeeded(storage: NoticeStorage = local()): Promise<boolean> {
  try {
    return noticeNeeded((await storage.get([NOTICE_KEY]))[NOTICE_KEY]);
  } catch {
    return true;
  }
}

/** Store the answer: remembered only when the user went ahead with "Don't
 *  show this again" ticked. Returns whether it is now remembered. */
export async function answerNotice(proceed: boolean, dontShowAgain: boolean, storage: NoticeStorage = local()): Promise<boolean> {
  if (noticeAnswer(proceed, dontShowAgain) === null) return false;
  await storage.set({ [NOTICE_KEY]: true });
  return true;
}

/** Options → "Show the notice again". */
export async function resetNotice(storage: NoticeStorage = local()): Promise<void> {
  await storage.remove([NOTICE_KEY]);
}

/** What a click on Start does: ask first, or start. Unknown (still
 *  loading) asks. Pure. */
export function startStep(needed: boolean | undefined): "ask" | "start" {
  return needed === false ? "start" : "ask";
}

/** Call `cb` with whether the notice is needed whenever its key changes
 *  (the options page reset it, another panel remembered it). */
export function onNoticeChanged(cb: (needed: boolean) => void): () => void {
  const listener = (changes: Record<string, { newValue?: unknown }>, area: string) => {
    if (area !== "local" || !(NOTICE_KEY in changes)) return;
    cb(noticeNeeded(changes[NOTICE_KEY]?.newValue));
  };
  browser.storage.onChanged.addListener(listener);
  return () => browser.storage.onChanged.removeListener(listener);
}
