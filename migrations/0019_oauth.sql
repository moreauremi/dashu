-- OAuth 2.1 pour le connecteur Claude (SPEC, « Connecteur Claude (MCP) »,
-- étape 3) : claude.ai, l'app iPhone et Claude Code s'autorisent depuis une
-- page de dashu, sans jeton à coller.
--
-- Codes et jetons de renouvellement font 32 octets aléatoires ; la base n'en
-- garde que l'empreinte SHA-256. Les jetons d'accès vont dans `access_tokens`
-- (migration 0018), avec `grant_id`.

-- Clients enregistrés (RFC 7591). Un client public, sans secret ; ses
-- adresses de retour sont un tableau JSON, limitées à celles de Claude. Un
-- client jamais autorisé est effacé au bout de 24 heures.
CREATE TABLE oauth_clients (
    client_id     TEXT    PRIMARY KEY,
    name          TEXT    NOT NULL,
    redirect_uris TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
);

-- Codes d'autorisation : 60 secondes, à usage unique, liés au client, à
-- l'adresse de retour, au compte et au défi PKCE.
CREATE TABLE oauth_codes (
    code_hash      TEXT    PRIMARY KEY,
    client_id      TEXT    NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    user_id        INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    redirect_uri   TEXT    NOT NULL,
    code_challenge TEXT    NOT NULL,
    expires_at     INTEGER NOT NULL
);

-- Une autorisation donnée à Claude, révocable dans Réglages › Connexions.
-- Le jeton de renouvellement change à chaque usage ; l'ancien est gardé :
-- s'il revient, c'est qu'il a été copié, et toute l'autorisation tombe.
CREATE TABLE oauth_grants (
    id                    INTEGER PRIMARY KEY,
    user_id               INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    client_id             TEXT    NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    refresh_hash          TEXT    NOT NULL UNIQUE,
    previous_refresh_hash TEXT,
    created_at            INTEGER NOT NULL,
    last_used_at          INTEGER,
    expires_at            INTEGER NOT NULL
);

CREATE INDEX oauth_grants_by_user ON oauth_grants (user_id);
CREATE INDEX oauth_grants_by_previous ON oauth_grants (previous_refresh_hash);

-- Supprimer une autorisation supprime ses jetons d'accès (`access_tokens`
-- n'a pas de clé étrangère vers cette table, créée après elle).
CREATE TRIGGER oauth_grants_tokens AFTER DELETE ON oauth_grants
BEGIN
    DELETE FROM access_tokens WHERE grant_id = OLD.id;
END;
