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

  // Écran de lancement : une fois effacé (fin de son animation en CSS), on le
  // retire de la page. Les animations du logo, à l'intérieur, sont ignorées.
  document.addEventListener("animationend", (event) => {
    if (event.target instanceof Element && event.target.matches("[data-splash]")) {
      event.target.remove();
    }
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
    if (!$("[data-island]") || $(".editor-page") || bar()?.open) return false;
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

  // ------------------------------------------------- Barre de commandes
  //
  // « / » hors d'un champ ouvre la barre, avec « / » déjà tapé ; effacer le
  // « / » la referme (SPEC, « Barre de commandes »). La liste des commandes
  // arrive du serveur à l'ouverture (GET /cmd/catalogue) : la complétion se
  // fait ici, sans l'attendre à chaque touche. Seules les suggestions de
  // /open (notes, fichiers) se cherchent sur le serveur (GET /cmd/suggest).
  //
  // Une ligne validée part par le formulaire htmx de layout.html (POST /cmd).
  // Le serveur la relit avec les mêmes règles (src/commands/mod.rs, à garder
  // en accord avec `analyse` et `findNode`) et répond par la page à ouvrir
  // (en-tête HX-Location, htmx s'en charge), ou par un message d'erreur
  // inséré dans #cmd-msg.

  const bar = () => $("#cmdbar");
  const field = () => $("#cmd-input");

  const cmd = {
    tree: null, // commandes reçues du serveur
    treeFor: "", // page pour laquelle elles ont été demandées
    loading: false,
    failed: false,
    list: [], // suggestions affichées
    index: -1, // suggestion en surbrillance
    chosen: false, // choisie aux flèches : Entrée la prend
    cycle: null, // Tab répétés : { base, start, list, index, value }
    history: null, // position dans l'historique, ou null
    help: false, // liste de /help affichée
    pendingEnter: false, // Entrée pressée avant l'arrivée des commandes
    remote: { q: null, items: [], timer: 0 },
  };

  /** Minuscules sans accents, comme `fold` côté serveur. */
  const fold = (text) =>
    text
      .toLowerCase()
      .normalize("NFD")
      .replace(/[̀-ͯ]/g, "");

  const wordsOf = (node) => [node.name, ...(node.aliases || [])];
  const matches = (choice, typed) =>
    [choice.v, ...(choice.alias || [])].some((w) => fold(w).startsWith(typed));

  /** Mots d'un texte, avec leur position. */
  function tokens(text) {
    const out = [];
    for (const m of text.matchAll(/\S+/g)) out.push({ text: m[0], start: m.index });
    return out;
  }

  /** La commande d'un niveau que désigne un mot (mêmes règles que le serveur). */
  function findNode(level, word) {
    const w = fold(word);
    const exact = level.find((n) => wordsOf(n).some((x) => fold(x) === w));
    if (exact) return { node: exact };
    const matching = level.filter((n) => wordsOf(n).some((x) => fold(x).startsWith(w)));
    const open = matching.filter((n) => !n.off);
    if (open.length === 1) return { node: open[0] };
    if (!open.length && matching.length === 1) return { node: matching[0] };
    if (!matching.length) return { unknown: true };
    return { many: (open.length ? open : matching).map((n) => n.name) };
  }

  const namesOf = (level) =>
    level
      .filter((n) => !n.off)
      .map((n) => n.name)
      .join(", ");

  /**
   * Lit la ligne comme le serveur. Rend la commande atteinte (`node`, son
   * chemin `path`), puis ce qui est en cours de frappe : un mot du niveau
   * `level` (`token`, à partir de `start`), ou l'argument de la commande
   * (`arg`, à partir de `argStart`). `error` si la ligne ne peut pas aboutir.
   */
  function analyse(value) {
    const body = value.slice(1);
    const list = tokens(body);
    const done = body === "" || /\s$/.test(body);
    let level = cmd.tree || [];
    let node = null;
    const path = [];
    const withArg = (from) => {
      const arg = body.slice(from);
      const last = done ? null : list[list.length - 1];
      return {
        node,
        path,
        arg,
        argStart: from + 1,
        token: last && last.start >= from ? last.text : "",
        start: last && last.start >= from ? last.start + 1 : value.length,
      };
    };
    for (let i = 0; i < list.length; i++) {
      const word = list[i];
      if (node && !node.children) return withArg(word.start);
      if (i === list.length - 1 && !done) return { node, path, level, token: word.text, start: word.start + 1 };
      const found = findNode(level, word.text);
      if (found.unknown) {
        return {
          node,
          path,
          error: path.length
            ? `« ${word.text} » n'existe pas après /${path.join(" ")} : ${namesOf(level)}.`
            : `Commande inconnue : /${word.text}. /help les liste toutes.`,
        };
      }
      if (found.many) return { node, path, error: `« ${word.text} » peut être : ${found.many.join(", ")}.` };
      if (found.node.off) return { node: found.node, path, error: found.node.off };
      node = found.node;
      path.push(node.name);
      level = node.children || [];
    }
    if (node && !node.children) return withArg(body.length);
    return { node, path, level, token: "", start: value.length };
  }

  /** Ce qui manque pour lancer la ligne (mêmes messages que le serveur), ou "". */
  function missing(value) {
    const a = analyse(`${value.trimEnd()} `);
    if (a.error) return a.error;
    const node = a.node;
    const path = a.path.join(" ");
    if (!node) return "Tape une commande après « / », ou /help pour les voir toutes.";
    if (node.children && !node.run) {
      const next = node.children.filter((c) => !c.off).map((c) => `/${path} ${c.name}`);
      return `Il manque la suite : ${next.join(", ")}.`;
    }
    const arg = (a.arg || "").trim();
    if (node.arg?.required && !arg) return `Il manque la suite : /${path} ${node.arg.usage}.`;
    if (!node.arg && arg) return `Rien n'est attendu après /${path}.`;
    return "";
  }

  /** Suggestions pour la saisie en cours. */
  function suggestions(a) {
    if (a.error) return [];
    const typed = fold(a.token);
    if (a.arg === undefined) {
      // Un nom de commande : d'abord ceux qui commencent par la saisie.
      return a.level
        .filter((n) => !n.off && wordsOf(n).some((w) => fold(w).startsWith(typed)))
        .sort((x, y) => fold(y.name).startsWith(typed) - fold(x.name).startsWith(typed))
        .map((n) => ({ v: n.name, help: n.help, node: n }));
    }
    const spec = a.node.arg;
    if (!spec) return [];
    if (spec.mode === "remote") return cmd.remote.q === a.arg.trim() ? cmd.remote.items : [];
    // Texte libre : on ne propose qu'une fois un mot commencé (@ecole, demain).
    if (spec.mode === "free" && !typed) return [];
    return (spec.values || []).filter((c) => matches(c, typed)).map((c) => ({ ...c, mode: spec.mode }));
  }

  /** /help : toutes les commandes disponibles, avec leur usage. */
  function helpList() {
    const out = [];
    const walk = (level, prefix) => {
      for (const node of level) {
        if (node.off) continue;
        const name = `${prefix}${node.name}`;
        if (node.run || !node.children) {
          out.push({ v: `/${name}${node.arg ? ` ${node.arg.usage}` : ""}`, help: node.help, insert: `/${name}` });
        }
        if (node.children) walk(node.children, `${name} `);
      }
    };
    walk(cmd.tree || [], "");
    return out;
  }

  // --- Affichage

  /** Recalcule les suggestions d'après la saisie, puis redessine. */
  function update() {
    const input = field();
    if (!input) return;
    const a = analyse(input.value);
    if (cmd.cycle) {
      cmd.list = cmd.cycle.list;
      cmd.index = cmd.cycle.index;
    } else {
      cmd.list = cmd.help ? helpList() : suggestions(a);
      // La première est en surbrillance : c'est elle que Tab complète.
      cmd.index = cmd.list.length ? 0 : -1;
      cmd.chosen = false;
      if (a.node?.arg?.mode === "remote") fetchRemote(a.arg.trim());
    }
    draw(a);
  }

  /** Suite grisée dans la ligne : ce que Tab ajouterait au mot commencé. */
  function ghost(a) {
    const typed = a.node?.arg?.mode === "remote" ? a.arg.trimStart() : a.token;
    const s = !cmd.cycle && !cmd.help && cmd.list[Math.max(cmd.index, 0)];
    return s && typed && fold(s.v).startsWith(fold(typed)) ? s.v.slice(typed.length) : "";
  }

  /** Dessine la liste, la suite grisée et la ligne d'aide. */
  function draw(a = analyse(field().value)) {
    const input = field();
    const value = input.value;

    // Liste
    const ul = $("#cmd-list");
    ul.replaceChildren(
      ...cmd.list.map((s, i) => {
        const li = document.createElement("li");
        li.id = `cmd-opt-${i}`;
        li.className = s.href ? "cmd-item is-item" : "cmd-item";
        li.setAttribute("role", "option");
        li.setAttribute("aria-selected", String(i === cmd.index));
        li.dataset.index = String(i);
        const main = document.createElement("span");
        main.className = "cmd-main";
        const name = document.createElement("span");
        name.className = "cmd-name";
        name.textContent = s.v;
        main.append(name);
        if (s.sub) {
          const sub = document.createElement("span");
          sub.className = "cmd-sub";
          sub.textContent = s.sub;
          main.append(sub);
        }
        const help = document.createElement("span");
        help.className = "cmd-desc";
        help.textContent = s.help || "";
        li.append(main, help);
        return li;
      }),
    );
    if (cmd.index >= 0) {
      input.setAttribute("aria-activedescendant", `cmd-opt-${cmd.index}`);
      $(`#cmd-opt-${cmd.index}`)?.scrollIntoView({ block: "nearest" });
    } else {
      input.removeAttribute("aria-activedescendant");
    }

    $(".cmd-typed").textContent = value;
    $(".cmd-rest").textContent = input.scrollWidth > input.clientWidth ? "" : ghost(a);

    // Aide : usage de la commande, ou erreur en rouge.
    const hint = $("#cmd-hint");
    hint.classList.toggle("is-error", cmd.failed || (!!cmd.tree && !!a.error));
    if (cmd.failed) hint.textContent = "Commandes indisponibles : vérifie la connexion, puis rouvre la barre.";
    else if (!cmd.tree) hint.textContent = "Chargement des commandes…";
    else if (a.error) hint.textContent = a.error;
    else if (cmd.help) hint.textContent = "Toutes les commandes. Tab, ou ↑ ↓ puis Entrée, en reprend une.";
    else if (a.arg === undefined && a.token && !cmd.list.length) {
      hint.textContent = a.path.length
        ? `« ${a.token} » n'existe pas après /${a.path.join(" ")} : ${namesOf(a.level)}.`
        : `Aucune commande ne commence par « ${a.token} ». /help les liste toutes.`;
      hint.classList.add("is-error");
    } else if (a.node) {
      const usage = a.node.arg ? ` ${a.node.arg.usage}` : a.node.children ? ` <${namesOf(a.node.children).replaceAll(", ", "|")}>` : "";
      hint.textContent = `/${a.path.join(" ")}${usage} · ${a.node.help}`;
    } else if (matchMedia("(pointer: coarse)").matches) {
      hint.textContent = "Touche une suggestion pour la reprendre. /help liste toutes les commandes.";
    } else hint.textContent = "Tab complète. /help liste toutes les commandes.";
  }

  /** Suggestions de /open, cherchées pendant la frappe. */
  function fetchRemote(q) {
    const remote = cmd.remote;
    if (remote.q === q || remote.pending === q) return;
    remote.pending = q;
    clearTimeout(remote.timer);
    remote.timer = setTimeout(async () => {
      try {
        const response = await fetch(`/cmd/suggest?q=${encodeURIComponent(q)}`, { headers: tabHeaders() });
        if (!response.ok) throw new Error(String(response.status));
        const data = await response.json();
        if (remote.pending !== q) return;
        remote.q = q;
        remote.items = data.items || [];
        update();
      } catch {
        remote.pending = null;
      }
    }, 120);
  }

  /** En-têtes d'une requête de la barre, comme celles de htmx. */
  function tabHeaders() {
    let tab = "";
    try {
      tab = JSON.parse($("#app").getAttribute("hx-headers"))["X-Espace"] || "";
    } catch {}
    return { "HX-Request": "true", "X-Espace": tab };
  }

  async function loadTree() {
    const page = $("#cmd-page")?.value || "/";
    if ((cmd.tree && cmd.treeFor === page) || cmd.loading) return;
    cmd.loading = true;
    cmd.failed = false;
    try {
      const response = await fetch(`/cmd/catalogue?page=${encodeURIComponent(page)}`, { headers: tabHeaders() });
      if (!response.ok) throw new Error(String(response.status));
      cmd.tree = (await response.json()).commands;
      cmd.treeFor = page;
    } catch {
      cmd.failed = true;
    }
    cmd.loading = false;
    update();
    // Entrée pressée pendant le chargement : on la rejoue.
    if (cmd.pendingEnter && cmd.tree) {
      cmd.pendingEnter = false;
      enter();
    }
  }

  // --- Ouvrir, fermer

  function openBar() {
    const dialog = bar();
    if (!dialog || dialog.open) return;
    setNav(false);
    Object.assign(cmd, { cycle: null, history: null, help: false, chosen: false, index: -1, list: [] });
    Object.assign(cmd.remote, { q: null, pending: null, items: [] });
    dialog.showModal();
    const input = field();
    input.value = "/";
    input.focus();
    input.setSelectionRange(1, 1);
    $("#cmd-msg").replaceChildren();
    cmd.pendingEnter = false;
    update();
    loadTree();
  }

  const closeBar = () => bar()?.open && bar().close();

  // Changement de page en cours (lien, ou page ouverte par une commande) :
  // la nouvelle page remplacera la barre. « / » tapé entre-temps l'ouvre
  // donc une fois la page en place.
  let navigating = false;
  let openAfter = false;
  document.addEventListener("htmx:beforeRequest", (event) => {
    if (event.detail.target === document.body) navigating = true;
  });
  document.addEventListener("htmx:afterRequest", (event) => {
    if (event.detail.target === document.body && !event.detail.successful) navigating = false;
  });

  /** « / » : ouvre la barre, ou dès que la page en cours d'arrivée est là. */
  function askBar() {
    if (navigating) openAfter = true;
    else openBar();
  }

  // Nouvelle page : ses commandes seront redemandées (note, fichier ouverts…).
  document.addEventListener("htmx:afterSettle", (event) => {
    if (event.detail.target !== document.body) return;
    cmd.tree = null;
    navigating = false;
    if (openAfter) {
      openAfter = false;
      openBar();
    }
  });

  // --- Saisie

  /** Remplace ce qui est en cours de frappe par une suggestion. */
  function completion(base, start, s) {
    if (s.insert) return `${s.insert} `;
    // Une commande qui attend une suite, ou un mot de texte libre : un espace
    // pour enchaîner.
    const more = s.node ? s.node.children || s.node.arg : s.mode === "free";
    return `${base.slice(0, start)}${s.v}${more ? " " : ""}`;
  }

  /** Début de ce que remplace une suggestion, dans la ligne. */
  function replaceFrom(value) {
    const a = analyse(value);
    if (cmd.help) return 0;
    return a.node?.arg?.mode === "remote" ? a.argStart : a.start;
  }

  function setValue(value) {
    const input = field();
    input.value = value;
    input.setSelectionRange(value.length, value.length);
    $("#cmd-msg").replaceChildren();
  }

  /** Tab : complète, puis passe à la suggestion suivante (Maj : précédente). */
  function tab(back) {
    const input = field();
    if (cmd.cycle && input.value === cmd.cycle.value) {
      const c = cmd.cycle;
      c.index = (c.index + (back ? -1 : 1) + c.list.length) % c.list.length;
      c.value = completion(c.base, c.start, c.list[c.index]);
      setValue(c.value);
      return update();
    }
    if (!cmd.list.length) return;
    const index = Math.max(cmd.index, 0);
    const base = input.value;
    const start = replaceFrom(base);
    const value = completion(base, start, cmd.list[index]);
    const list = cmd.list;
    cmd.help = false;
    // Une seule possibilité : on passe directement au niveau suivant.
    cmd.cycle = list.length > 1 ? { base, start, list, index, value } : null;
    setValue(value);
    update();
  }

  function move(step) {
    if (!cmd.list.length) return;
    cmd.index = (cmd.index + step + cmd.list.length) % cmd.list.length;
    cmd.chosen = true;
    if (cmd.cycle) {
      // Pendant les Tab, les flèches choisissent dans la même liste.
      const c = cmd.cycle;
      c.index = cmd.index;
      c.value = completion(c.base, c.start, c.list[c.index]);
      setValue(c.value);
    }
    draw();
  }

  // --- Historique (dans ce navigateur seulement)

  const HISTORY = "dashu.commandes";

  function history() {
    try {
      const list = JSON.parse(localStorage.getItem(HISTORY));
      return Array.isArray(list) ? list.filter((l) => typeof l === "string") : [];
    } catch {
      return [];
    }
  }

  function remember(line) {
    try {
      const list = history().filter((l) => l !== line);
      list.push(line);
      localStorage.setItem(HISTORY, JSON.stringify(list.slice(-50)));
    } catch {}
  }

  /** ↑ / ↓ sur un « / » seul, ou pendant qu'on parcourt l'historique. */
  function browseHistory(step) {
    const list = history();
    const at = (cmd.history ?? list.length) + step;
    if (at < 0 || !list.length) return;
    cmd.cycle = null;
    if (at >= list.length) {
      cmd.history = null;
      setValue("/");
    } else {
      cmd.history = at;
      setValue(list[at]);
    }
    update();
  }

  // --- Exécution

  /** Ouvre une page comme un lien de l'application (suggestion de /open). */
  function navigate(href) {
    closeBar();
    window.htmx.ajax("GET", href, {
      source: "#app",
      target: "body",
      swap: "innerHTML transition:true",
      headers: { "HX-Boosted": "true" },
      push: "true",
    });
  }

  function refuse(message) {
    const hint = $("#cmd-hint");
    hint.textContent = message;
    hint.classList.add("is-error");
  }

  function enter() {
    const input = field();
    if (!cmd.tree) {
      // Commandes pas encore arrivées : Entrée attend le chargement.
      cmd.pendingEnter = !cmd.failed;
      return;
    }
    // Une suggestion choisie aux flèches (ou celle de /open), ou la suite
    // grisée d'un mot commencé (« dem » -> « demain ») : on la prend.
    const s = cmd.list[cmd.index];
    if (s && (cmd.chosen || s.href || ghost(analyse(input.value)))) {
      if (s.href) {
        remember(input.value.trim());
        return navigate(s.href);
      }
      const value = completion(input.value, replaceFrom(input.value), s);
      cmd.help = false;
      cmd.cycle = null;
      setValue(value);
      update();
      // La commande attend encore quelque chose : on s'arrête là.
      if (missing(value)) return;
    }
    const line = input.value.trim();
    const problem = missing(line);
    if (problem) return refuse(problem);
    const node = analyse(`${line} `).node;
    if (node.client === "help") {
      cmd.help = true;
      cmd.cycle = null;
      setValue("/help");
      return update();
    }
    remember(line);
    if (node.client === "file") {
      // Un sélecteur de fichiers, que files.js envoie dans l'espace de la
      // page. Ouvert dans le geste de la touche : le navigateur l'exige.
      const picker = document.createElement("input");
      picker.type = "file";
      picker.multiple = true;
      picker.className = "sr-only";
      picker.tabIndex = -1;
      picker.dataset.uploadInput = "";
      document.body.append(picker);
      picker.click();
      return closeBar();
    }
    $("#cmd-form").requestSubmit();
  }

  function cmdKey(event) {
    const input = field();
    if (event.target !== input) return;
    if (event.key === "Tab") {
      event.preventDefault();
      cmd.history = null;
      tab(event.shiftKey);
    } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      event.preventDefault();
      const step = event.key === "ArrowUp" ? -1 : 1;
      if (cmd.history !== null || (input.value === "/" && step < 0)) browseHistory(step);
      else move(step);
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (!event.isComposing) enter();
    }
  }

  document.addEventListener("input", (event) => {
    if (event.target !== field()) return;
    const input = field();
    // Plus de « / » : on revient à la page.
    if (!input.value.startsWith("/")) return closeBar();
    cmd.cycle = null;
    cmd.history = null;
    cmd.help = false;
    $("#cmd-msg").replaceChildren();
    update();
  });

  // Toucher ou cliquer une suggestion : comme Tab (ou ouvrir une note).
  document.addEventListener("mousedown", (event) => {
    if (event.target instanceof Element && event.target.closest("#cmd-list")) event.preventDefault();
  });
  document.addEventListener("click", (event) => {
    const item = event.target instanceof Element && event.target.closest("#cmd-list .cmd-item");
    if (!item) return;
    cmd.index = Number(item.dataset.index);
    cmd.chosen = true;
    enter();
    field()?.focus();
  });

  // Requête de la barre partie : le message précédent s'efface ; la barre se
  // ferme si le serveur ouvre une page (htmx s'en charge ensuite).
  document.addEventListener("htmx:afterRequest", (event) => {
    if (event.detail.elt?.id !== "cmd-form") return;
    if (event.detail.successful && event.detail.xhr.getResponseHeader("HX-Location")) closeBar();
  });

  // ------------------------------------------------------------- Clics

  document.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;

    if (target.closest("[data-cmd-open]")) return openBar();
    if (target.closest("[data-nav-toggle]")) return setNav(!$("#app")?.hasAttribute("data-nav-open"));
    if (target.closest("[data-nav-close]")) return setNav(false);

    // Clic sur le fond assombri : ferme la barre de commandes.
    if (target === bar()) closeBar();

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
    if (bar()?.open) return cmdKey(event);

    if (event.key === "Escape") {
      setNav(false);
      $$("details.menu[open]").forEach((menu) => (menu.open = false));
      // Échap sort du champ en cours (l'éditeur de notes compris), pour que
      // « / » ouvre ensuite la barre de commandes. Sauf si le champ s'en est
      // déjà servi (fermer une liste, réduire une sélection…) ou s'il est
      // dans une fenêtre ou un menu, que la touche referme.
      const el = event.composedPath()[0];
      if (isTyping(event) && !event.defaultPrevented && !el.closest("dialog, [popover]")) el.blur();
      return;
    }

    // Raccourcis : jamais pendant une saisie ni avec Ctrl, Cmd ou Alt.
    if (event.ctrlKey || event.metaKey || event.altKey || isTyping(event)) return;

    // « / » ouvre la barre de commandes (sauf par-dessus une autre fenêtre).
    if (event.key === "/" && !$("dialog[open]")) {
      event.preventDefault();
      return askBar();
    }

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

  // ------------------------------------------------------------- Menus
  //
  // Menus de l'app (attribut popover, ouverts par popovertarget) : affichés
  // par-dessus la page, jamais coupés par un widget, fermés au clic dehors ou
  // par Échap. On les place sous leur bouton, aligné à droite, ou au-dessus
  // s'ils dépasseraient du bas de l'écran. La position passe par une feuille
  // construite (pas de style en ligne, voir la CSP).
  const menuSheet = new CSSStyleSheet();
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, menuSheet];

  /** Facteur de zoom de l'app (taille de l'interface). */
  const zoom = () => parseFloat(getComputedStyle($("#app") || document.body).getPropertyValue("--zoom")) || 1;

  function placeMenu(menu, above) {
    const opener = $(`[popovertarget="${CSS.escape(menu.id)}"]`);
    if (!opener) return;
    const z = zoom();
    const r = opener.getBoundingClientRect();
    const right = Math.max(8, innerWidth - r.right) / z;
    const top = above ? (r.top - 4 - menu.getBoundingClientRect().height) / z : (r.bottom + 4) / z;
    menuSheet.replaceSync(`#${CSS.escape(menu.id)} { top: ${Math.max(8 / z, top)}px; right: ${right}px; }`);
  }

  // Ces événements ne remontent pas : on les écoute à la capture.
  document.addEventListener(
    "beforetoggle",
    (event) => {
      const menu = event.target;
      if (menu instanceof HTMLElement && menu.matches(".pop-menu") && event.newState === "open") {
        placeMenu(menu, false);
      }
    },
    true,
  );
  document.addEventListener(
    "toggle",
    (event) => {
      const menu = event.target;
      if (!(menu instanceof HTMLElement) || !menu.matches(".pop-menu") || event.newState !== "open") return;
      if (menu.getBoundingClientRect().bottom > innerHeight - 8) placeMenu(menu, true);
      // Au clavier, le premier choix prend le focus.
      menu.querySelector("button:not([disabled]), input")?.focus({ preventScroll: true });
    },
    true,
  );

  // Choix d'un espace dans un formulaire (ajout d'une tâche) : la valeur et
  // le nom passent sur le bouton qui a ouvert le menu.
  document.addEventListener("click", (event) => {
    const choice = event.target instanceof Element && event.target.closest("[data-set-space]");
    if (!choice) return;
    const form = choice.closest("form");
    const menu = choice.closest(".pop-menu");
    const value = form?.querySelector("[data-space-value]");
    const button = form?.querySelector("[data-space-button]");
    if (!value || !button) return;
    value.value = choice.dataset.setSpace;
    button.dataset.accent = choice.dataset.color;
    button.setAttribute("aria-label", `Espace de la tâche : ${choice.dataset.name}`);
    const label = button.querySelector("[data-space-label]");
    if (label) label.textContent = choice.dataset.name;
    menu?.hidePopover();
    form.querySelector("input[name=title]")?.focus();
  });

  // ------------------------------------------------- Photo de profil

  // Réglages : la photo choisie est recadrée au carré et réduite à 256 px
  // ici même (canvas), puis envoyée en JPEG. Le serveur ne reçoit qu'une
  // petite image, qu'il relit et réenregistre (src/settings/profile.rs).
  document.addEventListener("change", async (event) => {
    const input = event.target.closest?.("[data-avatar-input]");
    const file = input?.files?.[0];
    if (!file) return;
    const status = $("[data-avatar-status]");
    const say = (text) => {
      if (status) status.textContent = text;
    };
    say("Envoi de la photo…");
    try {
      const bitmap = await createImageBitmap(file);
      const side = Math.min(bitmap.width, bitmap.height);
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 256;
      canvas
        .getContext("2d")
        .drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 256, 256);
      const jpeg = await new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.9));
      if (!jpeg) throw new Error("canvas");
      const token = JSON.parse($("#app").getAttribute("hx-headers"))["X-CSRF-Token"];
      const response = await fetch("/settings/avatar", {
        method: "POST",
        headers: { "X-CSRF-Token": token, "Content-Type": "image/jpeg" },
        body: jpeg,
      });
      if (!response.ok) throw new Error(String(response.status));
      // La page se recharge : la nouvelle photo apparaît partout, avec le message.
      window.location.reload();
    } catch {
      say("Cette image n'a pas pu être lue. Essaie une photo JPEG ou PNG.");
      input.value = "";
    }
  });
})();
