-- Fichiers (v2) : métadonnées en base, contenu sur le disque.
--
-- Le contenu vit dans FILES_DIR, sous un nom aléatoire (`storage_key`), jamais
-- sous le nom choisi par l'utilisateur. Supprimer la ligne ne supprime pas le
-- fichier du disque : c'est le handler de suppression qui s'en charge.
CREATE TABLE files (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id    INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    mime        TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    sha256      TEXT    NOT NULL,
    storage_key TEXT    NOT NULL UNIQUE,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX files_by_space ON files (user_id, space_id, updated_at DESC);
