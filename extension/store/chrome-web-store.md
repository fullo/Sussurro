# Chrome Web Store listing (#270)

For the Developer Dashboard's *Store listing* and *Privacy practices* tabs.
Upload `sussurro-extension-chrome-<version>.zip` (`npm run build:chrome`
in `extension/`). Category: **Productivity**.

Every claim below is checked against `extension/manifest.chrome.json` and
`docs/privacy.html#extension-permissions` as of this writing — re-verify
against the manifest before each submission, since a future change to
permissions or host access must be reflected here too (the unit tests in
`extension/scripts/manifest.test.ts` catch a manifest/README drift, not a
drift with this file).

## Single purpose description

Chrome requires one specific purpose, not a feature list.

> Captures the audio and participant names of a web meeting (Google Meet,
> Microsoft Teams or Zoom) and sends them only to the Sussurro desktop app
> running on the same computer, so it can transcribe the meeting locally.

## Detailed description

### English

```
Sussurro is a local, offline meeting transcriber. This extension is its
browser half: it captures a web meeting you are already in — Google Meet,
Microsoft Teams or the Zoom web client — and streams it to the Sussurro
app running on your own computer, which transcribes it live.

What it does
• Captures your microphone and the meeting's incoming audio (everyone
  else, mixed) once you press Start in its side panel — nothing before
  that.
• Reads the participants' names and who is currently speaking from the
  meeting page itself, so transcript lines are labelled with names
  instead of "Voice 1", "Voice 2"...
• Shows a live mirror of the transcript in the side panel while you meet:
  jump to the latest line, copy the transcript as text, or (when the app
  allows it) save an .srt subtitle file.
• Sends everything only to the Sussurro app at 127.0.0.1 on your own
  computer, using a pairing code you create in the app. It contacts no
  other server, has no analytics, telemetry or account, and does not work
  without the separate Sussurro desktop app installed and paired.

What it needs
Sussurro (the desktop app) is a separate, free download — this extension
does not transcribe or store anything by itself. Get it from
https://github.com/fullo/Sussurro, pair it once from the extension's
options page, and this extension becomes the way Sussurro sees your
browser meetings.

Everything Sussurro does — transcription, speaker labels, meeting
minutes — runs on your computer with locally downloaded models; sending
text to an external AI service is always a separate, explicit opt-in in
the app, never in this extension. Source code, privacy policy and support:
https://github.com/fullo/Sussurro
```

### Italian

```
Sussurro è un trascrittore di riunioni locale e offline. Questa estensione
è la sua metà nel browser: cattura una riunione web a cui stai già
partecipando — Google Meet, Microsoft Teams o il client web di Zoom — e la
trasmette all'app Sussurro in esecuzione sullo stesso computer, che la
trascrive in tempo reale.

Cosa fa
• Cattura il tuo microfono e l'audio in arrivo della riunione (tutti gli
  altri partecipanti, mixati) solo dopo aver premuto Start nel suo
  pannello laterale — niente prima di allora.
• Legge i nomi dei partecipanti e chi sta parlando direttamente dalla
  pagina della riunione, così le righe della trascrizione riportano i nomi
  invece di "Voce 1", "Voce 2"...
• Mostra uno specchio live della trascrizione nel pannello laterale
  durante la riunione: torna all'ultima riga, copia la trascrizione come
  testo o (quando l'app lo consente) salva un file di sottotitoli .srt.
• Invia tutto solo all'app Sussurro su 127.0.0.1, sullo stesso computer,
  usando un codice di abbinamento creato nell'app. Non contatta nessun
  altro server, non ha analitiche, telemetria o account, e non funziona
  senza l'app desktop Sussurro installata e abbinata separatamente.

Cosa serve
Sussurro (l'app desktop) è un download separato e gratuito — questa
estensione da sola non trascrive né salva nulla. Scaricala da
https://github.com/fullo/Sussurro, abbinala una volta dalla pagina delle
opzioni dell'estensione, e questa estensione diventa il modo in cui
Sussurro vede le tue riunioni nel browser.

Tutto ciò che Sussurro fa — trascrizione, etichette dei parlanti, verbali
di riunione — gira sul tuo computer con modelli scaricati localmente;
inviare testo a un servizio di AI esterno è sempre un'opzione separata ed
esplicita nell'app, mai in questa estensione. Codice sorgente, informativa
sulla privacy e supporto: https://github.com/fullo/Sussurro
```

## Permission justifications

Chrome's Privacy practices tab asks for one justification per requested
permission and per host permission. `extension/manifest.chrome.json`
requests: `storage`, `sidePanel`, `tabCapture`, `offscreen`, plus host
permissions for the three meeting sites and `http://127.0.0.1/*`.

| Permission | Justification |
|---|---|
| `storage` | Remembers, in the browser's own local storage, whether you dismissed the recording-consent notice and which meeting language you last picked per platform. No account data, no sync. |
| `sidePanel` | Shows the extension's side panel (Start/Stop, the live transcript, item actions) — its only user interface besides the options page. |
| `tabCapture` | A fallback only: if, after you press Start, no call audio has arrived from the page within 4 seconds (a meeting client that plays audio in a way the extension's normal capture can't reach), it captures that browser tab's own audio output so the meeting can still be transcribed. Not used otherwise, and not present at all in the Firefox build. |
| `offscreen` | Required by Chrome to run the `tabCapture` fallback above (an offscreen document is where Chrome allows a captured tab stream to be processed); it has no other use. |
| Host permission: `https://meet.google.com/*`, `https://teams.microsoft.com/*`, `https://teams.live.com/*`, `https://teams.cloud.microsoft/*`, `https://*.zoom.us/wc/*` | The extension's content scripts run only on these meeting pages, to capture the call's audio (via WebRTC) and to read the participants' names and who is speaking, directly from the page's own structure. No other site is requested — not `<all_urls>`, not a broad wildcard. |
| Host permission: `http://127.0.0.1/*` | To reach the Sussurro desktop app running on the user's own computer (the local API the app exposes). No other host is contacted. |

## Data usage disclosure

Chrome's form asks what user data the item collects and whether it is sold
or used for purposes unrelated to the item's core functionality.

- **Is data collected or transmitted off the user's device?** No. Audio
  and the participants' names are processed in the browser and sent only
  to `127.0.0.1` — the Sussurro app running on that same computer, under a
  pairing token the user creates. The extension has no server of its own,
  contacts no analytics, advertising or crash-reporting service, and
  nothing is sold or shared with any third party. (Re-check this framing
  against Chrome's current definition of "collected"/"transferred" before
  answering the form — some reviewers read a loopback connection to a
  separate local application differently from "the item keeps everything
  in-browser". The factual claim to keep either way: destination is always
  `127.0.0.1`, never a remote host.)
- **Personal communications**: the audio and participant names captured
  during a meeting are, functionally, personal communications content —
  disclose this category if the form requires stating what the extension
  *processes*, independent of whether it is judged to leave the device.
- **Website content**: the participants' names and speaking state are read
  from the meeting page's own DOM structure.
- All other categories (health, financial, authentication, location, web
  history, user activity beyond the above): not collected.
- **Certified use**: the extension's single purpose (see above) is the only
  use of any of this data; none of it is used for advertising, sold, or
  used to build a user profile.

## Remote code

**No remote code.** Every script is bundled at build time (Vite); nothing
is `eval`'d, fetched and executed, or loaded from a remote origin at
runtime. Chrome's form should be answered "No" / the remote-code checkbox
left unticked.

## Store listing assets

See [`screenshots.md`](screenshots.md) for what to capture (Chrome Web
Store: at least one 1280×800 or 640×400 screenshot, and an optional
440×280 small promo tile).
