# Windows code signing via SignPath (OSS)

Goal: Authenticode-sign the Windows installers so SmartScreen stops warning
users. Sussurro uses **[SignPath Foundation](https://signpath.org/)**, which
signs open-source projects for free.

This is a maintainer setup task — it needs a human to enroll the project and
add secrets. Until it's done, Windows installers stay unsigned (SmartScreen:
*More info → Run anyway*).

## 1. Enroll the project (one-time, maintainer)

1. Apply to the SignPath Foundation OSS program:
   <https://about.signpath.io/product/open-source> → "Apply now".
   Provide the repo URL (`github.com/fullo/Sussurro`), license (AGPL-3.0),
   and maintainer identity. Approval is manual and can take a few days.
2. Once approved you get a SignPath organization. Note these four values —
   they become GitHub secrets / workflow inputs:
   - **Organization ID** (`SIGNPATH_ORGANIZATION_ID`)
   - **Project slug** (e.g. `Sussurro`)
   - **Signing policy slug** (e.g. `release-signing`)
   - **API token** for a CI user (`SIGNPATH_API_TOKEN`, secret)
3. In the SignPath project, create an **artifact configuration** that matches
   what we submit (a zip of the `.exe` + `.msi`).

## 2. The signing-order gotcha (important)

The Tauri **updater** signature (minisign, in `latest.json`) is computed over
the exact bytes of the installer. Authenticode signing **modifies** those
bytes. So the order must be:

1. Build the unsigned installers.
2. **Authenticode-sign** them via SignPath.
3. **Then** run `tauri signer sign` over the *signed* installer to produce the
   `.sig`, and assemble `latest.json` from those.

If you sign in the wrong order (updater-sign first, then Authenticode), every
Windows auto-update will fail signature verification. This is why the Windows
job can't use `tauri-action`'s one-shot build+sign+publish — it must be split.

## 3. Workflow shape (wired once secrets exist)

The Windows leg of `.github/workflows/release.yml` becomes:

```yaml
# 1. build unsigned (no updater signing yet)
- run: npm run tauri build -- --no-bundle=false   # produces .exe/.msi
# 2. submit to SignPath, get signed artifacts back
- uses: signpath/github-action-submit-signing-request@v1
  with:
    api-token: ${{ secrets.SIGNPATH_API_TOKEN }}
    organization-id: ${{ secrets.SIGNPATH_ORGANIZATION_ID }}
    project-slug: Sussurro
    signing-policy-slug: release-signing
    artifact-configuration-slug: installers
    github-artifact-id: ${{ steps.upload.outputs.artifact-id }}
    wait-for-completion: true
    output-artifact-directory: signed
# 3. updater-sign the SIGNED installers, build latest.json, publish
- run: tauri signer sign --private-key-path ... signed/*.exe
- uses: softprops/action-gh-release@v2
  with: { files: "signed/*", draft: true }
```

macOS and Linux keep using `tauri-action` unchanged. Only Windows splits out.

> Leave this until SignPath approval + the four values are in the repo secrets.
> Ping me (Claude) with the org-id/project/policy slugs and I'll wire the
> Windows job and test it.

## Alternatives (not chosen)

- **Azure Trusted Signing** — ~$10/mo, no upfront cert; clean Actions
  integration, identity-validated.
- **Bought OV/EV certificate** — ~$200–400/yr; EV clears SmartScreen instantly.
- **Do nothing** — users click through SmartScreen; the app still runs.

See [`releases.md`](releases.md) for the overall signing status.
