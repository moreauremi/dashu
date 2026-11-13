// Remplace le paquet « entities » dans le bundle de l'éditeur (voir
// scripts/build-assets.mjs). markdown-it n'en utilise que decodeHTML (et sa
// variante stricte), pour lire les entités nommées comme « &eacute; ». Le
// paquet complet en connaît plus de 2 000 (75 Ko) ; une note n'en contient
// presque jamais, on garde les courantes. Une entité inconnue reste écrite
// telle quelle dans l'éditeur et dans le Markdown enregistré ; le serveur la
// décode toujours à l'affichage. Les entités numériques (« &#233; ») sont
// lues par markdown-it lui-même.

const NAMED = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  shy: "­",
  laquo: "«",
  raquo: "»",
  lsquo: "‘",
  rsquo: "’",
  ldquo: "“",
  rdquo: "”",
  hellip: "…",
  ndash: "–",
  mdash: "—",
  middot: "·",
  bull: "•",
  deg: "°",
  euro: "€",
  copy: "©",
  reg: "®",
  trade: "™",
  times: "×",
  divide: "÷",
  plusmn: "±",
  le: "≤",
  ge: "≥",
  ne: "≠",
  larr: "←",
  rarr: "→",
  uarr: "↑",
  darr: "↓",
  harr: "↔",
  agrave: "à",
  acirc: "â",
  ccedil: "ç",
  eacute: "é",
  egrave: "è",
  ecirc: "ê",
  euml: "ë",
  icirc: "î",
  iuml: "ï",
  ocirc: "ô",
  ugrave: "ù",
  ucirc: "û",
  uuml: "ü",
  oelig: "œ",
  Agrave: "À",
  Ccedil: "Ç",
  Eacute: "É",
  Egrave: "È",
  Ecirc: "Ê",
  OElig: "Œ",
};

export function decodeHTML(text) {
  return text.replace(/&([a-z][a-z0-9]*);/gi, (match, name) => NAMED[name] ?? match);
}

/** Version stricte (entité terminée par « ; ») : la même ici. */
export const decodeHTMLStrict = decodeHTML;
