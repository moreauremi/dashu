-- Partage d'un fichier par lien temporaire (SPEC v2, « Partage par lien »).
--
-- Le lien contient 32 octets aléatoires ; la base n'en garde que l'empreinte
-- SHA-256 : la lire ne permet pas de reconstituer un lien. Un lien vaut
-- jusqu'à `expires_at` (1 heure, 1 jour ou 7 jours), sauf révocation.
CREATE TABLE share_links (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    file_id    INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    token_hash TEXT    NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX share_links_by_file ON share_links (user_id, file_id);
