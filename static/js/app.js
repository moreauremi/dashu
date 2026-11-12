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
    syncThemeColor();

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

    bindWheel();
    $$("[data-tab-picker]").forEach(limitTabs);
    updateClock();
    updateNowLine();
  }

  // Réglages : quatre raccourcis au plus. Une fois le maximum atteint, les
  // cases restantes se grisent (le serveur vérifie aussi).
  function limitTabs(form) {
    const max = Number(form.dataset.max) || 4;
    const boxes = $$('input[type="checkbox"]', form);
    const count = boxes.filter((box) => box.checked).length;
    boxes.forEach((box) => (box.disabled = !box.checked && count >= max));
  }

  document.addEventListener("change", (event) => {
    const form = event.target instanceof Element && event.target.closest("[data-tab-picker]");
    if (form) limitTabs(form);
  });

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

  // ------------------------------------------- App sur l'écran d'accueil

  // Barre d'état de l'app : couleur de la barre du haut (--surface) dans le
  // thème imposé par les réglages, sinon dans celui du système (les deux
  // balises theme-color de base.html, une par thème).
  const BAR_COLOR = { light: "#ffffff", dark: "#181817" };

  function syncThemeColor() {
    const forced = BAR_COLOR[document.documentElement.dataset.theme];
    $$('meta[name="theme-color"]').forEach((meta) => {
      meta.content = forced || BAR_COLOR[meta.media.includes("dark") ? "dark" : "light"];
    });
  }

  // Ouverte depuis l'écran d'accueil, dashu ne zoome plus, comme une app
  // (dans le navigateur, le zoom reste permis). Le CSS coupe le double tap ;
  // ici on interdit le zoom dans le viewport, et comme iOS ignore cette
  // consigne depuis iOS 10, on bloque aussi le pincement lui-même.
  const standalone =
    matchMedia("(display-mode: standalone), (display-mode: fullscreen)").matches ||
    navigator.standalone === true;

  if (standalone) {
    const viewport = $('meta[name="viewport"]');
    if (viewport) viewport.content += ", maximum-scale=1, user-scalable=no";

    // Événements propres à Safari iOS : gesture* = pincement en cours.
    if ("ongesturestart" in window) {
      const block = (event) => event.preventDefault();
      document.addEventListener("gesturestart", block, { passive: false });
      document.addEventListener("gesturechange", block, { passive: false });
      document.addEventListener(
        "touchmove",
        (event) => {
          if (event.touches.length > 1) event.preventDefault();
        },
        { passive: false },
      );
    }
  }

  // ------------------------------------------------ Menu sur petit écran

  function setNav(open) {
    const app = $("#app");
    if (!app) return;
    app.toggleAttribute("data-nav-open", open);
    $("[data-nav-toggle]")?.setAttribute("aria-expanded", String(open));
  }

  // ------------------------------------------------- Espaces : île et balayage
  //
  // L'île (en haut) liste les espaces dans l'ordre : Perso, École, Taff.
  // On peut aussi balayer de côté, deux doigts au trackpad ou un doigt sur
  // téléphone : l'icône de l'espace voisin apparaît du côté du geste, dans un
  // rond qui se remplit ; quand il est plein, la page glisse vers cet espace.
  // Relâché avant, le rond se vide et rien ne se passe.

  /** Défilement horizontal (en px) pour remplir le rond au trackpad. */
  const WHEEL_DISTANCE = 320;
  /** Part de la largeur de l'écran à parcourir au doigt. */
  const TOUCH_SHARE = 0.38;
  /** Sans nouvel événement pendant ce délai, le geste est considéré relâché. */
  const RELEASE_MS = 160;
  /** Après un changement, on attend ce calme pour réarmer (inertie du trackpad). */
  const REARM_MS = 250;

  const spaceButtons = () => $$("[data-island] .isl-btn");
  const activeIndex = () =>
    spaceButtons().findIndex((b) => b.getAttribute("aria-current") === "true");

  /** Bouton de l'espace voisin : +1 à droite, −1 à gauche (pas de boucle). */
  function neighbour(dir) {
    const i = activeIndex();
    return i < 0 ? null : spaceButtons()[i + dir] || null;
  }

  /** Prépare le glissement de page, puis envoie le formulaire de l'espace. */
  function switchSpace(button, dir) {
    if (!button || button.getAttribute("aria-current") === "true") return;
    document.documentElement.dataset.slide = dir > 0 ? "next" : "prev";
    // L'île bascule tout de suite ; la page suit quand le serveur répond.
    spaceButtons().forEach((b) => b.toggleAttribute("aria-current", b === button));
    button.setAttribute("aria-current", "true");
    button.form.requestSubmit(button);
    setTimeout(() => delete document.documentElement.dataset.slide, 800);
  }

  // Clic dans l'île : même glissement, dans le sens du bouton choisi.
  document.addEventListener("click", (event) => {
    const button = event.target instanceof Element && event.target.closest("[data-island] .isl-btn");
    if (!button) return;
    event.preventDefault();
    const buttons = spaceButtons();
    switchSpace(button, Math.sign(buttons.indexOf(button) - activeIndex()));
  });

  const gesture = {
    dir: 0,
    progress: 0,
    releaseTimer: 0,
    rearmTimer: 0,
    blocked: false,
    // Plus petite amplitude vue depuis le blocage : l'inertie décroît.
    minAmp: Infinity,
  };

  function rearm() {
    clearTimeout(gesture.rearmTimer);
    gesture.blocked = false;
    gesture.minAmp = Infinity;
  }

  // L'avancement (--p) passe par une feuille de style construite, pas par un
  // attribut style : la Content-Security-Policy refuse tout style en ligne, et
  // htmx recopie le <body> quand il change de page.
  const progressSheet = new CSSStyleSheet();
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, progressSheet];
  const setProgress = (value) => progressSheet.replaceSync(`.swipe-hint { --p: ${value}; }`);

  function hint() {
    let el = $(".swipe-hint");
    if (!el) {
      el = document.createElement("div");
      el.className = "swipe-hint";
      el.setAttribute("aria-hidden", "true");
      document.body.append(el);
    }
    return el;
  }

  function showProgress(dir, progress) {
    const target = neighbour(dir);
    const el = hint();
    if (gesture.dir !== dir || !el.firstChild) {
      el.replaceChildren(...(target ? [target.querySelector("svg").cloneNode(true)] : []));
      el.dataset.accent = target?.dataset.accent || "";
      el.dataset.side = dir > 0 ? "right" : "left";
    }
    gesture.dir = dir;
    gesture.progress = Math.max(0, Math.min(1, progress));
    el.classList.remove("is-releasing", "is-done");
    el.classList.toggle("is-on", gesture.progress > 0.04);
    setProgress(gesture.progress);
    if (gesture.progress >= 1) complete();
  }

  function release() {
    clearTimeout(gesture.releaseTimer);
    const el = $(".swipe-hint");
    gesture.progress = 0;
    if (!el) return;
    el.classList.add("is-releasing");
    el.classList.remove("is-on");
    setProgress(0);
  }

  function complete() {
    const dir = gesture.dir;
    const el = hint();
    el.classList.add("is-done");
    gesture.blocked = true;
    gesture.minAmp = Infinity;
    gesture.progress = 0;
    clearTimeout(gesture.releaseTimer);
    // Filet de sécurité si la page ne change pas (réseau coupé…).
    clearTimeout(gesture.rearmTimer);
    gesture.rearmTimer = setTimeout(rearm, 1500);
    setTimeout(() => {
      el.classList.add("is-releasing");
      el.classList.remove("is-on", "is-done");
    }, 120);
    switchSpace(neighbour(dir), dir);
  }

  /** Vrai si un élément sous le pointeur peut encore défiler dans ce sens. */
  function scrollsHorizontally(node, dir) {
    for (let el = node; el instanceof Element && el !== document.body; el = el.parentElement) {
      if (el.scrollWidth <= el.clientWidth + 1) continue;
      const overflow = getComputedStyle(el).overflowX;
      if (overflow !== "auto" && overflow !== "scroll") continue;
      if (dir > 0 && el.scrollLeft + el.clientWidth < el.scrollWidth - 1) return true;
      if (dir < 0 && el.scrollLeft > 0) return true;
    }
    return false;
  }

  /** Le geste est-il permis à cet endroit ? */
  function swipeAllowed(target) {
    if (!$("[data-island]") || $(".editor-page") || palette()?.open) return false;
    if ($("#app")?.hasAttribute("data-nav-open")) return false;
    // Dashboard en mode édition : le doigt déplace les widgets, pas l'espace.
    if ($(".dash.editing")) return false;
    return !(target instanceof Element && target.closest("input, textarea, select, [contenteditable], .cm-editor"));
  }

  // Trackpad (ou Maj + molette) : on cumule le défilement horizontal.
  function onWheel(event) {
    const dx = event.deltaX * (event.deltaMode === 1 ? 16 : 1);

    // Juste après un changement, le trackpad continue sur sa lancée avec des
    // valeurs qui décroissent. On réarme au calme, ou dès qu'un net regain
    // d'amplitude montre que c'est un nouveau geste.
    if (gesture.blocked) {
      const amp = Math.abs(dx);
      gesture.minAmp = Math.min(gesture.minAmp, amp);
      clearTimeout(gesture.rearmTimer);
      gesture.rearmTimer = setTimeout(rearm, REARM_MS);
      if (!(gesture.minAmp < 6 && amp > Math.max(10, gesture.minAmp * 4))) return;
      rearm();
    }

    if (event.ctrlKey || Math.abs(dx) < 1 || Math.abs(dx) < Math.abs(event.deltaY) * 1.5) return;
    const dir = Math.sign(dx);
    if (!swipeAllowed(event.target) || scrollsHorizontally(event.target, dir)) return;

    // Retour en arrière pendant le geste : le rond se vide d'abord.
    let progress = gesture.dir === dir ? gesture.progress : -gesture.progress;
    progress += Math.abs(dx) / WHEEL_DISTANCE;
    if (progress < 0) {
      showProgress(-dir, -progress);
    } else if (neighbour(dir)) {
      showProgress(dir, progress);
    }
    clearTimeout(gesture.releaseTimer);
    gesture.releaseTimer = setTimeout(release, RELEASE_MS);
  }

  // L'écouteur est posé sur chaque #app, pas sur document. Au trackpad, Firefox
  // et Chrome envoient tout un geste (inertie comprise) à l'élément sous le
  // pointeur au début du geste. Après un changement de page, cet élément a
  // été retiré par htmx : ses événements ne remontent plus jusqu'à document,
  // mais ils passent encore par l'ancien #app, qui garde son écouteur. Sans
  // cela, il fallait cliquer pour refaire un balayage.
  const wheelBound = new WeakSet();
  function bindWheel() {
    const app = $("#app");
    if (!app || wheelBound.has(app)) return;
    wheelBound.add(app);
    app.addEventListener("wheel", onWheel, { passive: true });
  }

  // Doigt sur écran tactile.
  let touch = null;

  document.addEventListener(
    "touchstart",
    (event) => {
      touch = null;
      if (event.touches.length !== 1 || gesture.blocked) return;
      const t = event.touches[0];
      // Les bords de l'écran appartiennent au geste « retour » du système.
      if (t.clientX < 24 || t.clientX > window.innerWidth - 24) return;
      if (!swipeAllowed(event.target)) return;
      touch = { x: t.clientX, y: t.clientY, target: event.target, horizontal: null };
    },
    { passive: true },
  );

  document.addEventListener(
    "touchmove",
    (event) => {
      if (!touch || gesture.blocked) return;
      const t = event.touches[0];
      const dx = t.clientX - touch.x;
      const dy = t.clientY - touch.y;
      if (touch.horizontal === null) {
        if (Math.abs(dx) < 12 && Math.abs(dy) < 12) return;
        // Le premier mouvement net décide : vertical = on laisse défiler la page.
        touch.horizontal = Math.abs(dx) > Math.abs(dy) * 1.5;
        if (touch.horizontal && scrollsHorizontally(touch.target, -Math.sign(dx))) touch.horizontal = false;
      }
      if (!touch.horizontal) return;
      // Doigt vers la gauche = espace de droite, comme on tourne une page.
      const dir = -Math.sign(dx);
      if (dir && neighbour(dir)) showProgress(dir, Math.abs(dx) / (window.innerWidth * TOUCH_SHARE));
      else if (gesture.progress) release();
    },
    { passive: true },
  );

  const endTouch = () => {
    if (touch?.horizontal && !gesture.blocked) release();
    touch = null;
  };
  document.addEventListener("touchend", endTouch, { passive: true });
  document.addEventListener("touchcancel", endTouch, { passive: true });

  // Nouvelle page : le geste repart de zéro (réarmé après le calme).
  document.addEventListener("htmx:afterSettle", (event) => {
    if (event.detail.target !== document.body) return;
    gesture.dir = 0;
    gesture.progress = 0;
    if (gesture.blocked) {
      clearTimeout(gesture.rearmTimer);
      gesture.rearmTimer = setTimeout(rearm, REARM_MS);
    }
  });

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
    if (pendingG && Date.now() - pendingG < 1000 && ["d", "n", "f", "c"].includes(key)) {
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
