// Menus flottants de l'éditeur :
//   - le menu « / » (choisir ou transformer un bloc) ;
//   - la barre de mise en forme sur une sélection (gras, italique, lien…) ;
//   - la fiche d'un lien (ouvrir, modifier, retirer) et sa saisie, qui
//     cherche aussi parmi les notes et les fichiers ;
//   - la barre d'un tableau (ajouter ou retirer une ligne, une colonne).
//
// Chacun est un élément `popover` (au-dessus de tout, jamais coupé). Sa
// position passe par une feuille de style construite : la CSP refuse tout
// style en ligne (voir CLAUDE.md, « CSP stricte »).

import { Plugin, PluginKey, TextSelection } from "prosemirror-state";
import { toggleMark } from "prosemirror-commands";
import {
  isInTable,
  selectionCell,
  addRowAfter,
  addColumnAfter,
  deleteRow,
  deleteColumn,
  deleteTable,
} from "prosemirror-tables";
import { schema, safeHref } from "./format.js";

const M = schema.marks;
const SVG = "http://www.w3.org/2000/svg";

// ------------------------------------------------------------- Outils

let sprite = "";
/** Adresse du sprite d'icônes (avec son empreinte), donnée par la page. */
export function setSprite(url) {
  sprite = url || "";
}

function icon(name) {
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "i");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(SVG, "use");
  use.setAttribute("href", `${sprite}#${name}`);
  svg.append(use);
  return svg;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === false || value == null) continue;
    if (key === "text") node.textContent = value;
    else node.setAttribute(key, value === true ? "" : value);
  }
  node.append(...children.filter(Boolean));
  return node;
}

/** Sans accents ni majuscules : « Titre » trouve « titre », « tache » « tâche ». */
const fold = (text) => text.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

/** Facteur de zoom de l'app (taille de l'interface, voir app.js). */
const zoom = () =>
  parseFloat(getComputedStyle(document.getElementById("app") || document.body).getPropertyValue("--zoom")) || 1;

// Une seule feuille pour toutes les positions ; retirée avec l'éditeur.
const sheet = new CSSStyleSheet();
const rules = new Map();
let floaters = 0;

export function attachSheet() {
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
}

export function detachSheet() {
  document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== sheet);
  rules.clear();
}

function setRule(id, css) {
  rules.set(id, css);
  sheet.replaceSync([...rules.values()].join("\n"));
}

/** Élément flottant, placé près d'un rectangle de l'écran. */
class Floater {
  constructor(host, className, label) {
    this.el = el("div", {
      id: `ed-pop-${++floaters}`,
      class: `ed-pop ${className}`,
      popover: "manual",
      "aria-label": label,
    });
    host.append(this.el);
  }

  get open() {
    return this.el.matches(":popover-open");
  }

  /** `rect` : { left, top, bottom } en pixels de l'écran (et `right` pour
   *  aligner le menu à droite). */
  place(rect, { above = false, gap = 6, alignRight = false } = {}) {
    if (!this.open) this.el.showPopover();
    const z = zoom();
    const box = this.el.getBoundingClientRect();
    const vv = window.visualViewport;
    const viewTop = vv ? vv.offsetTop : 0;
    const viewBottom = vv ? vv.offsetTop + vv.height : innerHeight;
    const start = alignRight ? rect.right - box.width : rect.left;
    const left = Math.max(8, Math.min(start, innerWidth - box.width - 8));
    const over = rect.top - gap - box.height;
    const under = rect.bottom + gap;
    let top = above ? over : under;
    if (above && over < viewTop + 8) top = under;
    if (!above && under + box.height > viewBottom - 8 && over > viewTop + 8) top = over;
    setRule(this.el.id, `#${this.el.id} { left: ${left / z}px; top: ${top / z}px; }`);
  }

  hide() {
    if (this.open) this.el.hidePopover();
  }
}

/** Les clics dans un menu ne doivent pas faire perdre le curseur au texte. */
function keepFocus(node) {
  node.addEventListener("mousedown", (event) => {
    if (!(event.target instanceof HTMLInputElement)) event.preventDefault();
  });
}

// ---------------------------------------------------------- Menu « / »

export const slashKey = new PluginKey("slash");

/** Filtre les commandes du menu : un début de mot suffit, sans accents.
 *  Le nom du bloc passe avant ses mots-clés : « co » donne « Bloc de code »
 *  avant « Liste de tâches » (mot-clé « cocher »). */
function filter(items, query) {
  const q = fold(query.trim());
  if (!q) return items;
  const re = new RegExp(`(^|[\\s|/-])${q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);
  const score = (item) => {
    const label = fold(item.label);
    if (label.startsWith(q)) return 0;
    if (re.test(label)) return 1;
    return re.test(fold((item.keys || []).join(" | "))) ? 2 : -1;
  };
  return items
    .map((item, index) => ({ item, index, score: score(item) }))
    .filter((entry) => entry.score >= 0)
    .sort((a, b) => a.score - b.score || a.index - b.index)
    .map((entry) => entry.item);
}

/** Plugin du menu « / ». `items` : { label, desc, icon, hint, keys, group, run(view) }. */
export function slashMenu(items, host) {
  let menu = null;
  return new Plugin({
    key: slashKey,
    state: {
      init: () => ({ active: false }),
      apply(tr, prev, _old, state) {
        const meta = tr.getMeta(slashKey);
        if (meta?.close) return { active: false };
        if (meta?.open == null && !prev.active) return prev;
        const from = meta?.open ?? tr.mapping.map(prev.from);
        const { selection } = state;
        if (!selection.empty || selection.from <= from) return { active: false };
        if (state.doc.textBetween(from, from + 1) !== "/") return { active: false };
        if (state.doc.resolve(from).parent !== selection.$from.parent) return { active: false };
        const query = state.doc.textBetween(from + 1, selection.from);
        // Une espace juste après « / », ou plus rien ne correspond : on
        // écrivait simplement une barre oblique.
        if (/^\s/.test(query) || query.length > 30) return { active: false };
        if (query.length > 2 && !filter(items, query).length) return { active: false };
        return { active: true, from, query };
      },
    },
    props: {
      handleTextInput(view, from, to, text) {
        if (text !== "/") return false;
        const $from = view.state.doc.resolve(from);
        const type = $from.parent.type;
        if (type !== schema.nodes.paragraph && type !== schema.nodes.heading) return false;
        const before = $from.parent.textBetween(Math.max(0, $from.parentOffset - 1), $from.parentOffset);
        if (before && !/\s/.test(before)) return false;
        view.dispatch(view.state.tr.insertText("/", from, to).setMeta(slashKey, { open: from }));
        return true;
      },
      handleKeyDown(view, event) {
        if (!slashKey.getState(view.state).active) return false;
        return menu ? menu.key(event) : false;
      },
      handleDOMEvents: {
        blur(view) {
          if (slashKey.getState(view.state).active) view.dispatch(view.state.tr.setMeta(slashKey, { close: true }));
          return false;
        },
      },
    },
    view(view) {
      menu = new SlashView(view, items, host);
      return menu;
    },
  });
}

class SlashView {
  constructor(view, items, host) {
    this.view = view;
    this.items = items;
    this.pop = new Floater(host, "slash-menu", "Blocs");
    this.pop.el.setAttribute("role", "listbox");
    this.matches = [];
    this.index = 0;
    this.query = null;
    keepFocus(this.pop.el);
    this.pop.el.addEventListener("click", (event) => {
      const item = event.target.closest("[data-index]");
      if (item) this.choose(+item.dataset.index);
    });
    this.pop.el.addEventListener("mousemove", (event) => {
      const item = event.target.closest("[data-index]");
      if (item && +item.dataset.index !== this.index) this.select(+item.dataset.index, false);
    });
  }

  update(view) {
    this.view = view;
    const state = slashKey.getState(view.state);
    if (!state.active) {
      this.pop.hide();
      this.query = null;
      return;
    }
    if (state.query !== this.query) {
      this.query = state.query;
      this.matches = filter(this.items, state.query);
      this.index = 0;
      this.render();
    }
    const at = view.coordsAtPos(state.from);
    this.pop.place(at);
  }

  render() {
    const list = this.pop.el;
    list.replaceChildren();
    if (!this.matches.length) {
      list.append(el("p", { class: "slash-empty", text: "Aucun bloc ne correspond" }));
      return;
    }
    let group = null;
    this.matches.forEach((item, index) => {
      if (!this.query && item.group !== group) {
        group = item.group;
        list.append(el("p", { class: "slash-group", text: group }));
      }
      list.append(
        el(
          "button",
          {
            type: "button",
            class: "slash-item",
            role: "option",
            "data-index": index,
            "aria-selected": String(index === this.index),
          },
          el("span", { class: "slash-icon" }, icon(item.icon)),
          el(
            "span",
            { class: "slash-text" },
            el("span", { class: "slash-label", text: item.label }),
            el("span", { class: "slash-desc", text: item.desc }),
          ),
          item.hint ? el("kbd", { class: "slash-hint", text: item.hint }) : null,
        ),
      );
    });
  }

  select(index, scroll = true) {
    const buttons = this.pop.el.querySelectorAll("[data-index]");
    if (!buttons.length) return;
    this.index = (index + buttons.length) % buttons.length;
    buttons.forEach((b) => b.setAttribute("aria-selected", String(+b.dataset.index === this.index)));
    if (scroll) buttons[this.index].scrollIntoView({ block: "nearest" });
  }

  key(event) {
    if (event.isComposing) return false;
    switch (event.key) {
      case "ArrowDown":
        this.select(this.index + 1);
        return true;
      case "ArrowUp":
        this.select(this.index - 1);
        return true;
      case "Enter":
      case "Tab":
        if (!this.matches.length) return false;
        this.choose(this.index);
        return true;
      case "Escape":
        this.close();
        return true;
      default:
        return false;
    }
  }

  close() {
    this.view.dispatch(this.view.state.tr.setMeta(slashKey, { close: true }));
  }

  choose(index) {
    const item = this.matches[index];
    const state = slashKey.getState(this.view.state);
    if (!item || !state.active) return;
    // On efface « /texte tapé », puis la commande s'applique au bloc.
    const tr = this.view.state.tr.delete(state.from, this.view.state.selection.from);
    this.view.dispatch(tr.setMeta(slashKey, { close: true }));
    this.view.focus();
    item.run(this.view);
  }

  destroy() {
    this.pop.el.remove();
  }
}

// ----------------------------------------------------------- Liens

/** Lien sous le curseur (sélection vide) : sa marque et son étendue. */
function linkAt(state) {
  const { $from, empty } = state.selection;
  if (!empty) return null;
  const has = (node) => node && M.link.isInSet(node.marks);
  const mark = has($from.nodeAfter) || has($from.nodeBefore);
  if (!mark) return null;
  // Suites de morceaux qui portent ce lien : on garde celle du curseur.
  const start = $from.start();
  let from = null;
  let to = null;
  let found = null;
  $from.parent.forEach((child, offset) => {
    if (!mark.isInSet(child.marks)) return;
    const a = start + offset;
    if (to !== a) from = a;
    to = a + child.nodeSize;
    if ($from.pos >= from && $from.pos <= to) found = { from, to, mark };
  });
  return found;
}

/** « exemple.fr » -> « https://exemple.fr », « a@b.fr » -> « mailto:a@b.fr ». */
export function normalizeUrl(text) {
  const value = text.trim();
  if (!value) return null;
  if (/^[^\s@/]+@[^\s@/]+\.[a-z]{2,}$/i.test(value)) return `mailto:${value}`;
  if (/^www\./i.test(value) || /^[\w-]+(\.[\w-]+)*\.[a-z]{2,}(:\d+)?([/?#]\S*)?$/i.test(value)) {
    return safeHref(`https://${value}`);
  }
  return safeHref(value);
}

const looksLikeUrl = (text) =>
  /^(https?:|mailto:|tel:|\/|www\.)/i.test(text.trim()) || /^[\w-]+(\.[\w-]+)*\.[a-z]{2,}(\/\S*)?$/i.test(text.trim());

/** En-têtes des requêtes de la page, comme htmx (voir tabHeaders, app.js). */
function pageHeaders() {
  let tab = "";
  try {
    tab = JSON.parse(document.getElementById("app")?.getAttribute("hx-headers") || "{}")["X-Espace"] || "";
  } catch {
    // Pas d'en-tête d'espace : le serveur prend l'espace de l'adresse.
  }
  return { "HX-Request": "true", "X-Espace": tab };
}

// ---------------------------------------- Barre de mise en forme, liens, tableaux

const toolsKey = new PluginKey("tools");

/** Barre de mise en forme, fiche et saisie des liens, barre des tableaux.
 *  Renvoie le plugin et `editLink(view)` (Ctrl K, menu « / »). */
export function floatingTools(host) {
  let tools = null;
  const editLink = () => tools?.editLink();
  const plugin = new Plugin({
    key: toolsKey,
    props: {
      handleDOMEvents: {
        mousedown() {
          if (tools) tools.pointer = true;
          return false;
        },
        focus(view) {
          tools?.update(view);
          return false;
        },
        blur(view) {
          // Le temps de voir si le focus part dans un de nos menus.
          setTimeout(() => tools?.update(view), 0);
          return false;
        },
      },
      handleClick(view, _pos, event) {
        // Cmd/Ctrl + clic sur un lien : on l'ouvre.
        const link = event.target.closest?.("a[href]");
        if (link && (event.metaKey || event.ctrlKey)) {
          openHref(link.getAttribute("href"));
          return true;
        }
        return false;
      },
    },
    view(view) {
      tools = new Tools(view, host);
      return tools;
    },
  });
  return { plugin, editLink };
}

/** Ouvre un lien : dans dashu, sur place ; ailleurs, dans un nouvel onglet. */
function openHref(href) {
  const safe = safeHref(href);
  if (!safe) return;
  if (safe.startsWith("/") || safe.startsWith("#")) window.location.assign(safe);
  else window.open(safe, "_blank", "noopener,noreferrer");
}

class Tools {
  constructor(view, host) {
    this.view = view;
    this.pointer = false;
    this.editing = null;

    // --- Barre de mise en forme
    this.bubble = new Floater(host, "ed-bubble", "Mise en forme");
    this.bubble.el.setAttribute("role", "toolbar");
    keepFocus(this.bubble.el);
    const markButton = (mark, iconName, label) => {
      const button = el("button", { type: "button", class: "ed-tool", "data-mark": mark, title: label, "aria-label": label }, icon(iconName));
      button.addEventListener("click", () => {
        toggleMark(M[mark])(this.view.state, this.view.dispatch);
        this.view.focus();
      });
      return button;
    };
    const linkButton = el("button", { type: "button", class: "ed-tool", title: "Lien (Ctrl K)", "aria-label": "Lien" }, icon("link"));
    linkButton.addEventListener("click", () => this.editLink());
    this.bubble.el.append(
      markButton("strong", "bold", "Gras (Ctrl B)"),
      markButton("em", "italic", "Italique (Ctrl I)"),
      markButton("strike", "strike", "Barré (Ctrl Maj S)"),
      markButton("code", "code-inline", "Code (Ctrl E)"),
      el("span", { class: "ed-sep", "aria-hidden": "true" }),
      linkButton,
    );

    // --- Fiche d'un lien
    this.card = new Floater(host, "ed-link-card", "Lien");
    keepFocus(this.card.el);
    this.cardHref = el("a", { class: "ed-link-href", rel: "noopener noreferrer nofollow" });
    this.cardHref.addEventListener("click", (event) => {
      event.preventDefault();
      openHref(this.cardHref.getAttribute("href"));
    });
    const edit = el("button", { type: "button", class: "ed-tool", title: "Modifier le lien", "aria-label": "Modifier le lien" }, icon("pencil"));
    edit.addEventListener("click", () => this.editLink());
    const unlink = el("button", { type: "button", class: "ed-tool", title: "Retirer le lien", "aria-label": "Retirer le lien" }, icon("unlink"));
    unlink.addEventListener("click", () => {
      const link = linkAt(this.view.state);
      if (link) this.view.dispatch(this.view.state.tr.removeMark(link.from, link.to, M.link));
      this.view.focus();
    });
    this.card.el.append(this.cardHref, edit, unlink);

    // --- Saisie d'un lien (adresse, ou recherche d'une note, d'un fichier)
    this.form = new Floater(host, "ed-link-form", "Ajouter un lien");
    this.input = el("input", {
      type: "text",
      class: "ed-link-input",
      placeholder: "Colle un lien ou cherche une note",
      "aria-label": "Adresse du lien, ou nom d'une note ou d'un fichier",
      autocomplete: "off",
      spellcheck: "false",
      enterkeyhint: "done",
    });
    this.results = el("div", { class: "ed-link-results", role: "listbox" });
    this.form.el.append(this.input, this.results);
    this.hits = [];
    this.active = -1;
    this.input.addEventListener("input", () => this.search());
    this.input.addEventListener("keydown", (event) => this.formKey(event));
    this.input.addEventListener("blur", () => {
      setTimeout(() => {
        if (this.editing && !this.form.el.contains(document.activeElement)) this.closeForm(false);
      }, 0);
    });
    this.results.addEventListener("mousedown", (event) => event.preventDefault());
    this.results.addEventListener("click", (event) => {
      const row = event.target.closest("[data-hit]");
      if (row) this.pick(this.hits[+row.dataset.hit]);
    });

    // --- Barre d'un tableau
    this.table = new Floater(host, "ed-table-bar", "Tableau");
    this.table.el.setAttribute("role", "toolbar");
    keepFocus(this.table.el);
    const tableButton = (iconName, label, command, text) => {
      const button = el("button", { type: "button", class: "ed-tool", title: label, "aria-label": label }, icon(iconName), text ? el("span", { text }) : null);
      button.addEventListener("click", () => {
        command(this.view.state, this.view.dispatch);
        this.view.focus();
      });
      return button;
    };
    this.table.el.append(
      tableButton("row-add", "Ajouter une ligne", addRowAfter, "Ligne"),
      tableButton("col-add", "Ajouter une colonne", addColumnAfter, "Colonne"),
      el("span", { class: "ed-sep", "aria-hidden": "true" }),
      tableButton("row-del", "Supprimer la ligne", deleteRow),
      tableButton("col-del", "Supprimer la colonne", deleteColumn),
      tableButton("trash", "Supprimer le tableau", deleteTable),
    );

    this.onPointerUp = () => {
      if (!this.pointer) return;
      this.pointer = false;
      this.update(this.view);
    };
    this.onScroll = () => this.update(this.view);
    document.addEventListener("mouseup", this.onPointerUp);
    document.addEventListener("touchend", this.onPointerUp);
    this.scroller = view.dom.closest("[data-note-scroll]");
    this.scroller?.addEventListener("scroll", this.onScroll, { passive: true });
    window.visualViewport?.addEventListener("resize", this.onScroll);
  }

  update(view) {
    this.view = view;
    const { state } = view;
    const focused = view.hasFocus();
    const slash = slashKey.getState(state)?.active;

    // Saisie de lien ouverte : elle reste où elle est.
    if (this.editing) {
      this.bubble.hide();
      this.card.hide();
      this.table.hide();
      return;
    }

    // Barre de mise en forme : sélection de texte, hors code.
    const { selection } = state;
    const text =
      selection instanceof TextSelection &&
      !selection.empty &&
      !selection.$from.parent.type.spec.code &&
      !selection.$to.parent.type.spec.code;
    if (focused && text && !this.pointer && !slash) {
      for (const button of this.bubble.el.querySelectorAll("[data-mark]")) {
        const type = M[button.dataset.mark];
        button.setAttribute("aria-pressed", String(state.doc.rangeHasMark(selection.from, selection.to, type)));
      }
      const a = view.coordsAtPos(selection.from);
      const b = view.coordsAtPos(selection.to);
      // Sur écran tactile, sous la sélection : iOS met son propre menu dessus.
      const touch = matchMedia("(pointer: coarse)").matches;
      this.bubble.place(
        { left: Math.min(a.left, b.left), top: Math.min(a.top, b.top), bottom: Math.max(a.bottom, b.bottom) },
        { above: !touch },
      );
    } else {
      this.bubble.hide();
    }

    // Fiche du lien sous le curseur.
    const link = focused && !slash ? linkAt(state) : null;
    if (link) {
      const href = link.mark.attrs.href;
      this.cardHref.setAttribute("href", safeHref(href) || "#");
      this.cardHref.textContent = href.replace(/^(https?:\/\/|mailto:)/, "").replace(/\/$/, "");
      this.cardHref.title = href;
      const at = view.coordsAtPos(link.from);
      this.card.place({ left: at.left, top: at.top, bottom: view.coordsAtPos(link.to).bottom });
    } else {
      this.card.hide();
    }

    // Barre du tableau, au-dessus de lui.
    if (focused && !slash && isInTable(state)) {
      const $cell = selectionCell(state);
      const dom = view.nodeDOM($cell.before($cell.depth - 1));
      const rect = dom instanceof Element ? dom.getBoundingClientRect() : null;
      // À droite, au-dessus : le bout de la ligne précédente est souvent vide.
      if (rect) {
        const right = Math.min(rect.right, view.dom.getBoundingClientRect().right);
        this.table.place({ left: rect.left, right, top: rect.top, bottom: rect.top }, { above: true, alignRight: true });
      }
    } else {
      this.table.hide();
    }
  }

  // --- Saisie d'un lien

  /** Ouvre la saisie : sur la sélection, sur le lien du curseur, ou pour
   *  insérer un nouveau lien à la place du curseur. */
  editLink() {
    const { state } = this.view;
    const { selection } = state;
    const link = linkAt(state);
    let range = null;
    let href = "";
    if (!selection.empty) {
      // Plage réduite au texte sélectionné : un double-clic au bout d'une
      // ligne peut ne prendre que le saut de ligne.
      state.doc.nodesBetween(selection.from, selection.to, (node, pos) => {
        if (!node.isText) return;
        const from = Math.max(pos, selection.from);
        const to = Math.min(pos + node.nodeSize, selection.to);
        if (from >= to) return;
        range = range ? { from: Math.min(range.from, from), to: Math.max(range.to, to) } : { from, to };
        const mark = M.link.isInSet(node.marks);
        if (mark && !href) href = mark.attrs.href;
      });
    } else if (link) {
      range = { from: link.from, to: link.to };
      href = link.mark.attrs.href;
    }
    this.editing = { range, at: range ? range.from : selection.to };
    this.update(this.view);
    this.input.value = href;
    this.input.removeAttribute("aria-invalid");
    this.hits = [];
    this.active = -1;
    this.results.replaceChildren();
    const a = this.view.coordsAtPos(range ? range.from : this.editing.at);
    const b = this.view.coordsAtPos(range ? range.to : this.editing.at);
    this.form.place({ left: a.left, top: a.top, bottom: Math.max(a.bottom, b.bottom) });
    this.input.focus();
    this.input.select();
    if (!href) this.search();
  }

  closeForm(refocus = true) {
    this.editing = null;
    this.form.hide();
    clearTimeout(this.timer);
    if (refocus) this.view.focus();
    this.update(this.view);
  }

  /** Applique le lien : sur la plage choisie, ou insère `label`. */
  apply(href, label) {
    const editing = this.editing;
    if (!editing) return;
    const { state } = this.view;
    const mark = M.link.create({ href });
    let tr = state.tr;
    if (editing.range) {
      tr = tr.removeMark(editing.range.from, editing.range.to, M.link).addMark(editing.range.from, editing.range.to, mark);
      tr.setSelection(TextSelection.create(tr.doc, editing.range.to));
    } else {
      const text = label || href.replace(/^mailto:/, "");
      const marks = state.doc.resolve(editing.at).marks().filter((m) => m.type !== M.link);
      tr = tr.insert(editing.at, schema.text(text, [...marks, mark]));
      tr.setSelection(TextSelection.create(tr.doc, editing.at + text.length));
    }
    tr.removeStoredMark(M.link);
    this.editing = null;
    this.form.hide();
    this.view.dispatch(tr.scrollIntoView());
    this.view.focus();
  }

  pick(hit) {
    if (hit) this.apply(hit.href, hit.v);
  }

  submit() {
    if (this.active >= 0 && this.hits[this.active]) return this.pick(this.hits[this.active]);
    const value = this.input.value.trim();
    if (!value) {
      // Champ vidé sur un lien existant : on retire le lien.
      if (this.editing?.range) {
        const { from, to } = this.editing.range;
        this.view.dispatch(this.view.state.tr.removeMark(from, to, M.link));
      }
      return this.closeForm();
    }
    const href = normalizeUrl(value);
    if (!href) {
      if (this.hits[0]) return this.pick(this.hits[0]);
      this.input.setAttribute("aria-invalid", "true");
      return;
    }
    this.apply(href);
  }

  formKey(event) {
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      this.submit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      this.closeForm();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (!this.hits.length) return;
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      this.active = (this.active + step + this.hits.length + 1) % (this.hits.length + 1);
      if (this.active === this.hits.length) this.active = -1;
      this.renderHits();
    }
  }

  /** Recherche des notes et fichiers qui correspondent (GET /cmd/suggest). */
  search() {
    this.input.removeAttribute("aria-invalid");
    clearTimeout(this.timer);
    const q = this.input.value.trim();
    if (looksLikeUrl(q)) {
      this.hits = [];
      this.active = -1;
      this.renderHits();
      return;
    }
    this.timer = setTimeout(async () => {
      try {
        const response = await fetch(`/cmd/suggest?q=${encodeURIComponent(q)}`, { headers: pageHeaders() });
        if (!response.ok) throw new Error(String(response.status));
        const data = await response.json();
        if (this.input.value.trim() !== q || !this.editing) return;
        this.hits = (data.items || []).filter((hit) => safeHref(hit.href)).slice(0, 6);
        this.active = q && this.hits.length ? 0 : -1;
        this.renderHits();
      } catch {
        this.hits = [];
        this.renderHits();
      }
    }, 120);
  }

  renderHits() {
    this.results.replaceChildren(
      ...this.hits.map((hit, index) =>
        el(
          "button",
          { type: "button", class: "ed-hit", role: "option", "data-hit": index, "aria-selected": String(index === this.active) },
          icon(hit.href.startsWith("/files/") ? "file-blank" : "notes"),
          el("span", { class: "ed-hit-text" }, el("span", { class: "ed-hit-title", text: hit.v }), el("span", { class: "ed-hit-help", text: hit.help })),
        ),
      ),
    );
    if (this.editing) {
      // La liste a changé de hauteur : on replace la saisie.
      const { range, at } = this.editing;
      const a = this.view.coordsAtPos(range ? range.from : at);
      const b = this.view.coordsAtPos(range ? range.to : at);
      this.form.place({ left: a.left, top: a.top, bottom: Math.max(a.bottom, b.bottom) });
    }
  }

  destroy() {
    document.removeEventListener("mouseup", this.onPointerUp);
    document.removeEventListener("touchend", this.onPointerUp);
    this.scroller?.removeEventListener("scroll", this.onScroll);
    window.visualViewport?.removeEventListener("resize", this.onScroll);
    clearTimeout(this.timer);
    for (const floater of [this.bubble, this.card, this.form, this.table]) floater.el.remove();
  }
}
