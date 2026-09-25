/* Public entry of the transcript components, imported as
   `@sussurro/transcript` (Vite alias + tsconfig path) by the app and, from
   0.9, by the browser-extension side panel (E11). */
export { NEW_VOICE_TARGET, TranscriptLine, TranscriptView, alsoSpeakingText, lineTimestamp, toLines } from "./TranscriptView";
export type { OverlapSpanData, TranscriptLineData, TranscriptSpeaker } from "./TranscriptView";
