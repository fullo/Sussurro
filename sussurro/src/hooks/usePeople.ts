import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Person } from "../lib/types";

/** The People registry (#132) for a screen that links participants to it.
 *  An unreadable registry reads as empty here (linking is best-effort); the
 *  People screen itself reports the error. */
export function usePeople(): { people: Person[]; reload: () => void } {
  const [people, setPeople] = useState<Person[]>([]);
  const reload = useCallback(() => {
    invoke<Person[]>("people_list")
      .then(setPeople)
      .catch(() => setPeople([]));
  }, []);
  useEffect(reload, [reload]);
  return { people, reload };
}
