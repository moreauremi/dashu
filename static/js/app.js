// Comportements communs à toutes les pages de dashu.
//
// Ce fichier est chargé une seule fois. htmx (hx-boost) remplace ensuite le
// contenu de <body> à chaque navigation : tout passe donc par des écouteurs
// posés sur `document`, qui survivent à ces remplacements, et par `onPage()`,
// rejouée à chaque nouvelle page.
//
// Aucune requête n'est écrite ici : les échanges avec le serveur sont décrits
// par les attributs hx-* des gabarits.

(() => {
  "use strict";

  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform);

  // ---------------------------------------------------------------- Heure

  const parisTime = new Intl.DateTimeFormat("fr-FR", {
    timeZone: "Europe/Paris",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });

  /** Minutes écoulées depuis minuit, à Paris. */
  function parisMinutes() {
    const parts = parisTime.formatToParts(new Date());
    const get = (type) => Number(parts.find((p) => p.type === type).value);
    return get("hour") * 60 + get("minute");
  }

  function updateClock() {
    const clock = $("[data-clock]");
    if (clock) clock.textContent = parisTime.format(new Date());
  }

  /** Déplace la ligne « maintenant » de l'agenda (une rangée = 5 minutes). */
  function updateNowLine() {
    const line = $("[data-nowline]");
    const cal = $(".cal");
    if (!line || !cal) return;
    const first = Number(cal.dataset.firstHour) * 60;
    const last = first + Number(cal.dataset.hours) * 60;
    const now = parisMinutes();
    line.hidden = now < first || now >= last;
    if (!line.hidden) line.className = `nowline gs${Math.round((now - first) / 5)}`;
  }

  setInterval(() => {
    updateClock();
    updateNowLine();
  }, 20_000);

  // ------------------------------------------------- À chaque nouvelle page

  function onPage() {
    const app = $("#app");

    // Le thème choisi est porté par #app (remplacé à chaque navigation) ; on
    // le recopie sur <html>, que htmx ne touche pas.
    if (app) {
      if (app.dataset.theme) document.documentElement.dataset.theme = app.dataset.theme;
      else delete document.documentElement.dataset.theme;
    }

    $$("[data-mod-key]").forEach((kbd) => {
      kbd.textContent = isMac ? "⌘ K" : "Ctrl K";
    });

    // Message flash : visible 4 secondes.
    $$("[data-toast]").forEach((toast) => {
      setTimeout(() => toast.classList.add("is-leaving"), 4000);
      setTimeout(() => toast.remove(), 4300);
    });

    // Agenda sur petit écran : la semaine défile horizontalement, on amène
    // la colonne d'aujourd'hui à l'écran.
    const today = $(".cal-dayhead.is-today");
    const scroller = $(".cal-scroll");
    if (today && scroller) scroller.scrollLeft = today.offsetLeft - $(".cal-corner").offsetWidth;

    updateClock();
    updateNowLine();
  }

  document.addEventListener("DOMContentLoaded", onPage);
  document.addEventListener("htmx:afterSettle", (event) => {
    if (event.detail.target !== document.body) return;
    onPage();
    // Navigation au clavier : après un changement de page, le focus repart du
    // contenu (sauf si la page l'a déjà placé, comme l'éditeur).
    if (document.activeElement === document.body) $("#main")?.focus({ preventScroll: true });
  });

  // Bouton « Précédent » du navigateur : htmx redemande la page au serveur
  // et la remet en place, il faut la préparer comme une autre.
  document.addEventListener("htmx:historyRestore", onPage);

  // ------------------------------------------------ Menu sur petit écran

  function setNav(open) {
    const app = $("#app");
    if (!app) return;
    app.toggleAttribute("data-nav-open", open);
    $("[data-nav-toggle]")?.setAttribute("aria-expanded", String(open));
  }

  // ------------------------------------------------- Palette de commandes

  const palette = () => $("#palette");

  function openPalette() {
    const dialog = palette();
    if (!dialog || dialog.open) return;
    setNav(false);
    dialog.showModal();
    const input = $("#palette-q");
    input.value = "";
    input.focus();
    // Charge les résultats par défaut (pages, actions, notes récentes).
    window.htmx.trigger(input, "palette-open");
  }

  function paletteItems() {
    return $$("#palette-results .pal-item");
  }

  function selectPaletteItem(index) {
    const items = paletteItems();
    if (!items.length) return;
    const wrapped = (index + items.length) % items.length;
    items.forEach((item, i) => item.setAttribute("aria-selected", String(i === wrapped)));
    items[wrapped].scrollIntoView({ block: "nearest" });
  }

  function paletteKey(event) {
    const items = paletteItems();
    const current = items.findIndex((item) => item.getAttribute("aria-selected") === "true");
    if (event.key === "ArrowDown") {
      event.preventDefault();
      selectPaletteItem(current + 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      selectPaletteItem(current - 1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      (items[current] || items[0])?.click();
    }
  }

  // Nouveaux résultats : le premier est présélectionné.
  document.addEventListener("htmx:afterSwap", (event) => {
    // (Pas de cible quand htmx restaure une page depuis l'historique.)
    if (event.detail.target?.id === "palette-results") selectPaletteItem(0);
  });

  // ------------------------------------------------------------- Clics

  document.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;

    if (target.closest("[data-palette-open]")) return openPalette();
    if (target.closest("[data-nav-toggle]")) return setNav(!$("#app")?.hasAttribute("data-nav-open"));
    if (target.closest("[data-nav-close]")) return setNav(false);

    // Clic sur le fond assombri : ferme la palette.
    if (target === palette()) palette().close();

    // Clic en dehors d'un menu ouvert : on le referme.
    $$("details.menu[open]").forEach((menu) => {
      if (!menu.contains(target)) menu.open = false;
    });
  });

  // ----------------------------------------------------------- Clavier

  /** Vrai si la frappe est destinée à un champ de saisie. */
  function isTyping(event) {
    const el = event.composedPath()[0];
    if (!(el instanceof Element)) return false;
    return el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName);
  }

  // « G » attend une seconde lettre : G puis D, N ou C.
  let pendingG = 0;

  function go(key) {
    $(`[data-key="${key}"]`)?.click();
  }

  document.addEventListener("keydown", (event) => {
    // Ctrl+K / Cmd+K : partout, même en pleine saisie.
    if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (palette()?.open) palette().close();
      else openPalette();
      return;
    }

    if (palette()?.open) return paletteKey(event);

    if (event.key === "Escape") {
      setNav(false);
      $$("details.menu[open]").forEach((menu) => (menu.open = false));
      return;
    }

    // Raccourcis à une lettre : jamais pendant une saisie ni avec un modificateur.
    if (event.ctrlKey || event.metaKey || event.altKey || isTyping(event)) return;

    const key = event.key.toLowerCase();
    if (pendingG && Date.now() - pendingG < 1000 && ["d", "n", "c"].includes(key)) {
      pendingG = 0;
      event.preventDefault();
      return go(`g ${key}`);
    }
    pendingG = 0;

    if (key === "g") {
      pendingG = Date.now();
    } else if (key === "n") {
      event.preventDefault();
      go("n");
    }
  });
})();
