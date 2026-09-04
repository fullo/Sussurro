/**
 * Parses a dictionary file (.txt) where each line is a dictionary entry.
 * Removes leading/trailing whitespace and ignores empty lines.
 */
export function parseDictionaryFile(content: string): string[] {
  return content
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/**
 * Parses a snippet file (.csv) where each line is "cue,text".
 * Handles simple CSV format. For complex text with commas, users
 * are encouraged to wrap the 'text' part in double quotes.
 */
export function parseSnippetFile(content: string): { cue: string; text: string }[] {
  const lines = content.split(/\r?\n/);
  const snippets: { cue: string; text: string }[] = [];

  for (const line of lines) {
    if (!line.trim()) continue;

    // Find the first comma as the delimiter
    const commaIndex = line.indexOf(",");
    if (commaIndex !== -1) {
      const cue = line.slice(0, commaIndex).trim();
      let text = line.slice(commaIndex + 1).trim();

      // Simple quote handling: if the text part starts and ends with quotes, strip them.
      if (text.startsWith('"') && text.endsWith('"')) {
        text = text.slice(1, -1);
      }

      if (cue && text) {
        snippets.push({ cue, text });
      }
    }
  }

  return snippets;
}
