-- 1Life, module Clients (SPEC, « 1Life — Clients ») : le carnet de clients
-- d'un compte autorisé à 1Life (`tool_grants`).
--
-- Les clients sont au compte, pas à un espace : 1Life est un outil réservé,
-- ses clients le suivent dans tous les espaces où il est affiché. Le contrôle
-- d'accès est celui de 1Life (src/onelife), vérifié à chaque requête.
--
-- Tout est du texte brut, échappé à l'affichage : rien n'est interprété
-- (ni Markdown ni HTML). Les longueurs sont vérifiées par le code.
CREATE TABLE onelife_clients (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    contact    TEXT    NOT NULL DEFAULT '',
    email      TEXT    NOT NULL DEFAULT '',
    phone      TEXT    NOT NULL DEFAULT '',
    notes      TEXT    NOT NULL DEFAULT '',
    -- 1 : archivé, hors de la liste sans être effacé.
    archived   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Un nom ne se répète pas dans un compte (le code compare aussi les
-- majuscules accentuées et les accents, que NOCASE ignore).
CREATE UNIQUE INDEX onelife_clients_unique_name ON onelife_clients (user_id, name COLLATE NOCASE);
