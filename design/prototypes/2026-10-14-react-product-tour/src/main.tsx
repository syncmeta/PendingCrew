import { createRoot } from "react-dom/client";
import { PendingCrewTour } from "./PendingCrewTour";
const embedded = new URLSearchParams(location.search).get("embed") === "1";
createRoot(document.getElementById("root")!).render(
  <PendingCrewTour
    mode={embedded ? "embedded" : "standalone"}
    height={embedded ? "100dvh" : undefined}
  />,
);
