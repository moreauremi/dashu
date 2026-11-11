-- Envois en cours, par morceaux (v2).
--
-- Un fichier arrive en morceaux de 50 Mo au plus : Cloudflare refuse les
-- requêtes de plus de 100 Mo. Les morceaux s'ajoutent, dans l'ordre, au
-- fichier partiel `FILES_DIR/.envois/<id>.part` ; `received` dit combien
-- d'octets sont déjà là. Une fois complet, le fichier rejoint `files` et la
-- ligne disparaît. Un envoi sans nouvelles depuis 24 heures est effacé.
CREATE TABLE uploads (
    id         TEXT    PRIMARY KEY,   -- 64 caractères aléatoires
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id   INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    size       INTEGER NOT NULL,      -- taille annoncée au départ
    received   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX uploads_by_age ON uploads (updated_at);
