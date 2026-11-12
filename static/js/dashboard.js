// Dashboard modulaire (templates/dashboard/page.html) : mode édition,
// déplacement et taille des widgets, ajout et retrait.
//
// Les widgets sont des cases d'une grille bento : leur taille tient dans deux
// classes (w1…w4, h1…h4) et dans data-w / data-h. Rien ne passe par un
// attribut style (la CSP l'interdit) : pendant un glisser, la grille se
// réorganise en direct, le widget ne suit pas le doigt.
//
// À chaque changement, la disposition part au serveur (POST /dashboard/layout),
// pour l'espace actif. Écouteurs posés sur document : ils survivent aux
// navigations htmx, ce fichier ne s'exécute qu'une fois.
(() => {
  if (window.dashuDashboard) return;
  window.dashuDashboard = true;

  const MAX = 4;
  const $ = (selector, root = document) => root.querySelector(selector);
  const dash = () => $("[data-dash]");
  const grid = () => $("[data-dash-grid]");
  const editing = () => !!dash()?.classList.contains("editing");
  const widgets = () => [...(grid()?.querySelectorAll(".dw") ?? [])];
  const clamp = (n, min, max) => Math.min(max, Math.max(min, n));

  function csrf() {
    try {
      return JSON.parse($("#app").getAttribute("hx-headers"))["X-CSRF-Token"];
    } catch {
      return "";
    }
  }

  /** « courses:2x2,tasks:1x2 » : l'ordre et la taille de chaque widget. */
  function layout() {
    return widgets()
      .map((w) => `${w.dataset.kind}:${w.dataset.w}x${w.dataset.h}`)
      .join(",");
  }

  function status(text) {
    const node = $("[data-dash-status]");
    if (node) node.textContent = text;
  }

  // Les envois partent l'un après l'autre : le dernier arrivé gagne toujours.
  let queue = Promise.resolve();
  function send(value, empty) {
    const body = new URLSearchParams({ layout: value });
    if (empty) body.set("empty", "1");
    status("Enregistrement…");
    queue = queue
      .then(() =>
        fetch("/dashboard/layout", {
          method: "POST",
          headers: { "X-CSRF-Token": csrf() },
          body,
        }),
      )
      .then((response) => status(response.ok ? "Enregistré" : "Pas enregistré : recharge la page"))
      .catch(() => status("Pas enregistré : connexion perdue"));
    return queue;
  }

  const save = () => send(layout(), widgets().length === 0);

  /** Recharge la grille (après un ajout ou un retour à la disposition par
   *  défaut), une fois les envois en cours arrivés. */
  async function reloadGrid() {
    await queue;
    await htmx.ajax("GET", "/dashboard/grid", { target: "[data-dash-grid]", swap: "outerHTML" });
    syncCatalog();
  }

  /** Recharge le contenu d'un widget à sa nouvelle taille. */
  function reloadBody(widget) {
    const kind = widget.dataset.kind;
    htmx.ajax("GET", `/widgets/${kind}?w=${widget.dataset.w}&h=${widget.dataset.h}`, {
      target: `#w-${kind}`,
      swap: "outerHTML",
    });
  }

  /** Change la taille ; vrai si elle a changé. */
  function resize(widget, w, h) {
    w = clamp(w, 1, MAX);
    h = clamp(h, 1, MAX);
    if (w === +widget.dataset.w && h === +widget.dataset.h) return false;
    widget.classList.remove(`w${widget.dataset.w}`, `h${widget.dataset.h}`);
    widget.dataset.w = w;
    widget.dataset.h = h;
    widget.classList.add(`w${w}`, `h${h}`);
    for (const button of widget.querySelectorAll('[data-dw="size"]')) {
      button.setAttribute("aria-pressed", String(+button.dataset.w === w && +button.dataset.h === h));
    }
    return true;
  }

  /** Les widgets déjà présents ne s'ajoutent pas une seconde fois. */
  function syncCatalog() {
    const present = new Set(widgets().map((w) => w.dataset.kind));
    for (const option of document.querySelectorAll("[data-dash-option]")) {
      const used = present.has(option.dataset.kind);
      option.classList.toggle("is-used", used);
      for (const button of option.querySelectorAll("button")) button.disabled = used;
    }
  }

  function setEditing(on) {
    dash()?.classList.toggle("editing", on);
    status("");
    if (on) $('[data-dw="move"]', grid())?.focus();
    else $("[data-dash-edit]")?.focus();
  }

  // --- Clics -------------------------------------------------------------

  document.addEventListener("click", async (event) => {
    const target = event.target.closest(
      "[data-dash-edit], [data-dash-done], [data-dash-add], [data-dash-close], [data-dash-reset], [data-dash-pick], [data-dw]",
    );
    if (!target || !dash()?.contains(target)) return;

    if (target.matches("[data-dash-edit]")) return setEditing(true);
    if (target.matches("[data-dash-done]")) return setEditing(false);
    if (target.matches("[data-dash-add]")) {
      setEditing(true);
      syncCatalog();
      return $("[data-dash-catalog]")?.showModal();
    }
    if (target.matches("[data-dash-close]")) return $("[data-dash-catalog]")?.close();
    if (target.matches("[data-dash-reset]")) {
      if (!confirm("Revenir à la disposition par défaut pour cet espace ?")) return;
      await send("", false);
      return reloadGrid();
    }
    if (target.matches("[data-dash-pick]")) {
      const { kind, w, h } = target.dataset;
      $("[data-dash-catalog]")?.close();
      await send([layout(), `${kind}:${w}x${h}`].filter(Boolean).join(","), false);
      await reloadGrid();
      grid().querySelector(`.dw[data-kind="${kind}"]`)?.scrollIntoView({ block: "nearest" });
      return;
    }

    const widget = target.closest(".dw");
    if (!widget || !editing()) return;
    if (target.dataset.dw === "size") {
      if (resize(widget, +target.dataset.w, +target.dataset.h)) {
        save();
        reloadBody(widget);
      }
    } else if (target.dataset.dw === "remove") {
      const next = widget.nextElementSibling ?? widget.previousElementSibling;
      widget.remove();
      save();
      syncCatalog();
      $('[data-dw="move"]', next ?? document)?.focus();
      if (widgets().length === 0) reloadGrid();
    }
  });

  // --- Glisser : déplacer (poignée) ou agrandir (coin) ------------------

  let drag = null;

  document.addEventListener("pointerdown", (event) => {
    if (!editing() || event.button > 0) return;
    const handle = event.target.closest('[data-dw="move"], [data-dw="resize"]');
    const widget = handle?.closest(".dw");
    if (!widget) return;
    event.preventDefault();
    handle.setPointerCapture?.(event.pointerId);

    // Pas d'une colonne et d'une rangée, écarts compris, pour l'agrandissement.
    const style = getComputedStyle(grid());
    const columns = style.gridTemplateColumns.split(" ").length;
    const gap = parseFloat(style.columnGap) || 0;
    const rowGap = parseFloat(style.rowGap) || 0;
    const width = grid().getBoundingClientRect().width;
    drag = {
      mode: handle.dataset.dw,
      widget,
      x: event.clientX,
      y: event.clientY,
      w: +widget.dataset.w,
      h: +widget.dataset.h,
      stepX: (width - gap * (columns - 1)) / columns + gap,
      stepY: (parseFloat(style.gridAutoRows) || 140) + rowGap,
      changed: false,
    };
    widget.classList.add("dragging");
  });

  document.addEventListener("pointermove", (event) => {
    if (!drag) return;
    if (drag.mode === "resize") {
      const w = drag.w + Math.round((event.clientX - drag.x) / drag.stepX);
      const h = drag.h + Math.round((event.clientY - drag.y) / drag.stepY);
      if (resize(drag.widget, w, h)) drag.changed = true;
      return;
    }
    // Déplacer : le widget prend la place de celui qu'on survole, avant ou
    // après lui selon le côté où se trouve le pointeur.
    const over = document.elementFromPoint(event.clientX, event.clientY)?.closest(".dw");
    if (!over || over === drag.widget || !grid().contains(over)) return;
    const box = over.getBoundingClientRect();
    const after = (event.clientX - box.left) / box.width + (event.clientY - box.top) / box.height > 1;
    const sibling = after ? over.nextElementSibling : over.previousElementSibling;
    if (sibling === drag.widget) return;
    over[after ? "after" : "before"](drag.widget);
    drag.changed = true;
  });

  function endDrag() {
    if (!drag) return;
    const { widget, mode, changed } = drag;
    drag = null;
    widget.classList.remove("dragging");
    if (!changed) return;
    save();
    if (mode === "resize") reloadBody(widget);
  }
  document.addEventListener("pointerup", endDrag);
  document.addEventListener("pointercancel", endDrag);

  // --- Clavier sur la poignée : flèches pour déplacer, Maj + flèches pour la taille

  document.addEventListener("keydown", (event) => {
    const handle = event.target.closest?.('[data-dw="move"]');
    if (!handle || !editing()) return;
    const widget = handle.closest(".dw");
    const keys = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const step = keys[event.key];
    if (!step) return;
    event.preventDefault();
    if (event.shiftKey) {
      if (resize(widget, +widget.dataset.w + step[0], +widget.dataset.h + step[1])) {
        save();
        reloadBody(widget);
      }
      return;
    }
    const backward = step[0] + step[1] < 0;
    const sibling = backward ? widget.previousElementSibling : widget.nextElementSibling;
    if (!sibling?.classList.contains("dw")) return;
    sibling[backward ? "before" : "after"](widget);
    handle.focus();
    save();
  });

  // Échap quitte le mode édition (le catalogue ouvert se ferme d'abord, seul).
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && editing() && !$("[data-dash-catalog]")?.open) setEditing(false);
  });
})();
