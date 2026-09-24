import { useEffect, useState } from "react";
import { getPairing, onPairingChanged, type Pairing } from "./pairing";

/** The saved pairing, kept up to date when the options page changes it.
 *  `undefined` while loading, `null` when not paired. */
export function usePairing(): Pairing | null | undefined {
  const [pairing, setPairing] = useState<Pairing | null | undefined>(undefined);
  useEffect(() => {
    let live = true;
    getPairing().then(
      (p) => live && setPairing(p),
      () => live && setPairing(null),
    );
    const off = onPairingChanged((p) => live && setPairing(p));
    return () => {
      live = false;
      off();
    };
  }, []);
  return pairing;
}
