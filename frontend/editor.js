// Éditeur de notes : une seule vue, mise en forme à mesure qu'on écrit,
// comme Notion. « / » ouvre le menu des blocs (titres, listes, cases à
// cocher, tableau, citation, code, lien, image…), une sélection montre la
// barre de mise en forme, et les raccourcis Markdown marchent pendant la
// frappe (« # », « - », « [] », « > », « ``` », « --- », **gras**…).
// Source du bundle static/js/editor.js (voir scripts/build-assets.mjs).
//
// Construit sur ProseMirror (frontend/editor/) ; le Markdown reste le
// format stocké. À chaque modification, le document est réécrit en Markdown
// dans le <textarea> caché du formulaire : ce sont les attributs htmx du
// formulaire qui déclenchent la sauvegarde automatique (2 s). Ce fichier ne
// parle jamais au serveur lui-même, sauf pour la sauvegarde de dernière
// seconde (sendBeacon) et la recherche de notes à lier (GET /cmd/suggest).

import { EditorState, Selection, TextSelection } from "prosemirror-state";
import { EditorView } from "prosemirror-view";
import { Fragment, Slice } from "prosemirror-model";
import { keymap } from "prosemirror-keymap";
import { baseKeymap } from "prosemirror-commands";
import { history } from "prosemirror-history";
import { dropCursor } from "prosemirror-dropcursor";
import { gapCursor } from "prosemirror-gapcursor";
import { tableEditing, fixTables } from "prosemirror-tables";

import { schema, parseMarkdown, toMarkdown, sliceToMarkdown, safeHref } from "./editor/format.js";
import { turnInto, insertTable, insertRule, insertText, markdownShortcuts, editorKeys } from "./editor/commands.js";
import { placeholder, trailingParagraph, uploads, uploadFiles, pickFiles, nodeViews } from "./editor/plugins.js";
import { slashMenu, floatingTools, setSprite, attachSheet, detachSheet } from "./editor/ui.js";

/** Blocs du menu « / ». `hint` : le raccourci Markdown équivalent. */
function slashItems(editLink) {
  const block = (kind, attrs) => (view) => turnInto(view, kind, attrs);
  return [
    { group: "Blocs", label: "Texte", desc: "Un paragraphe simple", icon: "b-text", keys: ["paragraphe", "normal", "text"], run: block("paragraph") },
    { group: "Blocs", label: "Titre 1", desc: "Grand titre de partie", icon: "b-h1", hint: "#", keys: ["h1", "heading", "grand"], run: block("heading", { level: 1 }) },
    { group: "Blocs", label: "Titre 2", desc: "Titre moyen", icon: "b-h2", hint: "##", keys: ["h2", "heading", "sous-titre"], run: block("heading", { level: 2 }) },
    { group: "Blocs", label: "Titre 3", desc: "Petit titre", icon: "b-h3", hint: "###", keys: ["h3", "heading"], run: block("heading", { level: 3 }) },
    { group: "Blocs", label: "Liste à puces", desc: "Une liste simple", icon: "b-list", hint: "-", keys: ["puces", "bullet", "ul"], run: block("bullet") },
    { group: "Blocs", label: "Liste numérotée", desc: "Une liste avec des numéros", icon: "b-olist", hint: "1.", keys: ["numeros", "ordered", "ol"], run: block("ordered") },
    { group: "Blocs", label: "Liste de tâches", desc: "Des cases à cocher", icon: "tasks", hint: "[]", keys: ["taches", "todo", "to-do", "cases", "cocher", "checkbox"], run: block("task") },
    { group: "Blocs", label: "Tableau", desc: "Lignes et colonnes", icon: "b-table", keys: ["table", "grille", "colonnes"], run: (view) => insertTable(view) },
    { group: "Blocs", label: "Citation", desc: "Un passage mis en retrait", icon: "b-quote", hint: ">", keys: ["quote", "retrait"], run: block("quote") },
    { group: "Blocs", label: "Séparateur", desc: "Une ligne entre deux parties", icon: "b-hr", hint: "---", keys: ["ligne", "divider", "hr", "trait"], run: (view) => insertRule(view) },
    { group: "Blocs", label: "Bloc de code", desc: "Du code, en police fixe", icon: "code", hint: "```", keys: ["programme", "snippet"], run: block("code_block") },
    { group: "Insérer", label: "Lien", desc: "Une adresse, une note ou un fichier", icon: "link", keys: ["url", "adresse", "note", "fichier", "page"], run: (view) => editLink(view) },
    { group: "Insérer", label: "Image", desc: "Envoyer une photo dans l'espace", icon: "image", keys: ["photo", "img", "picture"], run: (view) => pickFiles(view, "image/*") },
    { group: "Insérer", label: "Fichier", desc: "Joindre un fichier de l'appareil", icon: "paperclip", keys: ["piece jointe", "pdf", "document", "file"], run: (view) => pickFiles(view) },
    {
      group: "Insérer",
      label: "Date du jour",
      desc: "Écrit la date d'aujourd'hui",
      icon: "calendar",
      keys: ["aujourdhui", "jour", "today"],
      run: (view) => insertText(view, new Date().toLocaleDateString("fr-FR", { day: "numeric", month: "long", year: "numeric" })),
    },
  ];
}

/** Texte collé : lu comme du Markdown (une liste collée devient une liste).
 *  Dans du code, ou avec Maj (collage sans mise en forme), du texte brut. */
function parsePastedText(text, $context, plain) {
  const clean = text.replace(/\r\n?/g, "\n");
  if ($context.parent.type.spec.code) return clean ? new Slice(Fragment.from(schema.text(clean)), 0, 0) : Slice.empty;
  if ($context.parent.type.spec.tableRole || plain) {
    const lines = clean.split("\n").filter((line) => line.trim());
    if ($context.parent.type.spec.tableRole) {
      const one = lines.join(" ");
      return one ? new Slice(Fragment.from(schema.text(one)), 0, 0) : Slice.empty;
    }
    const paragraphs = lines.map((line) => schema.nodes.paragraph.create(null, schema.text(line)));
    return paragraphs.length ? new Slice(Fragment.from(paragraphs), 1, 1) : Slice.empty;
  }
  const doc = parseMarkdown(clean);
  const content = doc.content;
  const open = (node) => (node && node.isTextblock ? 1 : 0);
  return new Slice(content, open(content.firstChild), open(content.lastChild));
}

function initEditor() {
  const host = document.getElementById("editor");
  const form = document.getElementById("note-form");
  const page = host?.closest(".editor-page");
  if (!host || !form || !page || host.pmView) return;

  const textarea = form.querySelector("textarea[name=body_md]");
  const title = form.querySelector("input[name=title]");
  const status = document.getElementById("save-status");
  setSprite(host.dataset.icons);
  attachSheet();

  let dirty = false;
  const markDirty = () => {
    dirty = true;
    if (status) {
      status.dataset.state = "dirty";
      status.textContent = "Modifications en cours…";
    }
  };

  // Sauvegarde immédiate (Ctrl+S) : déclenche le même envoi htmx que l'autosave.
  const flush = () => {
    if (window.htmx) window.htmx.trigger(form, "flush");
  };

  const toTitle = () => {
    if (!title) return false;
    title.focus();
    title.setSelectionRange(title.value.length, title.value.length);
    return true;
  };

  const tools = floatingTools(page);
  let doc = parseMarkdown(textarea.value);
  if (doc.lastChild?.type !== schema.nodes.paragraph) {
    doc = doc.copy(doc.content.addToEnd(schema.nodes.paragraph.create()));
  }
  let state = EditorState.create({
    doc,
    plugins: [
      slashMenu(slashItems(tools.editLink), page),
      markdownShortcuts(),
      tableEditing(),
      keymap(editorKeys({ save: flush, link: tools.editLink, toTitle })),
      keymap(baseKeymap),
      history(),
      dropCursor({ class: "ed-drop-cursor", color: false }),
      gapCursor(),
      placeholder(),
      trailingParagraph(),
      uploads(),
      tools.plugin,
    ],
  });
  // Tableau mal formé (lignes de longueurs différentes) : complété d'emblée,
  // sans compter comme une modification.
  const fixed = fixTables(state);
  if (fixed) state = state.apply(fixed.setMeta("addToHistory", false));

  // Le rendu fait par le serveur (affiché en attendant ce script) laisse la
  // place à l'éditeur.
  host.replaceChildren();
  const view = new EditorView(host, {
    state,
    nodeViews,
    attributes: {
      class: "prose note-body",
      "aria-label": "Contenu de la note",
      "aria-multiline": "true",
      role: "textbox",
      spellcheck: "true",
    },
    dispatchTransaction(tr) {
      view.updateState(view.state.apply(tr));
      if (!tr.docChanged) return;
      textarea.value = toMarkdown(view.state.doc);
      // Événement « input » : c'est lui que htmx écoute sur le formulaire.
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    },
    clipboardTextParser: parsePastedText,
    clipboardTextSerializer: sliceToMarkdown,
    handlePaste(view, event) {
      const data = event.clipboardData;
      if (data?.files?.length) return uploadFiles(view, data.files);
      const text = data?.getData("text/plain")?.trim();
      const href = text && /^(https?:\/\/|mailto:)\S+$/i.test(text) ? safeHref(text) : null;
      const { selection } = view.state;
      if (!href || selection.$from.parent.type.spec.code) return false;
      // Une adresse collée sur une sélection en fait un lien ; seule, elle
      // s'écrit comme un lien.
      const mark = schema.marks.link.create({ href });
      if (!selection.empty) {
        view.dispatch(view.state.tr.addMark(selection.from, selection.to, mark));
      } else {
        const tr = view.state.tr.replaceSelectionWith(schema.text(text, [mark]), false);
        view.dispatch(tr.removeStoredMark(schema.marks.link).scrollIntoView());
      }
      return true;
    },
    handleDrop(view, event, _slice, moved) {
      const files = event.dataTransfer?.files;
      if (moved || !files?.length) return false;
      const at = view.posAtCoords({ left: event.clientX, top: event.clientY });
      event.preventDefault();
      return uploadFiles(view, files, at ? at.pos : null);
    },
  });
  host.pmView = view;

  // Titre : Entrée ou flèche bas descend dans le texte, comme dans Notion.
  title?.addEventListener("keydown", (event) => {
    if (event.isComposing) return;
    if (event.key === "Enter" || (event.key === "ArrowDown" && title.selectionStart === title.value.length)) {
      event.preventDefault();
      view.focus();
      view.dispatch(view.state.tr.setSelection(Selection.atStart(view.state.doc)).scrollIntoView());
    } else if (event.key === "s" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      flush();
    }
  });

  // Clic sous la fin de la note : le curseur va à la fin.
  const scroller = host.closest("[data-note-scroll]");
  const onScrollerDown = (event) => {
    if (event.target !== scroller && !event.target.matches?.(".note-sheet")) return;
    if (event.clientY < view.dom.getBoundingClientRect().bottom) return;
    event.preventDefault();
    view.focus();
    view.dispatch(view.state.tr.setSelection(TextSelection.atEnd(view.state.doc)).scrollIntoView());
  };
  scroller?.addEventListener("mousedown", onScrollerDown);

  // Sauvegarde de dernière seconde quand on quitte la page avec des
  // modifications pas encore envoyées. sendBeacon ne sait pas poser d'en-tête,
  // le jeton CSRF part donc dans le corps (champ _csrf accepté par le serveur).
  function beacon() {
    if (!dirty) return;
    const data = new URLSearchParams(new FormData(form));
    data.set("_csrf", form.dataset.csrf || "");
    navigator.sendBeacon(form.dataset.saveUrl, data);
    dirty = false;
  }

  form.addEventListener("input", markDirty);

  form.addEventListener("htmx:beforeRequest", (e) => {
    if (e.target !== form || !status) return;
    status.dataset.state = "saving";
    status.textContent = "Enregistrement…";
  });
  form.addEventListener("htmx:afterRequest", (e) => {
    if (e.target !== form) return;
    if (e.detail.successful) {
      // Le serveur a répondu avec le fragment « Enregistré à HH:MM ».
      dirty = false;
      if (status) status.dataset.state = "saved";
    } else if (status) {
      status.dataset.state = "error";
      status.textContent = "Échec de l'enregistrement";
    }
  });

  const onHide = () => {
    if (document.visibilityState === "hidden") beacon();
  };
  const onLeave = (e) => {
    // Navigation htmx (hx-boost) vers une autre page pendant qu'on édite, ou
    // commande de la barre « / » (elle peut recharger la page ou en changer).
    const d = e.detail;
    if (d && (d.boosted || d.target === document.body || d.elt?.id === "cmd-form")) beacon();
  };
  document.addEventListener("visibilitychange", onHide);
  window.addEventListener("pagehide", beacon);
  document.body.addEventListener("htmx:beforeRequest", onLeave);

  // Nettoyage quand htmx remplace la page : sans ça les écouteurs globaux
  // s'accumuleraient à chaque note ouverte.
  const observer = new MutationObserver(() => {
    if (host.isConnected) return;
    observer.disconnect();
    document.removeEventListener("visibilitychange", onHide);
    window.removeEventListener("pagehide", beacon);
    document.body.removeEventListener("htmx:beforeRequest", onLeave);
    scroller?.removeEventListener("mousedown", onScrollerDown);
    view.destroy();
    detachSheet();
  });
  observer.observe(document.body, { childList: true, subtree: true });

  // Note vide : on commence par le titre, sinon directement dans le texte.
  if (title && !title.value) title.focus();
  else view.focus();
}

initEditor();
