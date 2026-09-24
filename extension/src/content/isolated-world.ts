/* ISOLATED-world content script: the extension side of a meeting page. It
   will relay audio frames and speaker events from the MAIN-world script to
   the background worker (#128).

   Scaffold only (#125): identifies the platform and does nothing else. */
import { detectPlatform } from "../shared/platform";

const platform = detectPlatform(location.href);
if (platform && import.meta.env.DEV) {
  console.debug(`Sussurro: content script ready on ${platform}`);
}
