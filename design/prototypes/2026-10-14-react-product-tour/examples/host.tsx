import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { PendingCrewTour } from "@pendingname/pendingcrew-product-tour";
import "@pendingname/pendingcrew-product-tour/styles.css";
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <section className="host-section" id="first-tour">
      <PendingCrewTour
        mode="embedded"
        height={700}
        storageKey="integration-first"
        persist={false}
        migrateLegacy={false}
      />
    </section>
    <section className="host-section" id="second-tour">
      <PendingCrewTour
        mode="embedded"
        height={700}
        storageKey="integration-second"
        persist={false}
        migrateLegacy={false}
      />
    </section>
  </StrictMode>,
);
