/* The version metadata sent to addons.mozilla.org with each signing
 * submission (#234), through `web-ext sign --amo-metadata <file>`:
 * https://mozilla.github.io/addons-server/topics/api/addons.html (version
 * create). Pure; scripts/amo-metadata.ts writes the file. Unit-tested in
 * amo.test.ts. */

/** AMO's limit for `approval_notes`. */
export const APPROVAL_NOTES_MAX = 3000;

export type AmoMetadata = {
  version: {
    /** Only Mozilla's reviewers see these. */
    approval_notes: string;
    /** Application names: min/max versions then come from the manifest.
     *  Firefox (desktop) only: the add-on needs the sidebar and a local
     *  Sussurro app, so it must not be marked Android-compatible. */
    compatibility: ["firefox"];
  };
};

/** The metadata for a submission, from AMO-REVIEWER-NOTES.md's text. */
export function amoMetadata(notes: string): AmoMetadata {
  const approval_notes = notes.replace(/\r\n/g, "\n").trim();
  if (!approval_notes) throw new Error("the reviewer notes are empty");
  if (approval_notes.length > APPROVAL_NOTES_MAX) {
    throw new Error(`the reviewer notes are ${approval_notes.length} characters; AMO accepts at most ${APPROVAL_NOTES_MAX}`);
  }
  return { version: { approval_notes, compatibility: ["firefox"] } };
}
