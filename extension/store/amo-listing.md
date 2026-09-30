# addons.mozilla.org listing, listed channel (#269)

The AMO account and signing keys already exist — the Firefox build is
signed on every release, today on the **unlisted** channel (self-hosted,
own `update_url`, #228). Switching to a **listed** version adds Mozilla
human review and store visibility; it needs a separate build (no
`update_url`, which AMO refuses on listed add-ons) and its own submission
text, both here.

## Build to submit

`sussurro-extension-firefox-listed-<version>.zip`
(`npm run build:firefox:listed` in `extension/`) — **not** the self-hosted
`sussurro-extension-firefox-<version>.zip` that existing self-hosted
installs use. Attach `sussurro-extension-source-<version>.zip` (the source
archive, built by the release workflow or by hand — see
[`README.md`](README.md)) and `../AMO-REVIEWER-NOTES.md` as the version's
approval notes, exactly as the existing unlisted signing step already does
(`extension/scripts/amo-metadata.ts`).

## Category and tags

- **Category**: Productivity (AMO's category list; re-check it hasn't been
  renamed at submission time).
- **Tags**: `meeting`, `transcription`, `google-meet`, `microsoft-teams`,
  `zoom`, `privacy`, `local`, `offline` — free-form, pick what still applies
  when AMO's tag list is checked at submission time.

## Summary (AMO's short, single-line field)

### English

```
Captures Google Meet, Microsoft Teams and Zoom meetings for the local, offline Sussurro transcriber. Nothing leaves your computer.
```

### Italian

```
Cattura riunioni Google Meet, Microsoft Teams e Zoom per il trascrittore locale e offline Sussurro. Niente lascia il tuo computer.
```

(Both comfortably under AMO's summary length limit; re-check the exact
character cap in the submission form, since AMO has changed it before.)

## Description

### English

```
Sussurro is a local, offline meeting transcriber for Windows, macOS (Apple Silicon)
and Linux. This add-on is its Firefox half (Firefox desktop 140 or later): it captures a web meeting you are
already in — Google Meet, Microsoft Teams or the Zoom web client — and
streams it to the Sussurro desktop app running on your own computer, which
transcribes it live. It requires that separate, free desktop app
(https://github.com/fullo/Sussurro) to work; the add-on alone does not
transcribe or store anything.

What it does:
– Captures nothing until you press Start in its sidebar — before that, its
  script on the meeting page only watches for the call's audio so it can
  record it once you start.
– Captures your microphone (labelled "You") and the meeting's incoming
  audio (everyone else, mixed).
– Reads the participants' names and who is currently speaking directly
  from the meeting page's own structure — never chat, never recordings, no
  clicking or opening panels — so transcript lines can be labelled with
  real names instead of "Voice 1", "Voice 2"...
– Shows a live mirror of the transcript in the sidebar while you meet.
– Sends everything only to the Sussurro app at 127.0.0.1 on your own
  computer, over a pairing token you create; it contacts no other server
  and collects no data of its own (see the manifest's
  data_collection_permissions and this add-on's privacy policy).

Recording a meeting records other people too: the sidebar shows a short
notice before your first recording, since depending on where you are and
your organisation's rules you may need to tell participants, or they may
need to agree, before you record them.

Privacy policy: https://fullo.github.io/Sussurro/privacy.html
Source code (AGPL-3.0-or-later): https://github.com/fullo/Sussurro
```

### Italian

```
Sussurro è un trascrittore di riunioni locale e offline per Windows, macOS
(Apple Silicon) e Linux. Questo componente aggiuntivo è la sua metà per Firefox (Firefox desktop 140 o successivo): cattura
una riunione web a cui stai già partecipando — Google Meet, Microsoft Teams
o il client web di Zoom — e la trasmette all'app desktop Sussurro in
esecuzione sullo stesso computer, che la trascrive in tempo reale.
Richiede quell'app desktop separata e gratuita
(https://github.com/fullo/Sussurro) per funzionare; da solo il componente
aggiuntivo non trascrive né salva nulla.

Cosa fa:
– Non cattura nulla finché non premi Start nella barra laterale — prima di
  allora, lo script sulla pagina della riunione osserva soltanto l'audio
  della chiamata, per poterlo registrare una volta avviato.
– Cattura il tuo microfono (etichettato "You") e l'audio in arrivo della
  riunione (tutti gli altri partecipanti, mixati).
– Legge i nomi dei partecipanti e chi sta parlando direttamente dalla
  struttura della pagina della riunione — mai la chat, mai registrazioni,
  senza aprire pannelli o fare clic — così le righe della trascrizione
  possono riportare nomi reali invece di "Voce 1", "Voce 2"...
– Mostra uno specchio live della trascrizione nella barra laterale durante
  la riunione.
– Invia tutto solo all'app Sussurro su 127.0.0.1, sullo stesso computer,
  tramite un token di abbinamento creato da te; non contatta nessun altro
  server e non raccoglie dati propri (vedi data_collection_permissions nel
  manifest e l'informativa sulla privacy di questo componente aggiuntivo).

Registrare una riunione registra anche altre persone: la barra laterale
mostra un breve avviso prima della tua prima registrazione, perché a
seconda di dove ti trovi e delle regole della tua organizzazione potresti
dover informare i partecipanti, o questi potrebbero dover acconsentire,
prima di registrarli.

Informativa sulla privacy: https://fullo.github.io/Sussurro/privacy.html
Codice sorgente (AGPL-3.0-or-later): https://github.com/fullo/Sussurro
```

## What AMO shows automatically (no extra text needed)

- **Permissions**: AMO lists `storage` and the host permissions straight
  from the manifest on the listing page; it does not have Chrome's
  per-permission justification form. Reviewers still read
  `AMO-REVIEWER-NOTES.md`'s permissions paragraph (submitted as approval
  notes) for the reasoning.
- **Data collection**: `browser_specific_settings.gecko.
  data_collection_permissions.required: ["none"]` in
  `manifest.firefox.json` makes AMO show the add-on as collecting no data;
  nothing extra to fill in for that.
- **Privacy policy URL**: `https://fullo.github.io/Sussurro/privacy.html`
  goes in the listing's dedicated field.

## After listing (do not do until #269 actually submits)

Per `extension/README.md` → *Releases, signing and updates*: once the
add-on is listed, drop `update_url` from every future submission (the
`firefox-listed` build already omits it) and plan what happens to existing
self-hosted installs using `docs/extension/updates.json` — Firefox does
not migrate them automatically from a self-hosted `update_url` to AMO's own
listing.
