/* Public entry of the transcript components, imported as
   `@sussurro/transcript` (Vite alias + tsconfig path) by the app and, from
   0.9, by the browser-extension side panel (E11). */
export { TranscriptLine, TranscriptView, lineTimestamp, toLines } from "./TranscriptView";
export type { TranscriptLineData } from "./TranscriptView";
