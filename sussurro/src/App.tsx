import { useAppController, type Ctl } from "./hooks/useAppController";
import { LegacyApp } from "./LegacyApp";
import { Shell } from "./shell/Shell";
import "./App.css";

/** The main window. `ui_v2` off (default): the classic single-column UI.
 *  On: the workspace preview (#114). Both render the same settings cards
 *  from one controller (`useAppController`). */
export default function App() {
  const ctl = useAppController();
  if (!ctl.settings) return <main className="loading">Loading…</main>;
  return ctl.settings.ui_v2 ? <Shell ctl={ctl as Ctl} /> : <LegacyApp ctl={ctl as Ctl} />;
}
