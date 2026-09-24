import { useEffect, useState } from "react";
import { isNoticeNeeded, onNoticeChanged } from "./notice";

/** Whether the next Start shows the recording notice (#136), kept up to
 *  date when the options page or another panel changes it. `undefined`
 *  while loading. */
export function useNoticeNeeded(): boolean | undefined {
  const [needed, setNeeded] = useState<boolean | undefined>(undefined);
  useEffect(() => {
    let live = true;
    void isNoticeNeeded().then((n) => live && setNeeded(n));
    const off = onNoticeChanged((n) => live && setNeeded(n));
    return () => {
      live = false;
      off();
    };
  }, []);
  return needed;
}
