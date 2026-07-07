# Sussurro landing site

The project's English marketing site: a static landing page
([`index.html`](index.html)) plus a [`blog/`](blog/) of 29 articles. No build
step, no dependencies to install.

> This site lives in `docs/` because GitHub Pages' "Deploy from a branch" mode
> only serves the repo root or `/docs`. The developer docs (`development.md`,
> `releases.md`, `compile/`, …) live alongside it in the same folder; a
> `.nojekyll` marker keeps Pages from running them through Jekyll.

- **Design**: the app's Daruma system (warm paper, ink, daruma-red). The
  daruma's second eye paints red as you reach the download section — the same
  "recording moment" motif as the app. Blog articles share
  [`assets/blog.css`](assets/blog.css).
- **Fonts**: Fraunces (display) + Inter (body) from Google Fonts, with system
  serif/sans fallbacks if offline.
- **Positioning**: local-first, private, open — the anti-cloud angle vs. Wispr
  Flow.

## SEO

Every page carries a canonical URL, Open Graph + Twitter Card tags, and
JSON-LD structured data (`SoftwareApplication` on the landing, `TechArticle`
on each post, `Blog` on the index). Social preview image:
[`assets/og.png`](assets/og.png) (1200×630); favicon
[`assets/favicon.svg`](assets/favicon.svg). Site-wide
[`sitemap.xml`](sitemap.xml) and [`robots.txt`](robots.txt) live at the site
root.

The canonical base URL is **`https://fullo.github.io/Sussurro`** — hardcoded
in the meta/sitemap. If the site moves to another domain, re-run the SEO pass
to rewrite those absolute URLs. Because this is a GitHub **project** page (a
subpath), crawlers read `robots.txt` from the host root
(`fullo.github.io/robots.txt`, which we don't control), not from our subpath —
so submit `sitemap.xml` directly in Google Search Console rather than relying
on robots.txt discovery. (A custom domain / apex would remove this caveat.)

## Preview locally

Just open the file, or serve the folder:

```bash
cd docs
python -m http.server 8080     # → http://localhost:8080
```

## Deploy

It's static — host it anywhere (GitHub Pages, Netlify, Cloudflare Pages, any
bucket). For GitHub Pages: **Settings → Pages → Deploy from a branch → `main`
/ `/docs`**. That serves this folder at `https://fullo.github.io/Sussurro/`,
matching the canonical URLs in the SEO tags. Requires the repo/Pages to be
public (planned for v0.5.0 — see [`../CLAUDE.md`](../CLAUDE.md)).

## Keep in sync

Feature copy mirrors the [root README](../README.md); update both when
features change. Download buttons link to the GitHub
[releases](https://github.com/fullo/Sussurro/releases) page.
