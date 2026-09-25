# Notes for AMO reviewers

Sussurro captures the audio of a web meeting (Google Meet, Microsoft Teams,
Zoom web) and streams it to the Sussurro desktop app on the same computer
(ws://127.0.0.1, after a pairing code). Nothing is sent anywhere else. The
add-on is self-distributed (unlisted channel, own update_url).

Build from the attached sources (Node.js >= 24): unzip, cd extension,
npm ci, npm run build:firefox. dist/firefox/ is the signed directory; the
build is deterministic. extension/README.md has the details.

The validator reports three warnings, all known:

1. UNSAFE_VAR_ASSIGNMENT x2, assets/page-*.js (line 9). Both are inside
   React DOM 19 (node_modules/react-dom/cjs/react-dom-client.production.js,
   functions setProp and setPropOnCustomElement): the write that implements
   React's dangerouslySetInnerHTML prop, minified as
   `n?.__html!==u&&(l.innerHTML=u)`. No Sussurro code uses
   dangerouslySetInnerHTML or assigns innerHTML/outerHTML/insertAdjacentHTML
   (extension/src and the shared sussurro/src/transcript components): the
   pages render text through React only, so the path is never taken. It is
   not tree-shakeable (it is a case of the prop setter every element uses),
   and we do not patch React. Our CI lint gate (extension/scripts/
   lint-policy.ts) accepts only these two sites, matched by that exact
   pattern, and fails the build on any other innerHTML warning.

2. KEY_FIREFOX_ANDROID_UNSUPPORTED_BY_MIN_VERSION (data_collection_permissions
   needs Firefox for Android 142). The add-on does not support Android: it
   needs the desktop sidebar and a Sussurro desktop app on the same device.
   The manifest deliberately has no gecko_android, and each submission sets
   the version's compatibility to Firefox (desktop) only. Without
   gecko_android the linter checks Android against gecko.strict_min_version
   (140), hence the warning.

Desktop: strict_min_version is 140.0, the first Firefox that reads
data_collection_permissions (ESR 128 is end-of-life; ESR 140 is the oldest
supported release).

Permissions: storage (pairing token, settings); host permissions for the
three meeting sites (content scripts that read the meeting's audio and
participant names) and http://127.0.0.1/* (the local app).
data_collection_permissions: none.
