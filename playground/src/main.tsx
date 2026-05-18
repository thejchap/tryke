import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Chrome } from "./Editor";
import "./styles/global.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Chrome />
  </StrictMode>
);
