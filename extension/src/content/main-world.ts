/* MAIN-world content script: runs in the meeting page's own JavaScript
   context at document_start, so it can wrap `RTCPeerConnection` before the
   page uses it (plan E4). No extension APIs here — it talks to the
   isolated-world script through `window.postMessage`.

   Scaffold only (#125): deliberately does nothing yet. The capture hook
   lands with #128. Keep it free of globals: this file shares the page's
   `window`. */

export {};
