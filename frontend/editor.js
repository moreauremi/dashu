// Éditeur de notes : CodeMirror 6 en mode Markdown.
// Source du bundle static/js/editor.js (voir scripts/build-assets.mjs).
//
// L'éditeur vit dans un shadow root : CodeMirror y pose ses styles avec
// adoptedStyleSheets, ce qui évite toute balise <style> injectée et permet de
// garder une Content-Security-Policy sans 'unsafe-inline'.
//
// Le contenu est recopié dans un <textarea> caché du formulaire ; ce sont les
// attributs htmx du formulaire qui déclenchent l'aperçu (300 ms) et la
// sauvegarde automatique (2 s). Ce fichier ne parle jamais au serveur
// lui-même, sauf pour la sauvegarde de dernière seconde (sendBeacon).

import { EditorState } from "@codemirror/state";
import {
  EditorView,
  keymap,
  drawSelection,
  highlightActiveLine,
  placeholder,
} from "@codemirror/view";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import {
  HighlightStyle,
  syntaxHighlighting,
  indentOnInput,
} from "@codemirror/language";
import { tags as t } from "@lezer/highlight";

// Les couleurs viennent des variables CSS de la page : elles traversent la
// frontière du shadow root et suivent donc le thème clair/sombre.
const theme = EditorView.theme({
  "&": {
    height: "100%",
    color: "var(--text)",
    backgroundColor: "transparent",
    fontSize: "14px",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--font-mono)",
    lineHeight: "1.65",
    overflow: "auto",
  },
  ".cm-content": {
    padding: "20px 0 40vh",
    caretColor: "var(--accent)",
    maxWidth: "78ch",
    margin: "0 auto",
  },
  ".cm-line": { padding: "0 24px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
  ".cm-selectionBackground": { backgroundColor: "var(--selection)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
    backgroundColor: "var(--selection)",
  },
  ".cm-activeLine": { backgroundColor: "var(--line-active)" },
  ".cm-placeholder": { color: "var(--text-3)" },
});

const highlight = HighlightStyle.define([
  { tag: t.heading1, fontWeight: "700", fontSize: "1.35em", color: "var(--text)" },
  { tag: t.heading2, fontWeight: "700", fontSize: "1.18em", color: "var(--text)" },
  { tag: [t.heading3, t.heading4, t.heading5, t.heading6], fontWeight: "700", color: "var(--text)" },
  { tag: t.strong, fontWeight: "700" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through", color: "var(--text-3)" },
  { tag: [t.link, t.url], color: "var(--accent-text)" },
  { tag: t.monospace, color: "var(--code-text)" },
  { tag: t.quote, color: "var(--text-2)", fontStyle: "italic" },
  { tag: [t.processingInstruction, t.meta, t.contentSeparator], color: "var(--text-3)" },
  { tag: t.list, color: "var(--text)" },
  { tag: t.atom, color: "var(--accent-text)" },
]);

function initEditor() {
  const host = document.getElementById("editor");
  const form = document.getElementById("note-form");
  if (!host || !form || host.shadowRoot) return;

  const textarea = form.querySelector("textarea[name=body_md]");
  const status = document.getElementById("save-status");
  const root = host.attachShadow({ mode: "open" });

  let dirty = false;

  const markDirty = () => {
    dirty = true;
    if (status) {
      status.dataset.state = "dirty";
      status.textContent = "Modifications en cours…";
    }
  };

  const view = new EditorView({
    root,
    parent: root,
    state: EditorState.create({
      doc: textarea.value,
      extensions: [
        history(),
        drawSelection(),
        highlightActiveLine(),
        indentOnInput(),
        EditorView.lineWrapping,
        placeholder("Écris en Markdown…"),
        markdown({ base: markdownLanguage, completeHTMLTags: false }),
        syntaxHighlighting(highlight),
        theme,
        keymap.of([
          {
            key: "Mod-s",
            run: () => {
              flush();
              return true;
            },
          },
          indentWithTab,
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.contentAttributes.of({ "aria-label": "Contenu de la note en Markdown" }),
        // Image collée ou fichier lâché : rangé dans l'espace de la note
        // (static/js/files.js), puis inséré en Markdown.
        EditorView.domEventHandlers({
          paste: (event, editor) => attach(event.clipboardData?.files, editor, null),
          drop: (event, editor) => {
            const at = editor.posAtCoords({ x: event.clientX, y: event.clientY });
            if (!attach(event.dataTransfer?.files, editor, at)) return false;
            event.preventDefault();
            return true;
          },
        }),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;
          textarea.value = update.state.doc.toString();
          // Événement « input » : c'est lui que htmx écoute sur le formulaire.
          textarea.dispatchEvent(new Event("input", { bubbles: true }));
        }),
      ],
    }),
  });

  /** Envoie les fichiers collés ou lâchés et insère leur lien à leur place :
   *  une image en `![nom](/files/id/raw)`, le reste en `[nom](/files/id)`.
   *  Un texte provisoire marque l'endroit pendant l'envoi. */
  let pending = 0;
  function attach(files, editor, at) {
    if (!files || files.length === 0 || !window.dashuUpload) return false;
    for (const file of files) {
      pending += 1;
      const mark = `![Envoi de ${file.name || "l'image"}…](#envoi-${pending})`;
      const pos = at ?? editor.state.selection.main.head;
      editor.dispatch({ changes: { from: pos, insert: `${mark}\n` } });
      const pasted = !file.name || file.name === "image.png";
      const name = pasted ? `collage-${stamp()}.${(file.type.split("/")[1] || "png").replace("jpeg", "jpg")}` : file.name;
      const replace = (text) => {
        const doc = editor.state.doc.toString();
        const from = doc.indexOf(mark);
        if (from < 0) return;
        editor.dispatch({ changes: { from, to: from + mark.length, insert: text } });
      };
      window
        .dashuUpload(file, name)
        .then((id) => {
          const image = /^image\/(png|jpeg|gif|webp)$/.test(file.type);
          replace(image ? `![${name}](/files/${id}/raw)` : `[${name}](/files/${id})`);
        })
        .catch(() => replace(""));
    }
    return true;
  }

  /** « 2026-11-13-140512 » : nom d'une image collée. */
  function stamp() {
    const d = new Date();
    const p = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
  }

  // Sauvegarde immédiate (Ctrl+S) : déclenche le même envoi htmx que l'autosave.
  function flush() {
    if (window.htmx) window.htmx.trigger(form, "flush");
  }

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
    // Navigation htmx (hx-boost) vers une autre page pendant qu'on édite.
    if (e.detail && e.detail.boosted) beacon();
  };
  document.addEventListener("visibilitychange", onHide);
  window.addEventListener("pagehide", beacon);
  document.body.addEventListener("htmx:beforeRequest", onLeave);

  // Onglets Écrire / Aperçu (petits écrans).
  const panes = document.getElementById("note-panes");
  document.querySelectorAll("[data-pane-tab]").forEach((btn) => {
    btn.addEventListener("click", () => {
      panes.dataset.show = btn.dataset.paneTab;
      document
        .querySelectorAll("[data-pane-tab]")
        .forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
      if (btn.dataset.paneTab === "write") view.focus();
    });
  });

  // Nettoyage quand htmx remplace la page : sans ça les écouteurs globaux
  // s'accumuleraient à chaque note ouverte.
  const observer = new MutationObserver(() => {
    if (host.isConnected) return;
    observer.disconnect();
    document.removeEventListener("visibilitychange", onHide);
    window.removeEventListener("pagehide", beacon);
    document.body.removeEventListener("htmx:beforeRequest", onLeave);
    view.destroy();
  });
  observer.observe(document.body, { childList: true, subtree: true });

  // Note vide : on commence par le titre, sinon directement dans le texte.
  const title = form.querySelector("input[name=title]");
  if (title && !title.value) title.focus();
  else view.focus();
}

initEditor();
