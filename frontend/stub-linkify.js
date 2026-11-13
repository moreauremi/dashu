// Remplace « linkify-it » dans le bundle de l'éditeur : markdown-it en crée
// une instance, mais l'éditeur ne transforme pas les adresses nues en liens
// (option linkify désactivée, comme pulldown-cmark côté serveur).
export default class LinkifyIt {
  test() {
    return false;
  }
  pretest() {
    return false;
  }
  match() {
    return null;
  }
  matchAtStart() {
    return null;
  }
}
