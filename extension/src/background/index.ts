/* Background worker: a service worker on Chrome, an event page on Firefox.
   Scaffold only (#125): opens the panel from the toolbar button. The
   WebSocket to the app, reconnects and per-tab capture state arrive with
   #128; pairing (the extension token) with #127. */
import browser from "webextension-polyfill";

if (__BROWSER__ === "chrome") {
  // Chrome: the toolbar button opens the side panel.
  chrome?.sidePanel?.setPanelBehavior({ openPanelOnActionClick: true }).catch((e: unknown) => {
    console.error("Sussurro: cannot bind the side panel to the toolbar button", e);
  });
} else {
  // Firefox: toggle the sidebar (allowed here: we are inside a user action).
  browser.action.onClicked.addListener(() => {
    browser.sidebarAction.toggle().catch((e: unknown) => {
      console.error("Sussurro: cannot toggle the sidebar", e);
    });
  });
}
