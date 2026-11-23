-- Jetons d'accès par compte (SPEC, « Connecteur Claude (MCP) », étape 1).
--
-- Un jeton agit au nom de son compte, avec exactement ses droits sur le site.
-- Il contient 32 octets aléatoires ; la base n'en garde que l'empreinte
-- SHA-256 : la lire ne permet pas de reconstituer un jeton. Un jeton expiré
-- est refusé, mais sa ligne reste (affichée « expiré ») jusqu'à ce qu'on la
-- supprime.
CREATE TABLE access_tokens (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- Autorisation OAuth donnée à Claude dont vient ce jeton (étape 3) ;
    -- NULL pour un jeton créé à la main dans Réglages › Connexions.
    grant_id     INTEGER,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    -- Réécrit au plus une fois par minute.
    last_used_at INTEGER
);

CREATE INDEX access_tokens_by_user ON access_tokens (user_id, created_at);
