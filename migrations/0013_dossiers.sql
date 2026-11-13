-- Dossiers (SPEC, « Dossiers ») : ils rangent les fichiers et les notes.
--
-- Ils n'existent que dans la base : sur le disque, un fichier garde son nom
-- aléatoire, quel que soit son dossier. Le dossier d'un espace n'est pas une
-- ligne : c'est l'espace lui-même (créé, renommé, supprimé avec lui).
--
-- Trois niveaux au plus, vérifiés par le code (src/folders/mod.rs) :
-- Espace / Dossier (`parent_id` vide) / Sous-dossier (`parent_id` = un
-- dossier du premier niveau). Un dossier et son contenu sont toujours dans
-- le même espace.
CREATE TABLE folders (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id   INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    parent_id  INTEGER REFERENCES folders (id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX folders_by_space ON folders (user_id, space_id, parent_id);
-- Deux voisins ne portent pas le même nom (le code compare aussi les
-- majuscules accentuées, que NOCASE ignore).
CREATE UNIQUE INDEX folders_unique_name ON folders (space_id, ifnull(parent_id, 0), name COLLATE NOCASE);

-- Dossier d'une note, d'un fichier, d'un envoi en cours. Vide : la racine de
-- l'espace. Un dossier supprimé sans qu'on ait rangé son contenu (cas qui ne
-- devrait pas arriver, le code le range avant) laisse ce contenu à la racine.
ALTER TABLE notes ADD COLUMN folder_id INTEGER REFERENCES folders (id) ON DELETE SET NULL;
ALTER TABLE files ADD COLUMN folder_id INTEGER REFERENCES folders (id) ON DELETE SET NULL;
ALTER TABLE uploads ADD COLUMN folder_id INTEGER REFERENCES folders (id) ON DELETE SET NULL;

CREATE INDEX notes_by_folder ON notes (folder_id) WHERE folder_id IS NOT NULL;
CREATE INDEX files_by_folder ON files (folder_id) WHERE folder_id IS NOT NULL;
