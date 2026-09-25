/* Bulk import and export for the Personal dictionary (.txt) and Snippets
   (.csv). The file is picked by a native dialog opened from Rust
   (`pick_import_file`, #156; `save_list_export`, #99): no path crosses IPC,
   the backend only hands back the picked file's name and text. Parsing,
   merging and formatting stay here, in ../utils and ../lib/personalization. */

import { invoke } from "@tauri-apps/api/core";
import type { Ctl } from "../hooks/useAppController";
import { exportDictionaryTxt, exportSnippetsCsv } from "../lib/personalization";
import {
  describeDictionaryMerge,
  describeSnippetMerge,
  mergeDictionary,
  mergeSnippets,
  parseDictionaryFile,
  parseSnippetFile,
} from "../utils";

type ImportKind = "dictionary" | "snippets";

interface ImportFile {
  name: string;
  contents: string;
}

// Let the backend show the picker (.txt for the dictionary, .csv for
// snippets) and read the file, then MERGE into the current list — existing
// entries are never replaced. Empty/unparseable files change nothing.
// Resolves to null when the user cancels the dialog.
async function pickImportFile(kind: ImportKind): Promise<string | null> {
  const file = await invoke<ImportFile | null>("pick_import_file", { kind });
  return file ? file.contents : null;
}

export async function importDictionary(ctl: Ctl): Promise<void> {
  const { settings, save, setBusy } = ctl;
  try {
    const content = await pickImportFile("dictionary");
    if (content === null) return;
    const words = parseDictionaryFile(content);
    if (words.length === 0) {
      setBusy("Import failed: no words found — expected a .txt file with one word or phrase per line.");
      return;
    }
    const result = mergeDictionary(settings.dictionary, words);
    if (result.added > 0 && !(await save({ ...settings, dictionary: result.merged }))) return;
    ctl.flash(describeDictionaryMerge(result));
  } catch (e) {
    setBusy(String(e));
  }
}

export async function importSnippets(ctl: Ctl): Promise<void> {
  const { settings, save, setBusy } = ctl;
  try {
    const content = await pickImportFile("snippets");
    if (content === null) return;
    const imported = parseSnippetFile(content);
    if (imported.length === 0) {
      setBusy('Import failed: no snippets found — expected a .csv file with one "cue,text" per line.');
      return;
    }
    const result = mergeSnippets(settings.snippets, imported);
    if (result.added > 0 && !(await save({ ...settings, snippets: result.merged }))) return;
    ctl.flash(describeSnippetMerge(result));
  } catch (e) {
    setBusy(String(e));
  }
}

/* ---------- Export (#99) ----------
   The text is built here in the same format the import reads, then handed
   to `save_list_export`, which opens the save dialog from Rust and writes
   it. Resolves after the user saved or cancelled. */

async function saveExport(ctl: Ctl, kind: ImportKind, contents: string, what: string): Promise<void> {
  if (!contents) {
    ctl.flash(`Nothing to export: the ${what} list is empty.`);
    return;
  }
  try {
    const name = await invoke<string | null>("save_list_export", { kind, contents });
    if (name) ctl.flash(`Exported to ${name}.`);
  } catch (e) {
    ctl.setBusy(String(e));
  }
}

export function exportDictionary(ctl: Ctl): Promise<void> {
  return saveExport(ctl, "dictionary", exportDictionaryTxt(ctl.settings.dictionary), "dictionary");
}

export function exportSnippets(ctl: Ctl): Promise<void> {
  return saveExport(ctl, "snippets", exportSnippetsCsv(ctl.settings.snippets), "snippet");
}
