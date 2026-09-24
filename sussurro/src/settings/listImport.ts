/* Bulk import for the Personal dictionary (.txt) and Snippets (.csv).
   Kept in one small module on purpose: #156 replaces the path-based read
   below with a Rust-side picker, and that change should stay local. */

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Ctl } from "../hooks/useAppController";
import {
  describeDictionaryMerge,
  describeSnippetMerge,
  mergeDictionary,
  mergeSnippets,
  parseDictionaryFile,
  parseSnippetFile,
} from "../utils";

// Pick a file, read its text via the narrow `read_import_file` command
// (.txt/.csv only), then MERGE into the current list — existing entries are
// never replaced. Empty/unparseable files change nothing.
async function readImportFile(title: string, name: string, ext: string): Promise<string | null> {
  const path = await openDialog({
    title,
    multiple: false,
    directory: false,
    filters: [{ name, extensions: [ext] }],
  });
  if (!path || typeof path !== "string") return null;
  return invoke<string>("read_import_file", { path });
}

export async function importDictionary(ctl: Ctl): Promise<void> {
  const { settings, save, setBusy } = ctl;
  try {
    const content = await readImportFile("Import dictionary", "Text files", "txt");
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
    const content = await readImportFile("Import snippets", "CSV files", "csv");
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
