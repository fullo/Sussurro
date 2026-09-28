/**
 * "The archive folder can't be read" (#328).
 *
 * When the archive root is missing, not a folder or refused by the OS, the
 * backend answers the archive commands with a JSON error carrying
 * `code: "archive_unreadable"` (`archive::unreadable::ui_error`) instead of
 * an empty list, so the Library can say what happened instead of looking
 * empty. Pure helpers: parsing and the Library's copy.
 */

export type UnreadableKind = "missing" | "not_a_directory" | "permission_denied" | "other";

export interface ArchiveUnreadable {
  code: "archive_unreadable";
  /** The archive folder. */
  path: string;
  kind: UnreadableKind;
  /** EACCES/EPERM: permissions or macOS privacy settings. */
  permission: boolean;
  /** The OS's own words ("Operation not permitted (os error 1)"). */
  reason: string;
  /** One line for places that only show text. */
  message: string;
}

export const ARCHIVE_UNREADABLE = "archive_unreadable";

/** The coded error in a rejected `invoke`, or null for any other error. */
export function parseArchiveUnreadable(e: unknown): ArchiveUnreadable | null {
  const raw = typeof e === "string" ? e : e instanceof Error ? e.message : null;
  if (!raw || !raw.startsWith("{")) return null;
  try {
    const v = JSON.parse(raw) as Partial<ArchiveUnreadable>;
    if (v?.code !== ARCHIVE_UNREADABLE || typeof v.path !== "string") return null;
    return {
      code: ARCHIVE_UNREADABLE,
      path: v.path,
      kind: (["missing", "not_a_directory", "permission_denied", "other"] as const).includes(v.kind as UnreadableKind)
        ? (v.kind as UnreadableKind)
        : "other",
      permission: v.permission === true,
      reason: typeof v.reason === "string" ? v.reason : "",
      message: typeof v.message === "string" ? v.message : `The archive folder ${v.path} can't be read.`,
    };
  } catch {
    return null;
  }
}

/** Text for an error shown as a line (toasts, notes): the coded error's
 *  message, anything else as it came. */
export function errorText(e: unknown): string {
  return parseArchiveUnreadable(e)?.message ?? String(e);
}

export interface UnreadableCopy {
  title: string;
  /** What is wrong, in plain words. */
  explanation: string;
  /** What to do about it, when there is something specific. */
  hint: string | null;
  /** The OS reason, shown as it is. */
  osReason: string;
  /** Nothing was deleted. */
  reassurance: string;
}

const isMac = (platform: string) => /Mac/i.test(platform);

/** The Library's message for an unreadable archive folder. */
export function unreadableCopy(
  err: ArchiveUnreadable,
  platform: string = typeof navigator !== "undefined" ? navigator.userAgent : "",
): UnreadableCopy {
  let explanation: string;
  let hint: string | null = null;
  if (err.permission || err.kind === "permission_denied") {
    explanation = "Sussurro isn't allowed to read this folder.";
    hint = isMac(platform)
      ? "Allow Sussurro to access this folder in System Settings → Privacy & Security → Files and Folders (or Full Disk Access), then try again."
      : "Check that your user can open this folder, then try again.";
  } else if (err.kind === "missing") {
    explanation = "The folder isn't there: it may have been moved or renamed, or it's on a drive that isn't connected.";
    hint = "Reconnect the drive or put the folder back, then try again.";
  } else if (err.kind === "not_a_directory") {
    explanation = "Something that isn't a folder is at this path.";
  } else {
    explanation = "The system couldn't list this folder.";
  }
  return {
    title: "Can't open the archive folder",
    explanation,
    hint,
    osReason: err.reason,
    reassurance: "Nothing was deleted: your items show up again as soon as the folder can be read.",
  };
}
