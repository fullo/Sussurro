# Sussurro landing site

The project's English landing page — a single, self-contained static file
([`index.html`](index.html)). No build step, no dependencies to install.

- **Design**: the app's Daruma system (warm paper, ink, daruma-red). The
  daruma's second eye paints red as you reach the download section — the same
  "recording moment" motif as the app.
- **Fonts**: Fraunces (display) + Inter (body) from Google Fonts, with system
  serif/sans fallbacks if offline.
- **Positioning**: local-first, private, open — the anti-cloud angle vs. Wispr
  Flow.

## Preview locally

Just open the file, or serve the folder:

```bash
cd site
python -m http.server 8080     # → http://localhost:8080
```

## Deploy

It's a static file — host it anywhere (GitHub Pages, Netlify, Cloudflare
Pages, any bucket). For GitHub Pages, point the Pages source at `/site` (or
copy `index.html` to the Pages branch). Requires the repo/Pages to be public
(planned for v0.5.0 — see [`../CLAUDE.md`](../CLAUDE.md)).

## Keep in sync

Feature copy mirrors the [root README](../README.md); update both when
features change. Download buttons link to the GitHub
[releases](https://github.com/fullo/Sussurro/releases) page.
