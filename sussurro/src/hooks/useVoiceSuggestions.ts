import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Item, Person, Settings, VoiceSuggestion } from "../lib/types";
import { shownSuggestions, suggestionsKey, suggestionsWanted, withDismissal } from "../lib/voiceSuggestions";

/** Voice suggestions for the open item (#242): asked again whenever its
 *  speakers, links, voice data or the People registry change — so after a
 *  run, after Re-detect and for an item opened from the archive later.
 *  `shown` maps a speaker id to the person it sounds like; `dismiss` is
 *  *Not X* (hidden at once, remembered by the backend for this document).
 *  A failed fetch shows nothing: suggestions are a convenience. */
export function useVoiceSuggestions(
  item: Item,
  people: Person[],
  settings: Pick<Settings, "voice_suggestions">,
): { shown: Map<string, Person>; dismiss: (speakerId: string, personId: string) => Promise<void> } {
  const wanted = suggestionsWanted(item, settings);
  const key = suggestionsKey(item, people);
  const id = item.id;
  const [list, setList] = useState<VoiceSuggestion[]>([]);
  const [dismissed, setDismissed] = useState<Set<string>>(() => new Set());

  // Another document: nothing of the previous one's carries over.
  useEffect(() => {
    setList([]);
    setDismissed(new Set());
  }, [id]);

  useEffect(() => {
    if (!wanted) {
      setList([]);
      return;
    }
    let alive = true;
    invoke<VoiceSuggestion[]>("voice_suggestions", { id })
      .then((s) => alive && setList(s))
      .catch(() => alive && setList([]));
    return () => {
      alive = false;
    };
    // `key` covers everything the answer depends on.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, wanted]);

  const speakers = item.segments.speakers;
  const shown = useMemo(
    () => (wanted ? shownSuggestions(speakers, list, people, dismissed) : new Map<string, Person>()),
    [wanted, speakers, list, people, dismissed],
  );

  const dismiss = useCallback(
    async (speakerId: string, personId: string) => {
      setDismissed((d) => withDismissal(d, speakerId, personId));
      // Not remembered on failure: it comes back on the next visit, which
      // is harmless (it stays hidden for now).
      await invoke("voice_suggestion_dismiss", { id, speakerId, personId }).catch(() => {});
    },
    [id],
  );

  return { shown, dismiss };
}
