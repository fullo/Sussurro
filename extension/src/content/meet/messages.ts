/* What the Meet observer emits (#131): the page messages of
 * shared/speakers.ts, with the performance time `at` in place of the page
 * frame `pf` (the MAIN world converts). */
import type { LitTile } from "./binder";
import type { Tile } from "./dom";
import type { HealthReport, SpeakerSource } from "../../shared/speakers";

export type ObserverMsg =
  | { type: "speaker_active"; id: string; name?: string; source: SpeakerSource; at: number }
  | { type: "speaker_idle"; id: string; source: SpeakerSource; at: number }
  | { type: "speaker_name"; id: string; name: string | null }
  | { type: "participants"; names: string[] }
  | ({ type: "observer_health" } & HealthReport);

/** The lit remote tiles that can vote: a guarded name, not the user. */
export function binderSafeLit(tiles: Tile[]): LitTile[] {
  return tiles.filter((t) => t.speaking && !t.self && t.name).map((t) => ({ name: t.name as string }));
}
