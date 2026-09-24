/* Third-party licences of the extension (#138), for the options page's
   About section. `licenses.json` is generated from the extension's npm
   production dependencies by `npm run licenses`
   (sussurro/scripts/gen-licenses.mjs --extension) and committed; a unit
   test checks it matches package-lock.json. Pure. */
import generated from "./licenses.json";

/** The generated file: packages point into a list of shared texts. */
export interface LicenseFile {
  packages: { name: string; version: string; license: string; repository: string; textId: number }[];
  texts: string[];
}

export interface LicenseEntry {
  name: string;
  version: string;
  license: string;
  repository: string;
  /** The full licence text ("" when the package ships none). */
  text: string;
}

export const SOURCE_URL = "https://github.com/fullo/Sussurro";

/** The shipped dependencies with their licence texts, by name. */
export function licenseEntries(file: LicenseFile = generated): LicenseEntry[] {
  return file.packages
    .map((p) => ({
      name: p.name,
      version: p.version,
      license: p.license,
      repository: repositoryUrl(p.repository),
      text: p.textId >= 0 ? (file.texts[p.textId] ?? "") : "",
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** npm's `git+https://….git` → a browsable https URL; anything that isn't
 *  http(s) → "" (never a link to another scheme). */
export function repositoryUrl(raw: string): string {
  const url = raw.trim().replace(/^git\+/, "").replace(/\.git$/, "");
  return /^https?:\/\//.test(url) ? url : "";
}
