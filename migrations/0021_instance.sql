-- Réglages de l'instance (SPEC, « Installation simple », étapes 4 et 5).
--
-- Une installation par l'assistant du premier lancement (ou par l'image
-- Docker) n'a pas de dashu.env à remplir : l'adresse publique, la connexion
-- Google et les adresses autorisées sont rangées ici, et se changent ensuite
-- dans Réglages › Instance. Les variables d'environnement gardent la
-- priorité : un serveur configuré par fichier ne voit aucun changement.

-- Clé et valeur : `base_url`, `google_client_id`, `google_client_secret`.
-- Le secret Google est chiffré avec la clé tirée du secret de session (comme
-- les jetons de Google Agenda), en hexadécimal.
CREATE TABLE instance_settings (
    key        TEXT    PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Adresses autorisées à se connecter, quand ALLOWED_EMAILS n'est pas posée.
CREATE TABLE allowed_emails (
    email    TEXT    PRIMARY KEY COLLATE NOCASE,
    added_at INTEGER NOT NULL,
    added_by INTEGER REFERENCES users (id) ON DELETE SET NULL
);

-- L'administrateur de l'instance : le compte créé par l'assistant. Sur une
-- base existante, le plus ancien compte.
ALTER TABLE users ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0;
UPDATE users SET is_admin = 1 WHERE id = (SELECT MIN(id) FROM users);
