// Commandes de l'éditeur : transformer un bloc (« / », raccourcis), insérer
// un tableau ou un séparateur, raccourcis Markdown pendant la frappe
// (« # », « - », « [] »…) et clavier.

import { NodeSelection, TextSelection } from "prosemirror-state";
import { findWrapping, canJoin } from "prosemirror-transform";
import {
  setBlockType,
  wrapIn,
  lift,
  toggleMark,
  chainCommands,
  newlineInCode,
  exitCode,
} from "prosemirror-commands";
import { wrapInList, splitListItem, liftListItem, sinkListItem } from "prosemirror-schema-list";
import { InputRule, inputRules, textblockTypeInputRule, wrappingInputRule, undoInputRule } from "prosemirror-inputrules";
import { undo, redo } from "prosemirror-history";
import { goToNextCell, addRowAfter, isInTable, selectionCell, TableMap } from "prosemirror-tables";
import { schema } from "./format.js";

const N = schema.nodes;
const M = schema.marks;

// ------------------------------------------------------------- Outils

/** Profondeur de l'élément de liste qui contient la position, ou -1. */
function itemDepth($pos) {
  for (let d = $pos.depth; d > 0; d--) if ($pos.node(d).type === N.list_item) return d;
  return -1;
}

/** Le curseur est-il dans le premier paragraphe d'un élément de liste ? */
function inItemHead($pos) {
  const d = $pos.depth - 1;
  return d > 0 && $pos.node(d).type === N.list_item && $pos.index(d) === 0 ? d : -1;
}

/** Sort le bloc du curseur de toutes ses listes. */
function leaveLists(view) {
  for (let i = 0; i < 8 && itemDepth(view.state.selection.$from) > 0; i++) {
    if (!liftListItem(N.list_item)(view.state, view.dispatch)) break;
  }
}

// --------------------------------------------------- Transformer un bloc

/** Transforme le bloc du curseur, comme le menu « / » de Notion.
 *  kind : paragraph, heading, code_block, bullet, ordered, task, quote. */
export function turnInto(view, kind, attrs = {}) {
  const run = (cmd) => cmd(view.state, view.dispatch, view);

  if (kind === "paragraph" || kind === "heading" || kind === "code_block") {
    leaveLists(view);
    const type = N[kind];
    // Texte sur une citation : on sort de la citation.
    if (kind === "paragraph" && view.state.selection.$from.node(-1).type === N.blockquote) run(lift);
    run(setBlockType(type, kind === "heading" ? { level: attrs.level || 1 } : {}));
    return;
  }

  if (kind === "quote") {
    leaveLists(view);
    if (view.state.selection.$from.node(-1).type !== N.blockquote) run(wrapIn(N.blockquote));
    return;
  }

  // Listes : puces, numéros, cases à cocher.
  const listType = kind === "ordered" ? N.ordered_list : N.bullet_list;
  const checked = kind === "task" ? false : null;
  const { $from } = view.state.selection;
  const d = itemDepth($from);

  if (d > 0) {
    const list = $from.node(d - 1);
    const tr = view.state.tr;
    // Une case reste dans sa liste (numérotée ou non) ; une puce devenue
    // numéro change toute la liste si elle est seule ou imbriquée, sinon
    // l'élément sort de sa liste pour en commencer une autre.
    if (kind !== "task" && list.type !== listType) {
      const nested = d >= 2 && $from.node(d - 2).type === N.list_item;
      if (list.childCount > 1 && !nested) {
        leaveLists(view);
        return turnInto(view, kind, attrs);
      }
      tr.setNodeMarkup($from.before(d - 1), listType, { tight: list.attrs.tight });
    }
    tr.setNodeMarkup($from.before(d), null, { ...$from.node(d).attrs, checked });
    view.dispatch(tr);
    return;
  }

  if ($from.parent.type !== N.paragraph) run(setBlockType(N.paragraph));
  if (!run(wrapInList(listType))) return;
  if (checked !== null) {
    const $now = view.state.selection.$from;
    const item = itemDepth($now);
    if (item > 0) view.dispatch(view.state.tr.setNodeMarkup($now.before(item), null, { checked }));
  }
}

// ----------------------------------------------------------- Insérer

/** Remplace le paragraphe vide du curseur par ces blocs, ou les pose juste
 *  après le bloc courant. Le curseur va dans le premier texte inséré, ou
 *  dans le paragraphe qui suit. */
export function insertBlocks(view, nodes, { cursorInside = true } = {}) {
  const { state } = view;
  const { $from } = state.selection;
  let depth = $from.depth;
  // On remonte jusqu'à un parent qui accepte ces blocs.
  while (depth > 0 && !$from.node(depth - 1).type.contentMatch.matchType(nodes[0].type)) depth--;
  if (depth === 0) return false;

  const block = $from.node(depth);
  const empty = block.isTextblock && block.content.size === 0;
  const from = empty ? $from.before(depth) : $from.after(depth);
  const to = empty ? $from.after(depth) : from;

  const parent = $from.node(depth - 1);
  const index = $from.index(depth - 1) + (empty ? 1 : 1);
  const content = [...nodes];
  // Un paragraphe après, si rien ne suit : on peut continuer à écrire.
  if (index >= parent.childCount || !cursorInside) content.push(N.paragraph.create());

  const tr = state.tr.replaceWith(from, to, content);
  const size = nodes.reduce((sum, n) => sum + n.nodeSize, 0);
  const target = cursorInside ? from + 1 : from + size + 1;
  tr.setSelection(TextSelection.near(tr.doc.resolve(Math.min(target, tr.doc.content.size))));
  view.dispatch(tr.scrollIntoView());
  return true;
}

/** Tableau de `cols` colonnes : une ligne d'en-tête et `rows - 1` lignes. */
export function createTable(rows = 3, cols = 3) {
  const row = (type) =>
    N.table_row.create(
      null,
      Array.from({ length: cols }, () => type.createAndFill()),
    );
  return N.table.create(null, [
    row(N.table_header),
    ...Array.from({ length: rows - 1 }, () => row(N.table_cell)),
  ]);
}

export function insertTable(view) {
  return insertBlocks(view, [createTable()]);
}

export function insertRule(view) {
  return insertBlocks(view, [N.horizontal_rule.create()], { cursorInside: false });
}

/** Insère du texte (avec ses marques) à la place de la sélection. */
export function insertText(view, text, marks = null) {
  const node = schema.text(text, marks);
  view.dispatch(view.state.tr.replaceSelectionWith(node, false).scrollIntoView());
}

// --------------------------------------------- Raccourcis pendant la frappe

/** `**gras**`, `*italique*`, `~~barré~~`, `` `code` `` : la marque se pose
 *  quand on tape le dernier caractère. */
function markRule(regexp, type) {
  return new InputRule(
    regexp,
    (state, match, start, end) => {
      const from = start + (match[0].length - match[1].length);
      const marks = [...state.doc.resolve(from).marks(), type.create()];
      const tr = state.tr.replaceWith(from, end, schema.text(match[2], marks));
      return tr.removeStoredMark(type);
    },
    { inCodeMark: false },
  );
}

/** `[] ` ou `[x] ` : case à cocher. */
const taskRule = new InputRule(/^\[([ xX]?)\]\s$/, (state, match, start, end) => {
  const checked = match[1].toLowerCase() === "x";
  const $start = state.doc.resolve(start);
  const tr = state.tr.delete(start, end);
  const head = inItemHead($start);
  if (head > 0) return tr.setNodeMarkup($start.before(head), null, { ...$start.node(head).attrs, checked });
  const range = tr.doc.resolve(start).blockRange();
  const wrapping = range && findWrapping(range, N.bullet_list);
  if (!wrapping) return null;
  tr.wrap(
    range,
    wrapping.map((w) => (w.type === N.list_item ? { type: N.list_item, attrs: { checked } } : w)),
  );
  const before = tr.doc.resolve(start - 1).nodeBefore;
  if (before && before.type === N.bullet_list && canJoin(tr.doc, start - 1)) tr.join(start - 1);
  return tr;
});

/** `---` seul sur sa ligne : séparateur. */
const ruleRule = new InputRule(/^(?:---|\*\*\*|___)$/, (state, _match, start, end) => {
  const $start = state.doc.resolve(start);
  if ($start.parent.type !== N.paragraph || $start.parent.content.size !== end - start) return null;
  const index = $start.index(-1);
  if (!$start.node(-1).canReplaceWith(index, index + 1, N.horizontal_rule)) return null;
  const at = $start.before();
  const tr = state.tr.replaceWith(at, $start.after(), [N.horizontal_rule.create(), N.paragraph.create()]);
  return tr.setSelection(TextSelection.create(tr.doc, at + 2));
});

export function markdownShortcuts() {
  return inputRules({
    rules: [
      textblockTypeInputRule(/^(#{1,3})\s$/, N.heading, (m) => ({ level: m[1].length })),
      taskRule,
      wrappingInputRule(/^\s*[-+*]\s$/, N.bullet_list),
      wrappingInputRule(
        /^(\d+)\.\s$/,
        N.ordered_list,
        (m) => ({ order: +m[1] }),
        (m, node) => node.childCount + node.attrs.order === +m[1],
      ),
      wrappingInputRule(/^\s*>\s$/, N.blockquote),
      textblockTypeInputRule(/^```([\w+#.-]*)\s$/, N.code_block, (m) => ({ params: m[1] })),
      ruleRule,
      markRule(/(?:^|[^\w*])(\*\*([^*\s](?:[^*]*[^*\s])?)\*\*)$/, M.strong),
      markRule(/(?:^|[^\w*])(\*([^*\s](?:[^*]*[^*\s])?)\*)$/, M.em),
      markRule(/(?:^|[^\w_])(_([^_\s](?:[^_]*[^_\s])?)_)$/, M.em),
      markRule(/(?:^|[^\w~])(~~([^~\s](?:[^~]*[^~\s])?)~~)$/, M.strike),
      markRule(/(?:^|[^\w`])(`([^`]+)`)$/, M.code),
    ],
  });
}

// ------------------------------------------------------------- Clavier

/** Entrée dans une liste : la nouvelle ligne garde le genre (une case
 *  suit une case, décochée). Sur une ligne vide, on sort de la liste. */
function splitItem(state, dispatch) {
  const { $from } = state.selection;
  if ($from.depth < 2 || $from.node(-1).type !== N.list_item) return false;
  const item = $from.node(-1);
  return splitListItem(N.list_item, item.attrs.checked === null ? null : { checked: false })(state, dispatch);
}

function liftEmptyItem(state, dispatch) {
  const { $from, empty } = state.selection;
  if (!empty || $from.parent.content.size || $from.depth < 2 || $from.node(-1).type !== N.list_item) return false;
  return liftListItem(N.list_item)(state, dispatch);
}

/** « ```js » puis Entrée : bloc de code. */
function fenceOnEnter(state, dispatch) {
  const { $from, empty } = state.selection;
  if (!empty || $from.parent.type !== N.paragraph || $from.parentOffset !== $from.parent.content.size) return false;
  const match = /^```([\w+#.-]*)$/.exec($from.parent.textContent);
  if (!match) return false;
  if (dispatch) {
    const tr = state.tr.delete($from.start(), $from.end());
    tr.setBlockType($from.start(), $from.start(), N.code_block, { params: match[1] });
    dispatch(tr.scrollIntoView());
  }
  return true;
}

/** Cellule de la ligne suivante (Entrée) ; une ligne s'ajoute au bout. */
function cellBelow(state, dispatch, view) {
  if (!isInTable(state)) return false;
  if (!dispatch) return true;
  const move = (s) => {
    const $cell = selectionCell(s);
    const table = $cell.node(-1);
    const start = $cell.start(-1);
    const map = TableMap.get(table);
    const rect = map.findCell($cell.pos - start);
    if (rect.bottom >= map.height) return null;
    const pos = start + map.map[rect.bottom * map.width + rect.left];
    const cell = s.doc.nodeAt(pos);
    return s.tr.setSelection(TextSelection.create(s.doc, pos + 1 + cell.content.size));
  };
  let tr = move(state);
  if (!tr) {
    addRowAfter(state, dispatch);
    tr = move(view ? view.state : state);
    if (!tr) return true;
    (view ? view.dispatch : dispatch)(tr.scrollIntoView());
    return true;
  }
  dispatch(tr.scrollIntoView());
  return true;
}

/** Tab dans un tableau : cellule suivante, nouvelle ligne au bout. */
function nextCell(state, dispatch, view) {
  if (!isInTable(state)) return false;
  if (goToNextCell(1)(state, dispatch)) return true;
  addRowAfter(state, dispatch);
  if (view) goToNextCell(1)(view.state, view.dispatch);
  return true;
}

/** Retour arrière au début d'un bloc : le bloc redevient du texte avant
 *  de se fondre dans le précédent (titre, liste, citation, code). */
function backspaceAtStart(state, dispatch, view) {
  const { $cursor } = state.selection;
  if (!$cursor || $cursor.parentOffset > 0) return false;
  const parent = $cursor.parent;
  if (parent.type === N.heading || parent.type === N.code_block) {
    return setBlockType(N.paragraph)(state, dispatch);
  }
  if (inItemHead($cursor) > 0) return liftListItem(N.list_item)(state, dispatch, view);
  if ($cursor.node(-1).type === N.blockquote && $cursor.index(-1) === 0) return lift(state, dispatch);
  return false;
}

/** Ctrl/Cmd + Entrée sur une case : la coche ou la décoche. */
function toggleTask(state, dispatch) {
  const d = itemDepth(state.selection.$from);
  if (d < 0) return false;
  const item = state.selection.$from.node(d);
  if (item.attrs.checked === null) return false;
  if (dispatch) {
    dispatch(state.tr.setNodeMarkup(state.selection.$from.before(d), null, { ...item.attrs, checked: !item.attrs.checked }));
  }
  return true;
}

function hardBreak(state, dispatch) {
  if (isInTable(state)) return true;
  if (dispatch) dispatch(state.tr.replaceSelectionWith(N.hard_break.create()).scrollIntoView());
  return true;
}

/** Tab dans du code : deux espaces. */
function indentCode(state, dispatch) {
  if (!state.selection.$from.parent.type.spec.code) return false;
  if (dispatch) dispatch(state.tr.insertText("  ").scrollIntoView());
  return true;
}

/** Raccourcis clavier propres à l'éditeur. `hooks` vient de editor.js :
 *  sauvegarde immédiate, lien, retour au titre. */
export function editorKeys(hooks) {
  const block = (kind, attrs) => (_s, _d, view) => {
    if (view) turnInto(view, kind, attrs);
    return true;
  };
  const keys = {
    "Mod-z": undo,
    "Shift-Mod-z": redo,
    "Mod-y": redo,
    Backspace: chainCommands(undoInputRule, backspaceAtStart),
    "Mod-b": toggleMark(M.strong),
    "Mod-i": toggleMark(M.em),
    "Mod-Shift-s": toggleMark(M.strike),
    "Mod-Shift-x": toggleMark(M.strike),
    "Mod-e": toggleMark(M.code),
    "Mod-k": (_s, _d, view) => {
      if (view) hooks.link(view);
      return true;
    },
    "Mod-s": () => {
      hooks.save();
      return true;
    },
    // Échap sort de l'éditeur (un menu ouvert le prend avant), pour que « / »
    // ouvre ensuite la barre de commandes. ProseMirror retient Échap : la
    // page (app.js) ne peut donc pas le faire elle-même.
    Escape: (_s, _d, view) => {
      view?.dom.blur();
      return true;
    },
    "Mod-Alt-0": block("paragraph"),
    "Mod-Alt-1": block("heading", { level: 1 }),
    "Mod-Alt-2": block("heading", { level: 2 }),
    "Mod-Alt-3": block("heading", { level: 3 }),
    "Mod-Alt-4": block("task"),
    "Mod-Alt-5": block("bullet"),
    "Mod-Alt-6": block("ordered"),
    "Mod-Alt-8": block("code_block"),
    Enter: chainCommands(cellBelow, fenceOnEnter, liftEmptyItem, splitItem),
    "Shift-Enter": chainCommands(newlineInCode, hardBreak),
    "Mod-Enter": chainCommands(toggleTask, exitCode),
    Tab: chainCommands(nextCell, sinkListItem(N.list_item), indentCode),
    "Shift-Tab": chainCommands(goToNextCell(-1), liftListItem(N.list_item)),
    ArrowUp: (state, _d, view) => {
      // Flèche haut sur la première ligne : retour au titre.
      const { $head, empty } = state.selection;
      if (!empty || !view || !view.endOfTextblock("up")) return false;
      const first = TextSelection.atStart(state.doc).$head;
      if (first.parent !== $head.parent) return false;
      return hooks.toTitle();
    },
  };
  return keys;
}

/** Sélectionne une image ou un séparateur cliqué. */
export function selectNode(view, pos) {
  view.dispatch(view.state.tr.setSelection(NodeSelection.create(view.state.doc, pos)));
}
