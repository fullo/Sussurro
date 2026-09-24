import browser from "webextension-polyfill";

/** Logo, title and the extension version (= the app version it ships with). */
export function Header({ title }: { title: string }) {
  const { version, version_name } = browser.runtime.getManifest() as { version: string; version_name?: string };
  return (
    <header className="page-header">
      <img src="icons/64.png" alt="" />
      <h1>{title}</h1>
      <span className="page-version">v{version_name ?? version}</span>
    </header>
  );
}
