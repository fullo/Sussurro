import { fmtCount } from "../lib/format";
import { PAGE_SIZE, type ListSort } from "../lib/personalization";

/** Sort order picker shared by the dictionary and snippet managers (#99). */
export function SortSelect({ value, onChange, label }: { value: ListSort; onChange: (s: ListSort) => void; label: string }) {
  return (
    <select className="lm-sort" aria-label={label} value={value} onChange={(e) => onChange(e.target.value as ListSort)}>
      <option value="added">Order added</option>
      <option value="az">A → Z</option>
      <option value="za">Z → A</option>
    </select>
  );
}

/** Previous / next pages for a long list; nothing when it fits on one page. */
export function Pager({
  page,
  pages,
  total,
  onPage,
  label,
}: {
  page: number;
  pages: number;
  total: number;
  onPage: (p: number) => void;
  label: string;
}) {
  if (pages <= 1) return null;
  const from = page * PAGE_SIZE + 1;
  const to = Math.min(total, (page + 1) * PAGE_SIZE);
  return (
    <nav className="lm-pager" aria-label={label}>
      <button type="button" className="btn-ghost" disabled={page === 0} onClick={() => onPage(page - 1)}>
        ‹ Previous
      </button>
      <span aria-live="polite">
        {fmtCount(from)}–{fmtCount(to)} of {fmtCount(total)} · page {page + 1} of {pages}
      </span>
      <button type="button" className="btn-ghost" disabled={page >= pages - 1} onClick={() => onPage(page + 1)}>
        Next ›
      </button>
    </nav>
  );
}
