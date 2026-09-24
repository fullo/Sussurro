/* The notice shown before the first recording of other people (#136):
   the app's meeting paths (System audio + mic, "Meeting in the room") and
   the extension's side panel (Start). One wording for both, kept here so a
   translation touches one file.

   Shared with the extension, which imports it as `@sussurro/notice` (Vite
   alias + tsconfig path, like `@sussurro/pairing`). Pure — no imports.

   The wording is deliberately neutral: it says that rules may apply, never
   which ones, and gives no legal advice. */

/** The README section on recording meetings (the notice's "read more"). */
export const RECORDING_PRIVACY_URL = "https://github.com/fullo/Sussurro#recording-meetings-and-consent";

/** Every string of the notice and of the reminder line. */
export const RECORDING_NOTICE = {
  title: "You are about to record other people",
  body: [
    "This recording captures the voices of the other participants, not only yours.",
    "Depending on where you and they are, and on the rules of your organisation, you may need to tell them that you are recording, and some may have to agree first. Sussurro cannot tell which rules apply to you.",
  ],
  local: "The audio stays on this computer: Sussurro transcribes it locally.",
  readMore: "Recording meetings and privacy",
  dontShowAgain: "Don't show this again",
  proceed: "Start recording",
  cancel: "Cancel",
  /** The line shown while (and before) other people are recorded. */
  reminder: "Recording other people — let them know.",
  reminderMore: "Why?",
  /** Settings / options: bring the notice back. */
  reshow: "Show the notice again",
  reshowDone: "The notice will be shown before the next meeting recording.",
  reshowHint: "Shown before recording other people, until you tick “Don't show this again”.",
} as const;

/** "Don't show this again" starts ticked: the notice is meant for the first
 *  recording; unticking it keeps it for the next ones too. */
export const DONT_SHOW_AGAIN_DEFAULT = true;

/** Whether the notice must be shown before a recording of other people.
 *  `seen` is the stored acknowledgement: absent (a fresh install, cleared
 *  settings or storage) means show it. */
export function noticeNeeded(seen: unknown): boolean {
  return seen !== true;
}

/** What to store after the user answered the notice: `true` to remember
 *  the acknowledgement, `null` to store nothing (show it again next time).
 *  Cancelling never stores anything. */
export function noticeAnswer(proceed: boolean, dontShowAgain: boolean): true | null {
  return proceed && dontShowAgain ? true : null;
}
