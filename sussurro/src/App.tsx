import { useAppController, type Ctl } from "./hooks/useAppController";
import { LegacyApp } from "./LegacyApp";
import "./App.css";

/** The main window: the classic single-column UI. The shared state lives in
 *  `useAppController`; the cards live in `settings/`. */
export default function App() {
  const ctl = useAppController();
  if (!ctl.settings) return <main className="loading">Loading…</main>;
  return <LegacyApp ctl={ctl as Ctl} />;
}
