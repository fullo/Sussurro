import { open as openDialog } from "@tauri-apps/plugin-dialog";

/** Audio files New → File accepts (picker filter and drag and drop). */
export const AUDIO_EXTENSIONS = ["wav", "mp3", "m4a", "aac", "flac", "ogg"];

/** Pick an audio file to transcribe; null when the dialog is dismissed. */
export async function pickAudioFile(): Promise<string | null> {
  const path = await openDialog({
    title: "Transcribe an audio file",
    multiple: false,
    directory: false,
    filters: [{ name: "Audio", extensions: AUDIO_EXTENSIONS }],
  });
  return path && typeof path === "string" ? path : null;
}
