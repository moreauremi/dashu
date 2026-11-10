// Remplace @codemirror/lang-html dans le bundle de l'éditeur.
//
// @codemirror/lang-markdown importe lang-html (qui tire à son tour les
// grammaires CSS et JavaScript) pour colorer le HTML écrit au milieu du
// Markdown. dashu n'en a pas besoin et ça triplait le poids de la page
// éditeur : on fournit donc la plus petite forme que markdown() accepte.
export const html = () => ({ support: [], language: { parser: null } });
export const htmlCompletionSource = () => null;
