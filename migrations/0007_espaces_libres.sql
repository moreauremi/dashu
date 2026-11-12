-- Espaces libres : on peut en créer, en renommer et en supprimer.
--
-- La table spaces limitait le slug à perso, ecole et taff, et la couleur à
-- trois accents (contraintes CHECK). SQLite ne sait pas retirer une contrainte :
-- on reconstruit la table selon la procédure de sa documentation
-- (https://www.sqlite.org/lang_altertable.html#otheralter). Les valeurs
-- permises sont désormais vérifiées par le code (src/spaces/mod.rs). Une
-- colonne icon s'ajoute : chaque espace choisit la sienne.
--
-- La reconstruction exige des clés étrangères désactivées, sinon DROP TABLE
-- spaces effacerait en cascade les notes, tâches, agendas et fichiers. dashu
-- lance donc ses migrations sur une connexion sans clés étrangères (src/db.rs).
-- Garde-fou pour tout autre outil (sqlx-cli les active) : sur une base qui a
-- déjà des espaces, la migration échoue au lieu de tout effacer.
CREATE TEMP TABLE garde_0007 (
    fk INTEGER CONSTRAINT "0007 : clés étrangères actives, migration refusée (voir src/db.rs)" CHECK (fk = 0)
);
INSERT INTO garde_0007 SELECT foreign_keys FROM pragma_foreign_keys WHERE EXISTS (SELECT 1 FROM spaces);
DROP TABLE garde_0007;

CREATE TABLE spaces_new (
    id       INTEGER PRIMARY KEY,
    user_id  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    slug     TEXT    NOT NULL,  -- identifiant court (API, session), gardé quand l'espace est renommé
    name     TEXT    NOT NULL,
    color    TEXT    NOT NULL,  -- nom d'un accent du design system
    icon     TEXT    NOT NULL,  -- nom d'une icône du sprite
    position INTEGER NOT NULL,
    UNIQUE (user_id, slug)
);
INSERT INTO spaces_new (id, user_id, slug, name, color, icon, position)
SELECT id, user_id, slug, name, color,
       CASE slug WHEN 'ecole' THEN 'school' WHEN 'taff' THEN 'work' ELSE 'home' END,
       position
FROM spaces;
DROP TABLE spaces;
ALTER TABLE spaces_new RENAME TO spaces;
