/* A live transcript part → the shared transcript components' lines (#129),
 * with the app's speaker chips. Panel only: `@sussurro/transcript` brings
 * React and CSS, which the background must not load. Pure. */
import { toLines, type TranscriptLineData, type TranscriptSpeaker } from "@sussurro/transcript";
import type { LivePart } from "../shared/live";
import { chipFor, lineSpeakerId } from "../shared/speakers";

export function partLines(p: LivePart): TranscriptLineData[] {
  const speakers = new Map<string, TranscriptSpeaker>();
  const segments = p.lines.map((l) => {
    const sid = lineSpeakerId(l);
    if (sid && !speakers.has(sid)) speakers.set(sid, chipFor(sid, p.speakers[sid]));
    return {
      id: l.id,
      start_ms: l.start_ms,
      text: l.text,
      ...(l.failed ? { stt_error: "the engine could not transcribe this stretch" } : {}),
      ...(sid ? { speaker_id: sid } : {}),
    };
  });
  return toLines(segments, [...speakers.values()]);
}
