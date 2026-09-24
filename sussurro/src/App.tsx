import { useAppController, type Ctl } from "./hooks/useAppController";
import { Shell } from "./shell/Shell";
import "./App.css";

/** The main window: the workspace (left rail: New · Library · People ·
 *  Recipes · Models · Settings). Every screen renders from one controller
 *  (`useAppController`), the single source of truth for the settings. */
export default function App() {
  const ctl = useAppController();
  if (!ctl.settings) return <div className="app-loading">Loading…</div>;
  return <Shell ctl={ctl as Ctl} />;
}
