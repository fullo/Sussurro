/* Options page. Scaffold only (#125): pairing with the app (extension
   token, "Test connection") arrives with #127. */
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Header } from "../shared/Header";
import "../shared/page.css";

function Options() {
  return (
    <main className="page">
      <Header title="Sussurro options" />
      <p className="page-note">Pairing with the Sussurro app will be set up here.</p>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Options />
  </StrictMode>,
);
