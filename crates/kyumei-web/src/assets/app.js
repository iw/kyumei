// Progressive enhancement only: every action works without this script.
(() => {
  "use strict";

  // Copy buttons on collection instructions.
  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-copy]");
    if (!button) return;
    const source = document.getElementById(button.dataset.copy);
    if (!source || !navigator.clipboard) return;
    navigator.clipboard.writeText(source.textContent.trim()).then(() => {
      const label = button.textContent;
      button.textContent = "Copied";
      setTimeout(() => { button.textContent = label; }, 1400);
    });
  });

  // Choices that apply at once, such as the time zone.
  document.addEventListener("change", (event) => {
    const select = event.target.closest("select[data-autosubmit]");
    if (select && select.form) select.form.submit();
  });

  // Track unsaved input so a live update never discards what the operator typed.
  let dirty = false;
  document.addEventListener("input", (event) => {
    if (event.target.closest("form") && !event.target.closest("[data-autosubmit]")) dirty = true;
  });
  document.addEventListener("submit", () => { dirty = false; });

  // Live updates: another tab, or the explanation agent, changed the investigation.
  const workspace = document.querySelector("[data-investigation]");
  if (workspace && "EventSource" in window) {
    const id = encodeURIComponent(workspace.dataset.investigation);
    const source = new EventSource(`/investigations/${id}/events`);
    source.addEventListener("changed", (event) => {
      if (event.data === workspace.dataset.version) return;
      if (dirty) {
        const banner = document.querySelector(".live-banner");
        if (banner) banner.hidden = false;
      } else {
        // Keep the notice about evidence just received: the change is often
        // the explanation of that same evidence.
        const url = new URL(window.location.href);
        url.searchParams.delete("error");
        window.location.replace(url.toString());
      }
    });
  }

  // Keep "#E3.O1"-style anchors visible: open the evidence affordance they live in.
  if (window.location.hash) {
    const target = document.getElementById(decodeURIComponent(window.location.hash.slice(1)));
    const details = target && target.closest("details");
    if (details) details.open = true;
  }
})();
