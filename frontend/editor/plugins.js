// Petits plugins de l'éditeur : texte d'aide sur la ligne vide, paragraphe
// toujours présent en fin de note, fichiers collés ou lâchés, et l'affichage
// des cases à cocher et des tableaux.

import { Plugin, PluginKey } from "prosemirror-state";
import { Decoration, DecorationSet } from "prosemirror-view";
import { schema } from "./format.js";

const N = schema.nodes;

// --------------------------------------------------- Texte d'aide

const HINTS = {
  heading: (node) => `Titre ${node.attrs.level}`,
  code_block: () => "Code",
};

/** « Tape / pour choisir un bloc » sur la ligne vide du curseur (le CSS ne
 *  l'affiche que si l'éditeur a le focus), et une invite sur une note vide. */
export function placeholder() {
  return new Plugin({
    props: {
      decorations(state) {
        const { doc, selection } = state;
        const only = doc.childCount === 1 ? doc.firstChild : null;
        if (only && only.type === N.paragraph && only.content.size === 0) {
          return DecorationSet.create(doc, [
            Decoration.node(0, only.nodeSize, {
              class: "is-empty is-doc-empty",
              "data-placeholder": "Écris, ou tape « / » pour choisir un bloc…",
            }),
          ]);
        }
        const { $from, empty } = selection;
        const parent = $from.parent;
        if (!empty || !parent.isTextblock || parent.content.size > 0) return null;
        if (parent.type.spec.tableRole) return null;
        const inItem = $from.depth > 1 && $from.node(-1).type === N.list_item;
        const hint = HINTS[parent.type.name]?.(parent) ?? (inItem ? "Liste" : "Tape « / » pour choisir un bloc");
        const pos = $from.before();
        return DecorationSet.create(doc, [
          Decoration.node(pos, pos + parent.nodeSize, { class: "is-empty", "data-placeholder": hint }),
        ]);
      },
    },
  });
}

/** Une note se termine toujours par un paragraphe : après un tableau ou un
 *  bloc de code, on peut cliquer dessous et continuer à écrire. */
export function trailingParagraph() {
  return new Plugin({
    appendTransaction(_trs, _old, state) {
      const last = state.doc.lastChild;
      if (last && last.type === N.paragraph) return null;
      return state.tr.insert(state.doc.content.size, N.paragraph.create());
    },
  });
}

// ----------------------------------------------------- Fichiers

const uploadKey = new PluginKey("uploads");

/** Repères « Envoi de photo.jpg… » pendant l'envoi d'un fichier. */
export function uploads() {
  return new Plugin({
    key: uploadKey,
    state: {
      init: () => DecorationSet.empty,
      apply(tr, set) {
        set = set.map(tr.mapping, tr.doc);
        const meta = tr.getMeta(uploadKey);
        if (meta?.add) {
          const label = document.createElement("span");
          label.className = "ed-uploading";
          label.textContent = `Envoi de ${meta.add.name}…`;
          set = set.add(tr.doc, [Decoration.widget(meta.add.pos, label, { id: meta.add.id })]);
        } else if (meta?.remove) {
          set = set.remove(set.find(null, null, (spec) => spec.id === meta.remove));
        }
        return set;
      },
    },
    props: {
      decorations: (state) => uploadKey.getState(state),
    },
  });
}

function placeholderPos(state, id) {
  const found = uploadKey.getState(state).find(null, null, (spec) => spec.id === id);
  return found.length ? found[0].from : null;
}

/** « 2026-11-13-140512 » : nom d'une image collée. */
function stamp() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

/** Envoie des fichiers dans l'espace de la note (static/js/files.js) et les
 *  insère à leur place : une image s'affiche, le reste devient un lien. */
export function uploadFiles(view, files, pos = null) {
  if (!files || files.length === 0 || !window.dashuUpload) return false;
  let at = pos;
  for (const file of files) {
    const pasted = !file.name || file.name === "image.png";
    const ext = (file.type.split("/")[1] || "png").replace("jpeg", "jpg");
    const name = pasted ? `collage-${stamp()}.${ext}` : file.name;
    const id = {};
    const tr = view.state.tr;
    if (at === null) {
      if (!tr.selection.empty) tr.deleteSelection();
      at = tr.selection.from;
    }
    view.dispatch(tr.setMeta(uploadKey, { add: { id, pos: at, name } }));

    const done = (node) => {
      if (view.isDestroyed) return;
      const where = placeholderPos(view.state, id);
      const tr2 = view.state.tr.setMeta(uploadKey, { remove: id });
      if (where !== null && node) tr2.insert(where, node);
      view.dispatch(tr2);
    };
    window.dashuUpload(file, name).then(
      (fileId) => {
        const image = /^image\/(png|jpeg|gif|webp)$/.test(file.type);
        done(
          image
            ? N.image.create({ src: `/files/${fileId}/raw`, alt: name })
            : schema.text(name, [schema.marks.link.create({ href: `/files/${fileId}` })]),
        );
      },
      () => done(null),
    );
  }
  return true;
}

/** Ouvre le sélecteur de fichiers (menu « / » : Image, Fichier). */
export function pickFiles(view, accept) {
  const input = document.createElement("input");
  input.type = "file";
  input.multiple = true;
  if (accept) input.accept = accept;
  input.hidden = true;
  const at = view.state.selection.from;
  input.addEventListener("change", () => {
    uploadFiles(view, input.files, Math.min(at, view.state.doc.content.size));
    input.remove();
    view.focus();
  });
  // Dans la page : Safari n'ouvre pas le sélecteur d'un champ détaché.
  view.dom.parentNode.append(input);
  input.click();
}

// ------------------------------------------------------- Affichage

/** Élément de liste : une vraie case à cocher pour les tâches. */
class ListItemView {
  constructor(node, view, getPos) {
    this.node = node;
    this.dom = document.createElement("li");
    this.contentDOM = document.createElement("div");
    this.contentDOM.className = "li-body";
    if (node.attrs.checked !== null) {
      this.box = document.createElement("input");
      this.box.type = "checkbox";
      this.box.className = "li-check";
      this.box.setAttribute("aria-label", "Fait");
      this.box.contentEditable = "false";
      this.box.addEventListener("mousedown", (event) => event.preventDefault());
      this.box.addEventListener("click", (event) => {
        event.preventDefault();
        const pos = getPos();
        if (pos == null || !view.editable) return;
        const checked = !this.node.attrs.checked;
        view.dispatch(view.state.tr.setNodeMarkup(pos, null, { ...this.node.attrs, checked }));
      });
      this.dom.append(this.box);
    }
    this.dom.append(this.contentDOM);
    this.sync();
  }

  sync() {
    if (this.node.attrs.checked === null) {
      this.dom.removeAttribute("data-checked");
    } else {
      this.dom.setAttribute("data-checked", String(this.node.attrs.checked));
      this.box.checked = this.node.attrs.checked;
    }
  }

  update(node) {
    if (node.type !== this.node.type) return false;
    // Puce devenue case (ou l'inverse) : on refait l'élément.
    if ((node.attrs.checked === null) !== (this.node.attrs.checked === null)) return false;
    this.node = node;
    this.sync();
    return true;
  }

  stopEvent(event) {
    return event.target === this.box;
  }

  ignoreMutation(mutation) {
    return mutation.target === this.box || (mutation.type === "attributes" && mutation.target === this.dom);
  }
}

/** Tableau : défile de côté s'il est plus large que la note. */
class TableView {
  constructor() {
    this.dom = document.createElement("div");
    this.dom.className = "table-wrap";
    const table = document.createElement("table");
    this.contentDOM = document.createElement("tbody");
    table.append(this.contentDOM);
    this.dom.append(table);
  }

  update(node) {
    return node.type === N.table;
  }

  ignoreMutation(mutation) {
    return mutation.type === "attributes" && (mutation.target === this.dom || mutation.target === this.contentDOM.parentNode);
  }
}

export const nodeViews = {
  list_item: (node, view, getPos) => new ListItemView(node, view, getPos),
  table: () => new TableView(),
};
