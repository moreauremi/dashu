-- Corbeille (SPEC, « API complète et corbeille ») : supprimer une note, un
-- fichier, un dossier ou un espace le met de côté 30 jours au lieu de
-- l'effacer.
--
-- Une note, un fichier ou un dossier supprimé passe dans l'espace
-- « corbeille » de son espace : un espace caché (`trash_of`), que la vue
-- `space_access` ne donne à personne. Toutes les requêtes de contenu passent
-- par cette vue : ce qui est dans la corbeille disparaît donc de partout
-- (listes, recherche, barre de commandes, Perso, widgets, API) sans qu'aucune
-- d'elles ait à le savoir. Un espace supprimé, lui, reste en place, marqué
-- `deleted_at`, et la vue le cache avec tout son contenu.

-- L'espace corbeille d'un espace (un au plus), effacé avec lui.
ALTER TABLE spaces ADD COLUMN trash_of INTEGER REFERENCES spaces (id) ON DELETE CASCADE;
-- Un espace supprimé : quand, par qui, par où (« le site », ou le nom du
-- jeton ou de l'application).
ALTER TABLE spaces ADD COLUMN deleted_at INTEGER;
ALTER TABLE spaces ADD COLUMN deleted_by INTEGER;
ALTER TABLE spaces ADD COLUMN deleted_via TEXT;
CREATE UNIQUE INDEX spaces_trash ON spaces (trash_of) WHERE trash_of IS NOT NULL;

-- Une note, un fichier ou un dossier dans la corbeille :
--   * deleted_at, deleted_by, deleted_via : quand, qui, par où ;
--   * trash_op : numéro de l'opération ; un dossier supprimé et tout ce qu'il
--     contenait portent le même, et se restaurent ensemble ;
--   * trash_root : 1 pour ce que la personne a supprimé (une ligne de la page
--     Corbeille), 0 pour ce qui est parti avec un dossier ;
--   * trash_folder_id : le dossier d'où vient l'élément supprimé seul (son
--     `folder_id` est vidé : le dossier, lui, est resté en place).
ALTER TABLE notes ADD COLUMN deleted_at INTEGER;
ALTER TABLE notes ADD COLUMN deleted_by INTEGER;
ALTER TABLE notes ADD COLUMN deleted_via TEXT;
ALTER TABLE notes ADD COLUMN trash_op INTEGER;
ALTER TABLE notes ADD COLUMN trash_root INTEGER NOT NULL DEFAULT 0;
ALTER TABLE notes ADD COLUMN trash_folder_id INTEGER;

ALTER TABLE files ADD COLUMN deleted_at INTEGER;
ALTER TABLE files ADD COLUMN deleted_by INTEGER;
ALTER TABLE files ADD COLUMN deleted_via TEXT;
ALTER TABLE files ADD COLUMN trash_op INTEGER;
ALTER TABLE files ADD COLUMN trash_root INTEGER NOT NULL DEFAULT 0;
ALTER TABLE files ADD COLUMN trash_folder_id INTEGER;

ALTER TABLE folders ADD COLUMN deleted_at INTEGER;
ALTER TABLE folders ADD COLUMN deleted_by INTEGER;
ALTER TABLE folders ADD COLUMN deleted_via TEXT;
ALTER TABLE folders ADD COLUMN trash_op INTEGER;
ALTER TABLE folders ADD COLUMN trash_root INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN trash_folder_id INTEGER;
-- Deux dossiers « Math » peuvent être dans la même corbeille : le nom d'un
-- dossier y est mis de côté ici, et remplacé par un nom unique (les noms sont
-- uniques par emplacement, index folders_unique_name).
ALTER TABLE folders ADD COLUMN trash_name TEXT;

CREATE INDEX notes_in_trash ON notes (deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX files_in_trash ON files (deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX folders_in_trash ON folders (deleted_at) WHERE deleted_at IS NOT NULL;

-- L'accès au contenu : ni les espaces corbeille, ni les espaces supprimés.
DROP VIEW space_access;
CREATE VIEW space_access (user_id, space_id) AS
    SELECT user_id, id FROM spaces
     WHERE trash_of IS NULL AND deleted_at IS NULL
    UNION ALL
    SELECT m.user_id, m.space_id FROM space_members m
      JOIN spaces s ON s.id = m.space_id
     WHERE s.deleted_at IS NULL;

-- L'accès à la corbeille : celle de chaque espace accessible. Dans un espace
-- partagé, elle est commune à tous ses membres.
CREATE VIEW trash_access (user_id, space_id, trash_id) AS
    SELECT a.user_id, a.space_id, t.id
      FROM space_access a JOIN spaces t ON t.trash_of = a.space_id;

-- L'agenda : rien d'un espace supprimé (ses agendas iCal et ses événements
-- reviennent avec lui s'il est restauré).
DROP VIEW agenda_items;
CREATE VIEW agenda_items (user_id, space_id, title, location, starts_at, ends_at, all_day, event_id) AS
    SELECT s.user_id, s.space_id, e.title, e.location, e.starts_at, e.ends_at, e.all_day, NULL
    FROM calendar_events e
    JOIN calendar_sources s ON s.id = e.source_id
    WHERE NOT EXISTS (
        SELECT 1 FROM events ev
        WHERE ev.user_id = s.user_id AND ev.google_event_id IS NOT NULL
          AND ev.google_event_id || '@google.com' = e.uid
    )
      AND s.space_id NOT IN (SELECT id FROM spaces WHERE deleted_at IS NOT NULL)
    UNION ALL
    SELECT user_id, space_id, title, location, starts_at, ends_at, all_day, id
    FROM events
    WHERE space_id NOT IN (SELECT id FROM spaces WHERE deleted_at IS NOT NULL);
