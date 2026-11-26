// Produit les fichiers statiques tiers de dashu dans static/ :
//   - static/js/htmx.js        htmx minifié, copié tel quel depuis npm
//   - static/js/editor.js      ProseMirror + notre code d'éditeur, bundle esbuild
//   - static/js/pdf.js, pdf-worker.js, pdfjs/   lecteur de PDF (pdf.js de Mozilla)
//   - static/icons.svg         sprite SVG unique, icônes Lucide (licence ISC)
//   - static/fonts/*.woff2     Rethink Sans et JetBrains Mono (licence OFL), sous-ensemble latin
//
// Le résultat est versionné dans Git : `cargo build` et l'image Docker n'ont
// donc pas besoin de Node. On ne relance ce script que pour mettre à jour une
// de ces briques :  npm install && npm run build

import { build } from "esbuild";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const nm = (...p) => join(root, "node_modules", ...p);
const out = (...p) => join(root, "static", ...p);

await mkdir(out("js"), { recursive: true });
await mkdir(out("fonts"), { recursive: true });

// --- htmx -------------------------------------------------------------
await copyFile(nm("htmx.org", "dist", "htmx.min.js"), out("js", "htmx.js"));

// --- Éditeur (ProseMirror) --------------------------------------------
await build({
  entryPoints: [join(root, "frontend", "editor.js")],
  outfile: out("js", "editor.js"),
  bundle: true,
  minify: true,
  format: "iife",
  target: ["es2020"],
  legalComments: "none",
  logLevel: "info",
  // Voir les deux fichiers : les entités HTML rares et la détection des
  // adresses nues ne servent pas à l'éditeur.
  alias: {
    entities: join(root, "frontend", "stub-entities.js"),
    "linkify-it": join(root, "frontend", "stub-linkify.js"),
  },
});

// --- Lecteur de PDF (pdf.js de Mozilla) -------------------------------
// Version « legacy » : la version moderne demande les tout derniers
// navigateurs (Map.getOrInsertComputed, Promise.try…), la legacy apporte ce
// qui manque. Chargé seulement sur la page d'un PDF (app.js, `pdfViewer`).
//   - static/js/pdf.js             pdf.js + notre lecteur (frontend/pdf.js)
//   - static/js/pdf-worker.js      le worker de pdf.js, qui lit le PDF hors du fil principal
//   - static/js/pdfjs/*.js         décodeurs d'images JBIG2 et JPEG 2000 (PDF scannés),
//                                  en JavaScript : la CSP refuse WebAssembly. pdf.js
//                                  les demande sous leur nom exact, sans empreinte.
await build({
  entryPoints: [join(root, "frontend", "pdf.js")],
  outfile: out("js", "pdf.js"),
  bundle: true,
  minify: true,
  format: "esm",
  target: ["es2022"],
  legalComments: "none",
  banner: { js: "/* pdf.js (Mozilla, https://github.com/mozilla/pdf.js), licence Apache-2.0. */" },
  logLevel: "info",
});
await copyFile(nm("pdfjs-dist", "legacy", "build", "pdf.worker.min.mjs"), out("js", "pdf-worker.js"));
await mkdir(out("js", "pdfjs"), { recursive: true });
for (const decoder of ["jbig2_nowasm_fallback.js", "openjpeg_nowasm_fallback.js"]) {
  await copyFile(nm("pdfjs-dist", "wasm", decoder), out("js", "pdfjs", decoder));
}

// --- Polices ----------------------------------------------------------
await copyFile(
  nm("@fontsource-variable", "rethink-sans", "files", "rethink-sans-latin-wght-normal.woff2"),
  out("fonts", "rethink-sans.woff2"),
);
await copyFile(
  nm("@fontsource-variable", "jetbrains-mono", "files", "jetbrains-mono-latin-wght-normal.woff2"),
  out("fonts", "jetbrains-mono.woff2"),
);

// --- Sprite d'icônes --------------------------------------------------
// Nom dans dashu -> nom du fichier Lucide. On n'embarque que ce qui sert.
const icons = {
  dashboard: "layout-dashboard",
  notes: "notebook-text",
  calendar: "calendar-days",
  settings: "settings",
  search: "search",
  plus: "plus",
  x: "x",
  check: "check",
  trash: "trash-2",
  pin: "pin",
  "pin-off": "pin-off",
  "chevron-left": "chevron-left",
  "chevron-right": "chevron-right",
  "arrow-left": "arrow-left",
  refresh: "refresh-cw",
  menu: "menu",
  sun: "sun",
  moon: "moon",
  monitor: "monitor",
  logout: "log-out",
  tag: "tag",
  "map-pin": "map-pin",
  clock: "clock",
  move: "arrow-right-left",
  eye: "eye",
  pencil: "pencil",
  alert: "triangle-alert",
  enter: "corner-down-left",
  key: "key-round",
  file: "file-text",
  files: "files",
  upload: "upload",
  download: "download",
  shrink: "shrink",
  "file-image": "file-image",
  "file-video": "file-play",
  "file-audio": "file-music",
  "file-archive": "file-archive",
  "file-sheet": "file-spreadsheet",
  "file-code": "file-code",
  "file-blank": "file",
  tasks: "list-todo",
  link: "link",
  info: "info",
  user: "user",
  // Icônes au choix d'un espace (voir ICONS dans src/spaces/mod.rs).
  home: "house",
  school: "graduation-cap",
  work: "briefcase-business",
  building: "building-2",
  book: "book-open",
  code: "code-xml",
  users: "users",
  heart: "heart",
  dumbbell: "dumbbell",
  music: "music",
  plane: "plane",
  folder: "folder",
  // Barre latérale ancrée ou détachée, photo de profil.
  "panel-pin": "panel-left",
  "panel-float": "panel-left-close",
  camera: "camera",
  // Outils : onglet, conversions, suivi ; espaces archivés.
  tools: "wrench",
  convert: "file-output",
  replace: "replace",
  loader: "loader-circle",
  archive: "archive",
  unarchive: "archive-restore",
  // Barre de commandes « / ».
  commands: "square-slash",
  // Éditeur de notes : menu « / », barre de mise en forme, tableaux.
  "b-text": "type",
  "b-h1": "heading-1",
  "b-h2": "heading-2",
  "b-h3": "heading-3",
  "b-list": "list",
  "b-olist": "list-ordered",
  "b-table": "table",
  "b-quote": "text-quote",
  "b-hr": "minus",
  image: "image",
  paperclip: "paperclip",
  bold: "bold",
  italic: "italic",
  strike: "strikethrough",
  "code-inline": "code",
  unlink: "unlink",
  "row-add": "between-horizontal-end",
  "col-add": "between-vertical-end",
  "row-del": "table-rows-split",
  "col-del": "table-columns-split",
  // Menu « … » des lignes de l'onglet Fichiers.
  more: "ellipsis",
  // Lecteur de PDF : zoom.
  "zoom-in": "zoom-in",
  "zoom-out": "zoom-out",
};

let sprite =
  '<svg xmlns="http://www.w3.org/2000/svg"><!-- Icônes Lucide (https://lucide.dev), licence ISC. Fichier généré par scripts/build-assets.mjs. -->';
for (const [name, file] of Object.entries(icons)) {
  const svg = await readFile(nm("lucide-static", "icons", `${file}.svg`), "utf8");
  const inner = svg
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<svg[\s\S]*?>/, "")
    .replace("</svg>", "")
    .replace(/\s+/g, " ")
    .replace(/ \/>/g, "/>")
    .replace(/> </g, "><")
    .trim();
  sprite += `<symbol id="${name}" viewBox="0 0 24 24">${inner}</symbol>`;
}
// Icônes propres à dashu, hors Lucide, dessinées comme elles : grille de 24,
// traits ronds sans remplissage (le style vient de .i dans app.css).
const own = {
  // Logo de 1Life, outil réservé (src/onelife) : les deux traits du logo,
  // suivis en leur milieu.
  "1life": {
    viewBox: "0 0 24 24",
    inner:
      '<path d="M6.77 6.04 11.31 21"/>' +
      '<path d="M6.77 6.04C6.98 10.09 5.3 13.44 3.15 15.11"/>' +
      '<path d="M11.7 3 15.05 13.2C15.6 14.87 16.56 15.66 18.28 15.66H20.84"/>',
  },
};
for (const [name, { viewBox, inner }] of Object.entries(own)) {
  sprite += `<symbol id="${name}" viewBox="${viewBox}">${inner}</symbol>`;
}
sprite += "</svg>\n";
await writeFile(out("icons.svg"), sprite);

console.log(`sprite : ${Object.keys(icons).length + Object.keys(own).length} icônes`);
