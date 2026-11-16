-- Espaces partagés (SPEC, « Espaces partagés »).
--
-- Le propriétaire d'un espace reste `spaces.user_id`. Il invite d'autres
-- comptes ; une fois l'invitation acceptée, ils deviennent membres et
-- partagent les notes, fichiers, dossiers et tâches de l'espace.
--
-- Changement de convention (voir 0001) : la colonne `user_id` des contenus
-- garde leur auteur, mais ne décide plus de l'accès. L'accès se décide par
-- l'espace, à travers la vue `space_access`. Les agendas, Temporaire, les
-- envois en cours et les conversions restent filtrés par compte.

-- Un membre qui n'est pas le propriétaire.
CREATE TABLE space_members (
    space_id  INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Identifiant d'adresse de l'espace chez ce membre (`/e/<slug>/`) : celui
    -- de l'espace, ou suffixé (« projet-2 ») s'il est déjà pris chez lui.
    -- Le code vérifie qu'il ne croise pas les slugs de ses propres espaces.
    slug      TEXT    NOT NULL,
    -- Place de l'espace dans l'île de ce membre.
    position  INTEGER NOT NULL,
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (space_id, user_id),
    UNIQUE (user_id, slug)
);

-- Invitation en attente : acceptée, elle devient une ligne de space_members ;
-- refusée ou annulée, elle disparaît.
CREATE TABLE space_invitations (
    id         INTEGER PRIMARY KEY,
    space_id   INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,  -- l'invité
    invited_by INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    UNIQUE (space_id, user_id)
);
CREATE INDEX space_invitations_by_user ON space_invitations (user_id);

-- Qui a accès à quel espace : son propriétaire et ses membres. Les requêtes
-- de contenu filtrent par `space_id IN (SELECT space_id FROM space_access
-- WHERE user_id = ?)`, qui garde le même paramètre que l'ancien
-- `user_id = ?`.
CREATE VIEW space_access (user_id, space_id) AS
    SELECT user_id, id FROM spaces
    UNION ALL
    SELECT user_id, space_id FROM space_members;

-- Garde-fou des notes modifiées à plusieurs : l'éditeur renvoie la version
-- qu'il a ouverte, et un enregistrement sur une version dépassée est refusé.
ALTER TABLE notes ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;

-- Les anciens index commencent par user_id, qui ne filtre plus le contenu.
CREATE INDEX notes_in_space ON notes (space_id, pinned DESC, updated_at DESC);
CREATE INDEX files_in_space ON files (space_id, updated_at DESC);
CREATE INDEX folders_in_space ON folders (space_id, parent_id);
CREATE INDEX tasks_in_space ON tasks (space_id);
