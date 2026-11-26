-- Outils réservés (SPEC, « Outils par espace », PR 2) : un outil du
-- registre marqué `reserved` (1Life) n'existe que pour les comptes autorisés
-- ici. Seule la commande `dashu tools grant|revoke` écrit dans cette table :
-- aucune route HTTP, ni l'API, ni le connecteur.
CREATE TABLE tool_grants (
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    tool       TEXT    NOT NULL,  -- identifiant du registre (`1life`)
    granted_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, tool)
) WITHOUT ROWID;
