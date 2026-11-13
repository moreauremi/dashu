// Schéma du document et conversion Markdown <-> document ProseMirror.
//
// Le Markdown reste le format stocké (body_md) : la recherche plein texte,
// l'API d'Hermès, les extraits et le rendu côté serveur n'en voient pas
// d'autre. L'éditeur le lit à l'ouverture (markdown-it) et le réécrit à
// chaque modification. N'existent donc ici que des blocs que le Markdown sait
// écrire : titres, listes (puces, numéros, cases à cocher), citations, code,
// séparateurs, tableaux, images et liens.

import { Schema } from "prosemirror-model";
import { tableNodes } from "prosemirror-tables";
import { MarkdownParser, MarkdownSerializer, MarkdownSerializerState } from "prosemirror-markdown";
import markdownit from "markdown-it";

/** Adresse qu'on accepte de suivre ou d'afficher : web, courriel,
 *  téléphone, ou chemin de dashu. Tout le reste (javascript:, data:…) est
 *  refusé, au collage comme à la saisie. */
export function safeHref(href) {
  const value = String(href || "").trim();
  if (!value) return null;
  if (/^\/(?!\/)/.test(value) || value.startsWith("#")) return value;
  return /^(https?:|mailto:|tel:)/i.test(value) ? value : null;
}

/** Source d'image acceptée : celles que la CSP laisse passer. */
function safeSrc(src) {
  const value = String(src || "").trim();
  if (/^\/(?!\/)/.test(value) || /^https:/i.test(value)) return value;
  return /^data:image\/(png|jpeg|gif|webp);/i.test(value) ? value : null;
}

const ALIGNS = ["left", "center", "right"];

export const schema = new Schema({
  nodes: {
    doc: { content: "block+" },

    paragraph: {
      content: "inline*",
      group: "block",
      parseDOM: [{ tag: "p" }],
      toDOM: () => ["p", 0],
    },

    heading: {
      attrs: { level: { default: 1 } },
      content: "inline*",
      group: "block",
      defining: true,
      parseDOM: [1, 2, 3, 4, 5, 6].map((level) => ({ tag: `h${level}`, attrs: { level } })),
      toDOM: (node) => [`h${node.attrs.level}`, 0],
    },

    blockquote: {
      content: "block+",
      group: "block",
      defining: true,
      parseDOM: [{ tag: "blockquote" }],
      toDOM: () => ["blockquote", 0],
    },

    code_block: {
      attrs: { params: { default: "" } },
      content: "text*",
      marks: "",
      group: "block",
      code: true,
      defining: true,
      parseDOM: [
        {
          tag: "pre",
          preserveWhitespace: "full",
          getAttrs: (dom) => ({ params: dom.getAttribute("data-lang") || "" }),
        },
      ],
      toDOM: (node) => [
        "pre",
        node.attrs.params ? { "data-lang": node.attrs.params } : {},
        ["code", 0],
      ],
    },

    horizontal_rule: {
      group: "block",
      parseDOM: [{ tag: "hr" }],
      toDOM: () => ["hr"],
    },

    bullet_list: {
      attrs: { tight: { default: true } },
      content: "list_item+",
      group: "block",
      parseDOM: [{ tag: "ul", getAttrs: (dom) => ({ tight: !dom.hasAttribute("data-loose") }) }],
      toDOM: (node) => ["ul", node.attrs.tight ? {} : { "data-loose": "" }, 0],
    },

    ordered_list: {
      attrs: { order: { default: 1 }, tight: { default: true } },
      content: "list_item+",
      group: "block",
      parseDOM: [
        {
          tag: "ol",
          getAttrs: (dom) => ({
            order: dom.hasAttribute("start") ? +dom.getAttribute("start") : 1,
            tight: !dom.hasAttribute("data-loose"),
          }),
        },
      ],
      toDOM: (node) => [
        "ol",
        {
          ...(node.attrs.order === 1 ? {} : { start: node.attrs.order }),
          ...(node.attrs.tight ? {} : { "data-loose": "" }),
        },
        0,
      ],
    },

    // Élément de liste. `checked` vaut null pour une puce ou un numéro,
    // true/false pour une case à cocher (« - [ ] » en Markdown).
    list_item: {
      attrs: { checked: { default: null } },
      content: "paragraph block*",
      defining: true,
      parseDOM: [
        {
          tag: "li",
          getAttrs: (dom) => {
            const box = dom.querySelector(":scope > input[type=checkbox]");
            const flag = dom.getAttribute("data-checked");
            if (box) return { checked: box.checked };
            return { checked: flag === null ? null : flag === "true" };
          },
        },
      ],
      toDOM: (node) => [
        "li",
        node.attrs.checked === null ? {} : { "data-checked": String(node.attrs.checked) },
        0,
      ],
    },

    // Tableaux : une ligne d'en-tête, des cellules d'une seule ligne de
    // texte, comme dans un tableau Markdown (GFM).
    ...tableNodes({
      tableGroup: "block",
      cellContent: "inline*",
      cellAttributes: {
        align: {
          default: null,
          getFromDOM: (dom) => {
            const value = dom.getAttribute("data-align") || dom.getAttribute("align");
            return ALIGNS.includes(value) ? value : null;
          },
          setDOMAttr: (value, attrs) => {
            if (value) attrs["data-align"] = value;
          },
        },
      },
    }),

    text: { group: "inline" },

    image: {
      inline: true,
      attrs: { src: {}, alt: { default: null }, title: { default: null } },
      group: "inline",
      draggable: true,
      parseDOM: [
        {
          tag: "img[src]",
          getAttrs: (dom) => {
            const src = safeSrc(dom.getAttribute("src"));
            if (!src) return false;
            return { src, alt: dom.getAttribute("alt"), title: dom.getAttribute("title") };
          },
        },
      ],
      toDOM: (node) => ["img", { src: node.attrs.src, alt: node.attrs.alt || "", title: node.attrs.title }],
    },

    hard_break: {
      inline: true,
      group: "inline",
      selectable: false,
      parseDOM: [{ tag: "br" }],
      toDOM: () => ["br"],
    },
  },

  marks: {
    em: {
      parseDOM: [{ tag: "i" }, { tag: "em" }, { style: "font-style=italic" }],
      toDOM: () => ["em", 0],
    },
    strong: {
      parseDOM: [
        { tag: "strong" },
        { tag: "b", getAttrs: (dom) => dom.style.fontWeight !== "normal" && null },
        { style: "font-weight", getAttrs: (v) => /^(bold(er)?|[6-9]\d{2})$/.test(v) && null },
      ],
      toDOM: () => ["strong", 0],
    },
    strike: {
      parseDOM: [{ tag: "s" }, { tag: "del" }, { tag: "strike" }],
      toDOM: () => ["s", 0],
    },
    link: {
      attrs: { href: {}, title: { default: null } },
      inclusive: false,
      parseDOM: [
        {
          tag: "a[href]",
          getAttrs: (dom) => {
            const href = safeHref(dom.getAttribute("href"));
            return href ? { href, title: dom.getAttribute("title") } : false;
          },
        },
      ],
      toDOM: (mark) => [
        "a",
        { href: mark.attrs.href, title: mark.attrs.title, rel: "noopener noreferrer nofollow" },
        0,
      ],
    },
    code: {
      parseDOM: [{ tag: "code" }],
      toDOM: () => ["code", 0],
    },
  },
});

// ------------------------------------------------------------- Lecture

const tokenizer = markdownit({ html: false, linkify: false, typographer: false });

// Cases à cocher « - [ ] texte » / « - [x] texte » : markdown-it ne les
// connaît pas. On les repère après coup sur le premier paragraphe d'un
// élément de liste, on retire la case du texte et on la note sur l'élément.
tokenizer.core.ruler.push("dashu_tasks", (state) => {
  const tokens = state.tokens;
  for (let i = 2; i < tokens.length; i++) {
    const inline = tokens[i];
    if (inline.type !== "inline" || tokens[i - 1].type !== "paragraph_open") continue;
    if (tokens[i - 2].type !== "list_item_open") continue;
    const first = inline.children?.[0];
    const match = first?.type === "text" && /^\[([ xX])\](?:[ \t]+|$)/.exec(first.content);
    if (!match) continue;
    tokens[i - 2].meta = { ...tokens[i - 2].meta, checked: match[1] !== " " };
    first.content = first.content.slice(match[0].length);
    if (!first.content) inline.children.shift();
  }
});

/** Alignement d'une colonne : markdown-it le pose en style sur la cellule. */
function alignOf(token) {
  const match = /text-align:\s*(left|center|right)/.exec(token.attrGet("style") || "");
  return { align: match ? match[1] : null };
}

const parser = new MarkdownParser(schema, tokenizer, {
  blockquote: { block: "blockquote" },
  paragraph: { block: "paragraph" },
  list_item: { block: "list_item", getAttrs: (tok) => ({ checked: tok.meta?.checked ?? null }) },
  bullet_list: { block: "bullet_list", getAttrs: (_, tokens, i) => ({ tight: isTight(tokens, i) }) },
  ordered_list: {
    block: "ordered_list",
    getAttrs: (tok, tokens, i) => ({ order: +tok.attrGet("start") || 1, tight: isTight(tokens, i) }),
  },
  heading: { block: "heading", getAttrs: (tok) => ({ level: +tok.tag.slice(1) }) },
  code_block: { block: "code_block", noCloseToken: true },
  fence: { block: "code_block", getAttrs: (tok) => ({ params: tok.info.trim() }), noCloseToken: true },
  hr: { node: "horizontal_rule" },
  image: {
    node: "image",
    getAttrs: (tok) => ({
      src: safeSrc(tok.attrGet("src")) || "",
      title: tok.attrGet("title") || null,
      alt: tok.content || null,
    }),
  },
  hardbreak: { node: "hard_break" },
  table: { block: "table" },
  thead: { ignore: true },
  tbody: { ignore: true },
  tr: { block: "table_row" },
  th: { block: "table_header", getAttrs: alignOf },
  td: { block: "table_cell", getAttrs: alignOf },
  em: { mark: "em" },
  strong: { mark: "strong" },
  s: { mark: "strike" },
  link: {
    mark: "link",
    getAttrs: (tok) => ({ href: safeHref(tok.attrGet("href")) || "#", title: tok.attrGet("title") || null }),
  },
  code_inline: { mark: "code", noCloseToken: true },
});

function isTight(tokens, i) {
  while (++i < tokens.length) if (tokens[i].type !== "list_item_open") return tokens[i].hidden;
  return false;
}

/** Markdown -> document. Une note illisible (cas extrême) reste éditable :
 *  on la montre alors comme un seul bloc de texte. */
export function parseMarkdown(text) {
  try {
    return parser.parse(text || "");
  } catch (error) {
    console.error("Lecture du Markdown impossible", error);
    const lines = String(text || "").split("\n");
    return schema.node(
      "doc",
      null,
      lines.map((line) => schema.node("paragraph", null, line ? [schema.text(line)] : [])),
    );
  }
}

// ------------------------------------------------------------- Écriture

const plain = (state, node) => {
  state.renderInline(node);
  state.closeBlock(node);
};

const nodes = {
  blockquote(state, node) {
    state.wrapBlock("> ", null, node, () => state.renderContent(node));
  },
  code_block(state, node) {
    const ticks = node.textContent.match(/`{3,}/gm);
    const fence = ticks ? ticks.sort().slice(-1)[0] + "`" : "```";
    state.write(fence + (node.attrs.params || "") + "\n");
    state.text(node.textContent, false);
    state.ensureNewLine();
    state.write(fence);
    state.closeBlock(node);
  },
  heading(state, node) {
    state.write(`${"#".repeat(node.attrs.level)} `);
    state.renderInline(node, false);
    state.closeBlock(node);
  },
  horizontal_rule(state, node) {
    state.write("---");
    state.closeBlock(node);
  },
  bullet_list(state, node) {
    state.renderList(node, "  ", () => "- ");
  },
  ordered_list(state, node) {
    const start = node.attrs.order ?? 1;
    const width = String(start + node.childCount - 1).length;
    state.renderList(node, " ".repeat(width + 2), (i) => {
      const n = String(start + i);
      return " ".repeat(width - n.length) + n + ". ";
    });
  },
  list_item(state, node) {
    if (node.attrs.checked !== null) {
      const empty = node.childCount === 1 && node.firstChild.content.size === 0;
      state.write((node.attrs.checked ? "[x]" : "[ ]") + (empty ? "" : " "));
    }
    state.renderContent(node);
  },
  paragraph: plain,
  table(state, node) {
    const rows = [];
    node.forEach((row) => {
      const cells = [];
      row.forEach((cell) => cells.push(cellText(cell)));
      rows.push(cells);
    });
    const width = Math.max(1, ...rows.map((r) => r.length));
    const aligns = [];
    node.firstChild?.forEach((cell) => aligns.push(cell.attrs.align));
    const line = (cells) =>
      `| ${Array.from({ length: width }, (_, i) => cells[i] ?? "").join(" | ")} |`;
    const rule = Array.from({ length: width }, (_, i) => {
      const a = aligns[i];
      return a === "center" ? ":---:" : a === "right" ? "---:" : a === "left" ? ":---" : "---";
    });
    const lines = [line(rows[0] || []), `| ${rule.join(" | ")} |`, ...rows.slice(1).map(line)];
    lines.forEach((text, i) => {
      if (i) state.ensureNewLine();
      state.write(text);
    });
    state.closeBlock(node);
  },
  image(state, node) {
    state.write(
      `![${state.esc(node.attrs.alt || "")}](${escapeUrl(node.attrs.src)}` +
        (node.attrs.title ? ` "${node.attrs.title.replace(/"/g, '\\"')}"` : "") +
        ")",
    );
  },
  hard_break(state, node, parent, index) {
    for (let i = index + 1; i < parent.childCount; i++) {
      if (parent.child(i).type !== node.type) {
        // Dans un tableau, une cellule tient sur une ligne.
        state.write(state.inTableCell ? " " : "\\\n");
        return;
      }
    }
  },
  text(state, node) {
    state.text(node.text, !state.inAutolink);
  },
};

const marks = {
  em: { open: "*", close: "*", mixable: true, expelEnclosingWhitespace: true },
  strong: { open: "**", close: "**", mixable: true, expelEnclosingWhitespace: true },
  strike: { open: "~~", close: "~~", mixable: true, expelEnclosingWhitespace: true },
  link: {
    open(state, mark, parent, index) {
      state.inAutolink = isPlainUrl(mark, parent, index);
      return state.inAutolink ? "<" : "[";
    },
    close(state, mark) {
      const auto = state.inAutolink;
      state.inAutolink = undefined;
      if (auto) return ">";
      const href = escapeUrl(mark.attrs.href);
      const title = mark.attrs.title ? ` "${mark.attrs.title.replace(/"/g, '\\"')}"` : "";
      return `](${href}${title})`;
    },
    mixable: true,
  },
  code: {
    open: (_s, _m, parent, index) => backticks(parent.child(index), -1),
    close: (_s, _m, parent, index) => backticks(parent.child(index - 1), 1),
    escape: false,
  },
};

/** Adresse sans parenthèse ni espace : elle ne fermerait pas le lien trop tôt. */
const escapeUrl = (url) => url.replace(/[()\s]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0")}`);

function backticks(node, side) {
  let len = 0;
  if (node.isText) for (const m of node.text.matchAll(/`+/g)) len = Math.max(len, m[0].length);
  let out = len > 0 && side > 0 ? " `" : "`";
  out += "`".repeat(len);
  if (len > 0 && side < 0) out += " ";
  return out;
}

function isPlainUrl(link, parent, index) {
  if (link.attrs.title || !/^\w+:/.test(link.attrs.href)) return false;
  const content = parent.child(index);
  if (!content.isText || content.text !== link.attrs.href) return false;
  if (content.marks[content.marks.length - 1] !== link) return false;
  return index === parent.childCount - 1 || !link.isInSet(parent.child(index + 1).marks);
}

const serializer = new MarkdownSerializer(nodes, marks);

/** Texte d'une cellule de tableau, sur une ligne, avec les « | » échappés. */
function cellText(cell) {
  const state = new MarkdownSerializerState(nodes, marks, {});
  state.inTableCell = true;
  state.renderInline(cell, false);
  return state.out.replace(/\|/g, "\\|").replace(/\n/g, " ").trim();
}

/** Document -> Markdown. */
export function toMarkdown(doc) {
  return serializer.serialize(doc).replace(/\s+$/, "");
}

/** Morceau copié -> Markdown (presse-papiers en texte brut). */
export function sliceToMarkdown(slice) {
  try {
    const doc = schema.topNodeType.createAndFill(null, slice.content);
    if (doc) return toMarkdown(doc);
  } catch {
    // Morceau qui n'est pas un document complet (texte seul) : texte brut.
  }
  return slice.content.textBetween(0, slice.content.size, "\n\n");
}
