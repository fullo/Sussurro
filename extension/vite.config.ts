/// <reference types="vitest/config" />
import { defineConfig, type UserConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

export type Target = "chrome" | "firefox";

const here = (p: string) => fileURLToPath(new URL(p, import.meta.url));

/** Settings shared by every bundle of the extension (pages and scripts).
 *  `scripts/build.ts` merges the per-bundle input/output on top. */
export function baseConfig(target: Target): UserConfig {
  return {
    plugins: [react()],
    resolve: {
      alias: {
        // Transcript components shared with the app (plan E11): the same
        // alias the app declares in sussurro/vite.config.ts.
        "@sussurro/transcript": here("../sussurro/src/transcript/index.ts"),
      },
      // The shared components live under sussurro/: without this, their
      // `react` import would resolve to sussurro/node_modules (a second React,
      // or nothing at all when only the extension is installed, as in CI).
      dedupe: ["react", "react-dom"],
    },
    define: {
      __BROWSER__: JSON.stringify(target),
    },
    build: {
      // MAIN-world content scripts need Chrome >= 111 / Firefox >= 128;
      // the side panel API needs Chrome >= 116 (see the manifests).
      target: ["chrome116", "firefox128"],
    },
  };
}

// Used by `vitest` (and by `vite build` if ever run by hand: prefer
// `npm run build:chrome` / `build:firefox`, which also write the manifest).
export default defineConfig({
  ...baseConfig("chrome"),
  test: {
    include: ["src/**/*.test.ts", "scripts/**/*.test.ts"],
  },
});
