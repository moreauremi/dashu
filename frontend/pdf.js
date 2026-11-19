// Lecteur de PDF de la page d'un fichier (/files/{id}).
//
// pdf.js (Mozilla) dessine chaque page dans un <canvas>, avec par-dessus une
// couche de texte transparente : on sélectionne et on copie le texte, et la
// recherche du navigateur le trouve sur les pages déjà dessinées.
//
// Sécurité. Le PDF arrive comme une simple donnée (/files/{id}/raw, en
// application/octet-stream, CSP `sandbox`) : le navigateur ne l'affiche
// jamais lui-même. pdf.js le lit dans son worker ; il ne lance pas le
// JavaScript d'un PDF (pdf.sandbox n'est pas embarqué), ni les formulaires
// XFA, et la CSP de dashu interdit eval et WebAssembly.
//
// Chargé une fois, à la première page de PDF ouverte (app.js, `pdfViewer`),
// puis gardé : app.js appelle `mount` à chaque page, avec le lecteur de la
// nouvelle page, ou `null` pour fermer celui d'avant et rendre sa mémoire.
//
// Mémoire. Seules les pages proches de l'écran sont dessinées, les autres
// rendent leur canvas : Safari sur iPhone refuse au-delà de quelques
// centaines de Mo de canvas, et un long PDF n'y tiendrait pas.
//
// Les tailles calculées passent par `element.style` (CSSOM) : la CSP ne
// refuse que l'attribut `style` écrit dans le HTML, et la couche de texte de
// pdf.js procède de la même façon.

import {
  AnnotationMode,
  GlobalWorkerOptions,
  PasswordException,
  RenderingCancelledException,
  TextLayer,
  getDocument,
} from "pdfjs-dist/legacy/build/pdf.mjs";

/** 100 % : la taille réelle de la page (96 px CSS par pouce, 72 points PDF). */
const CSS_UNITS = 96 / 72;
/** Paliers des boutons de zoom. */
const STEPS = [0.5, 0.67, 0.8, 1, 1.25, 1.5, 2, 2.5, 3, 4];
/** Zoom d'ouverture : toute la largeur disponible, sans dépasser 125 %. */
const AUTO_MAX = 1.25;
/** Au plus 2 pixels par pixel CSS (un iPhone en a 3 : la mémoire d'abord). */
const MAX_RATIO = 2;
/** Un canvas ne dépasse pas 16 millions de pixels (limite de Safari). */
const MAX_PIXELS = 16 * 1024 * 1024;
/** Pages dessinées d'avance : une hauteur et demie d'écran de chaque côté. */
const AHEAD = "150% 0px";

/** Le lecteur ouvert, ou null. */
let viewer = null;

/** Branche le lecteur sur `el` (le `[data-pdf]` de la page), ou ferme celui
 *  qui est ouvert si `el` est null. */
export function mount(el) {
  if (viewer && viewer.el === el) return;
  close();
  if (el) open(el);
}

// ------------------------------------------------------------ Ouverture

function open(el) {
  const v = {
    el,
    sheet: el.querySelector(".pdf-pages"),
    bar: el.querySelector(".pdf-bar"),
    status: el.querySelector(".pdf-status"),
    pages: [],
    zoom: 1,
    scale: CSS_UNITS,
    auto: true,
    current: 1,
    shown: false,
    closed: false,
    doc: null,
    queue: Promise.resolve(),
    observers: [],
  };
  viewer = v;

  GlobalWorkerOptions.workerSrc = el.dataset.worker;
  v.task = getDocument({
    url: el.dataset.pdf,
    // Décodeurs des images JBIG2 et JPEG 2000 (PDF scannés), en JavaScript :
    // la CSP refuse WebAssembly.
    wasmUrl: new URL(el.dataset.decoders, location.href).href,
    useWasm: false,
    enableXfa: false,
  });
  v.task.onProgress = ({ loaded, total }) => {
    if (total && !v.doc && !v.closed) {
      say(v, `Chargement du PDF… ${Math.min(100, Math.round((loaded / total) * 100))} %`);
    }
  };
  v.task.promise.then((doc) => start(v, doc)).catch((error) => fail(v, error));

  v.bar.addEventListener("click", (event) => {
    const button = event.target instanceof Element && event.target.closest("button");
    if (!button || !v.doc) return;
    if (button.hasAttribute("data-pdf-fit")) rezoom(v, null);
    else step(v, Number(button.dataset.pdfStep));
  });
}

/** Le document est lu : une place par page, à la taille de la première
 *  (corrigée page par page au moment de la dessiner), puis on observe. */
async function start(v, doc) {
  if (v.closed) return;
  v.doc = doc;
  const first = await doc.getPage(1);
  if (v.closed) return;
  const base = first.getViewport({ scale: 1 });
  for (let n = 1; n <= doc.numPages; n++) {
    const div = document.createElement("div");
    div.className = "pdf-page";
    v.pages.push({
      n,
      div,
      page: n === 1 ? first : null,
      width: base.width,
      height: base.height,
      measured: false,
      wanted: false,
      queued: false,
      drawn: 0,
      render: null,
      canvas: null,
    });
  }
  v.sheet.replaceChildren(...v.pages.map((p) => p.div));
  v.el.classList.add("is-ready");
  v.bar.hidden = false;
  layout(v);
  count(v);
  watch(v);
}

function fail(v, error) {
  if (v.closed) return;
  v.el.classList.add("is-failed");
  const locked = error instanceof PasswordException || error?.name === "PasswordException";
  say(
    v,
    locked
      ? "Ce PDF est protégé par un mot de passe : télécharge-le pour l'ouvrir."
      : "Ce PDF ne s'ouvre pas ici : télécharge-le pour le lire.",
  );
}

function say(v, text) {
  v.status.textContent = text;
  v.status.hidden = !text;
}

// --------------------------------------------------------- Mise en page

/** Taille de toutes les pages pour le zoom en cours. En zoom automatique,
 *  la première page prend toute la largeur (125 % au plus). */
function layout(v) {
  if (v.auto) {
    const { paddingLeft, paddingRight } = getComputedStyle(v.sheet);
    const room = v.sheet.clientWidth - parseFloat(paddingLeft) - parseFloat(paddingRight);
    v.zoom = Math.max(0.1, Math.min(AUTO_MAX, room / (v.pages[0].width * CSS_UNITS)));
  }
  v.scale = v.zoom * CSS_UNITS;
  for (const p of v.pages) size(v, p);
  v.bar.querySelector("[data-pdf-fit]").textContent = `${Math.round(v.zoom * 100)} %`;
  const [out, inn] = v.bar.querySelectorAll("[data-pdf-step]");
  out.disabled = v.zoom <= STEPS[0] + 0.001;
  inn.disabled = v.zoom >= STEPS[STEPS.length - 1] - 0.001;
}

function size(v, p) {
  const { style } = p.div;
  style.width = `${Math.floor(p.width * v.scale)}px`;
  style.height = `${Math.floor(p.height * v.scale)}px`;
  // Lu par la couche de texte de pdf.js (positions et tailles du texte).
  style.setProperty("--scale-factor", String(v.scale));
}

/** Compteur « 3 / 12 » : la page au milieu de l'écran. */
function count(v) {
  v.bar.querySelector("[data-pdf-count]").textContent = `${v.current} / ${v.pages.length}`;
}

function watch(v) {
  const byDiv = new Map(v.pages.map((p) => [p.div, p]));

  // Pages proches de l'écran : dessinées ; les autres : effacées.
  const near = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const p = byDiv.get(entry.target);
        p.wanted = entry.isIntersecting;
        if (p.wanted) queue(v, p);
        else release(p);
      }
    },
    { rootMargin: AHEAD },
  );
  // La ligne du milieu de l'écran : la page en cours.
  const middle = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        v.current = byDiv.get(entry.target).n;
        count(v);
      }
    },
    { rootMargin: "-50% 0px -50% 0px" },
  );
  for (const p of v.pages) {
    near.observe(p.div);
    middle.observe(p.div);
  }

  // La colonne change de largeur (fenêtre, barre latérale, téléphone
  // tourné) : le zoom automatique suit.
  let width = v.sheet.clientWidth;
  let timer = 0;
  const resize = new ResizeObserver(() => {
    const now = v.sheet.clientWidth;
    if (now === width) return;
    width = now;
    if (!v.auto) return;
    clearTimeout(timer);
    timer = setTimeout(() => !v.closed && rezoom(v, null), 150);
  });
  resize.observe(v.sheet);

  v.observers.push(near, middle, resize);
}

// ------------------------------------------------------------------ Zoom

function step(v, direction) {
  const next =
    direction > 0
      ? STEPS.find((s) => s > v.zoom + 0.001)
      : STEPS.findLast((s) => s < v.zoom - 0.001);
  if (next) rezoom(v, next);
}

/** Change le zoom (null : ajuster à la largeur) en gardant sous les yeux le
 *  même endroit du document. Les pages déjà dessinées s'étirent le temps
 *  d'être redessinées. */
function rezoom(v, zoom) {
  const anchor = v.pages[v.current - 1].div;
  const before = anchor.getBoundingClientRect();
  const middle = window.innerHeight / 2;
  const fy = (middle - before.top) / before.height;
  const sheet = v.sheet;
  const fx = (sheet.scrollLeft + sheet.clientWidth / 2) / sheet.scrollWidth;

  v.auto = zoom === null;
  if (zoom !== null) v.zoom = zoom;
  layout(v);

  const after = anchor.getBoundingClientRect();
  window.scrollBy(0, after.top + fy * after.height - middle);
  sheet.scrollLeft = fx * sheet.scrollWidth - sheet.clientWidth / 2;
  for (const p of v.pages) {
    if (!p.wanted) continue;
    p.render?.cancel();
    queue(v, p);
  }
}

// ---------------------------------------------------------------- Dessin

/** Les pages se dessinent une à une, dans l'ordre où elles arrivent près de
 *  l'écran (le vieux processeur du serveur n'y est pour rien : tout se passe
 *  sur l'appareil, mais un téléphone n'aime pas non plus tout à la fois). */
function queue(v, p) {
  if (p.queued || p.drawn === v.scale) return;
  p.queued = true;
  v.queue = v.queue
    .then(() => draw(v, p))
    .catch((error) => v.closed || console.warn("PDF, page", p.n, error));
}

async function draw(v, p) {
  p.queued = false;
  if (v.closed || !p.wanted || p.drawn === v.scale) return;
  const scale = v.scale;
  p.page ??= await v.doc.getPage(p.n);
  const page = p.page;
  if (v.closed || !p.wanted) return;

  // Taille réelle de la page : on avait supposé celle de la première.
  if (!p.measured) {
    const one = page.getViewport({ scale: 1 });
    p.width = one.width;
    p.height = one.height;
    p.measured = true;
    p.div.style.setProperty("--user-unit", String(one.userUnit));
    size(v, p);
  }

  const viewport = page.getViewport({ scale });
  const ratio = Math.min(
    window.devicePixelRatio || 1,
    MAX_RATIO,
    Math.sqrt(MAX_PIXELS / (viewport.width * viewport.height)),
  );
  const canvas = document.createElement("canvas");
  canvas.className = "pdf-canvas";
  canvas.width = Math.floor(viewport.width * ratio);
  canvas.height = Math.floor(viewport.height * ratio);
  p.render = page.render({
    canvasContext: canvas.getContext("2d", { alpha: false }),
    viewport,
    transform: ratio === 1 ? null : [ratio, 0, 0, ratio, 0, 0],
    annotationMode: AnnotationMode.ENABLE,
  });
  try {
    await p.render.promise;
  } catch (error) {
    drop(canvas);
    if (error instanceof RenderingCancelledException) return;
    throw error;
  } finally {
    p.render = null;
  }

  // Couche de texte : du texte transparent posé exactement sur celui du
  // dessin, pour le sélectionner.
  const text = document.createElement("div");
  text.className = "pdf-text";
  try {
    await new TextLayer({
      textContentSource: page.streamTextContent({ includeMarkedContent: true }),
      container: text,
      viewport,
    }).render();
  } catch (error) {
    console.warn("PDF, texte de la page", p.n, error);
  }

  if (v.closed || !p.wanted || scale !== v.scale) {
    drop(canvas);
    return;
  }
  drop(p.canvas);
  p.canvas = canvas;
  p.div.replaceChildren(canvas, text);
  p.drawn = scale;
  if (!v.shown) {
    v.shown = true;
    say(v, "");
  }
}

/** Efface une page loin de l'écran. */
function release(p) {
  p.render?.cancel();
  drop(p.canvas);
  p.canvas = null;
  p.div.replaceChildren();
  p.drawn = 0;
  p.page?.cleanup();
}

/** Un canvas réduit à rien : Safari rend sa mémoire tout de suite. */
function drop(canvas) {
  if (!canvas) return;
  canvas.width = 0;
  canvas.height = 0;
}

// ------------------------------------------------------------- Fermeture

function close() {
  const v = viewer;
  if (!v) return;
  viewer = null;
  v.closed = true;
  for (const observer of v.observers) observer.disconnect();
  for (const p of v.pages) {
    p.render?.cancel();
    drop(p.canvas);
  }
  // Arrête le worker et libère le document.
  v.task.destroy();
}
