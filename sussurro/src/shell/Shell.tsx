import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AboutDialog } from "../components/AboutDialog";
import type { Ctl } from "../hooks/useAppController";
import { useEngineRuns } from "../hooks/useEngineRuns";
import type { RunKind } from "../lib/engineRuns";
import { withDefaults } from "../lib/library";
import type { Item } from "../lib/types";
import { LibraryScreen } from "./LibraryScreen";
import { ModelsScreen } from "./ModelsScreen";
import { PeopleScreen } from "./PeopleScreen";
import { RecipesScreen } from "./RecipesScreen";
import { NewScreen, type NewDefaults } from "./NewScreen";
import { Rail, type Screen } from "./Rail";
import { SettingsScreen, type SectionId } from "./SettingsScreen";
import "./shell.css";

const SCREEN_KEY = "shellScreen";

function loadScreen(): Screen {
  try {
    const s = localStorage.getItem(SCREEN_KEY);
    if (s === "new" || s === "library" || s === "people" || s === "recipes" || s === "models" || s === "settings") return s;
  } catch {
    /* storage unavailable: fall through */
  }
  return "library";
}

/** Workspace preview (proposal A, #114): left rail with New · Library ·
 *  People · Recipes · Models · Settings. Dictation stays tray-first and is
 *  configured under Settings → Dictation. Recipes hosts the LLM profiles
 *  (#119) next to the recipes (#120); People is the registry that gives
 *  participants their email (#132). */
export function Shell({ ctl }: { ctl: Ctl }) {
  const engine = useEngineRuns();
  const [screen, setScreenState] = useState<Screen>(loadScreen);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [libraryVersion, setLibraryVersion] = useState(0);
  const [libraryCount, setLibraryCount] = useState<number | null>(null);
  const [section, setSection] = useState<SectionId>("dictation");
  const [aboutOpen, setAboutOpen] = useState(false);
  const [defaults, setDefaults] = useState<NewDefaults>({ tags: [], categories: [] });
  /** Defaults captured when each run started, applied when it finishes. */
  const runDefaults = useRef<Record<RunKind, NewDefaults>>({
    mic: { tags: [], categories: [] },
    file: { tags: [], categories: [] },
    link: { tags: [], categories: [] },
    system: { tags: [], categories: [] },
  });
  const applied = useRef(new Set<string>());

  const setScreen = useCallback((s: Screen) => {
    setScreenState(s);
    try {
      localStorage.setItem(SCREEN_KEY, s);
    } catch {
      /* ignore */
    }
  }, []);

  // The workspace fills the window; the classic page scrolls the body.
  useEffect(() => {
    document.documentElement.classList.add("ui-v2");
    return () => document.documentElement.classList.remove("ui-v2");
  }, []);

  const refreshLibrary = useCallback(() => setLibraryVersion((v) => v + 1), []);

  // When a run writes its item: add New's default tags / category, then
  // refresh the Library. Keyed by item id so it happens once per item.
  const { mic, file, link } = engine.runs;
  useEffect(() => {
    for (const run of [mic, file, link]) {
      const result = run?.result;
      if (!run || !result || applied.current.has(result.item_id)) continue;
      applied.current.add(result.item_id);
      const d = runDefaults.current[run.kind];
      (async () => {
        if (d.tags.length || d.categories.length) {
          try {
            const item = await invoke<Item>("archive_get", { id: result.item_id });
            const meta = withDefaults(item.meta, d.tags, d.categories);
            if (meta) await invoke("archive_update_meta", { id: result.item_id, meta });
          } catch (e) {
            ctl.setBusy(`Saved, but the default tags could not be added: ${e}`);
          }
        }
        refreshLibrary();
      })();
    }
  }, [mic, file, link, ctl, refreshLibrary]);

  // A run's item appears in the Library as soon as it starts (#153, marked
  // recording) and may be renamed at the end (untitled → titled folder):
  // refresh on every item-id change and let a selection follow the rename.
  const micItem = mic?.itemId ?? null;
  const fileItem = file?.itemId ?? null;
  const linkItem = link?.itemId ?? null;
  useEffect(() => {
    if (micItem || fileItem || linkItem) refreshLibrary();
  }, [micItem, fileItem, linkItem, refreshLibrary]);
  useEffect(() => {
    for (const run of [mic, file, link]) {
      if (run?.previousItemId && run.itemId) {
        const { previousItemId, itemId } = run;
        setSelectedId((sel) => (sel === previousItemId ? itemId : sel));
      }
    }
  }, [mic, file, link]);
  // A run that ended in an error still changes the Library (kept or discarded item).
  const micStatus = mic?.status;
  const fileStatus = file?.status;
  const linkStatus = link?.status;
  useEffect(() => {
    if (micStatus === "error" || fileStatus === "error" || linkStatus === "error") refreshLibrary();
  }, [micStatus, fileStatus, linkStatus, refreshLibrary]);

  const openItem = useCallback(
    (id: string) => {
      setSelectedId(id);
      setScreen("library");
    },
    [setScreen],
  );

  // "Open in Sussurro" from the browser extension (#126,
  // `POST /items/{id}/open`): the app comes to the front on that item.
  useEffect(() => {
    const unlisten = listen<string>("open-item", (e) => {
      refreshLibrary();
      openItem(e.payload);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [openItem, refreshLibrary]);

  const startRun = (kind: RunKind) => {
    runDefaults.current[kind] = { tags: defaults.tags.slice(), categories: defaults.categories.slice() };
  };

  return (
    <div className="shell">
      <Rail
        ctl={ctl}
        engine={engine}
        screen={screen}
        onNavigate={setScreen}
        libraryCount={libraryCount}
      />
      <main className="sh-main">
        {ctl.busy && <p className="busy" role="alert">{ctl.busy}</p>}
        {screen === "new" && (
          <NewScreen
            ctl={ctl}
            engine={engine}
            defaults={defaults}
            onDefaultsChange={setDefaults}
            onRunStart={startRun}
            onOpenItem={openItem}
          />
        )}
        {screen === "library" && (
          <LibraryScreen
            ctl={ctl}
            selectedId={selectedId}
            onSelect={setSelectedId}
            version={libraryVersion}
            onChanged={refreshLibrary}
            onCount={setLibraryCount}
            onNew={() => setScreen("new")}
          />
        )}
        {screen === "people" && <PeopleScreen ctl={ctl} />}
        {screen === "recipes" && <RecipesScreen ctl={ctl} />}
        {screen === "models" && (
          <ModelsScreen
            ctl={ctl}
            onOpenSettings={(s) => { setSection(s); setScreen("settings"); }}
            onOpenRecipes={() => setScreen("recipes")}
          />
        )}
        {screen === "settings" && (
          <SettingsScreen
            ctl={ctl}
            section={section}
            onSection={setSection}
            onOpenModels={() => setScreen("models")}
            onOpenRecipes={() => setScreen("recipes")}
            onAbout={() => setAboutOpen(true)}
          />
        )}
      </main>
      {aboutOpen && <AboutDialog version={ctl.version} onClose={() => setAboutOpen(false)} />}
    </div>
  );
}
