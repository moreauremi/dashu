-- Quota de stockage par compte (SPEC v2, « Quota de stockage »).
--
-- Plafond personnel d'un compte, en octets. NULL : la valeur par défaut du
-- serveur (`FILES_QUOTA`, 30 Gio sans elle). Changer cette valeur par défaut
-- s'applique donc à tous ceux qui n'ont pas de plafond à eux. Seule la
-- commande `dashu quota` écrit cette colonne : aucune page, aucune route de
-- l'API ni du connecteur Claude.
ALTER TABLE users ADD COLUMN quota_bytes INTEGER CHECK (quota_bytes IS NULL OR quota_bytes > 0);

-- Journal des changements de plafond, écrit par `dashu quota set` et
-- `dashu quota reset`. L'adresse est recopiée (pas de clé étrangère) : le
-- journal survit au compte. NULL : la valeur par défaut.
CREATE TABLE quota_changes (
    id           INTEGER PRIMARY KEY,
    email        TEXT    NOT NULL,
    before_bytes INTEGER,
    after_bytes  INTEGER,
    changed_at   INTEGER NOT NULL
);

-- L'usage d'un compte : la somme des tailles de ses fichiers, hors fichiers
-- de l'onglet Outils pas encore rangés (`expires_at`). Index couvrant : le
-- calcul ne lit pas la table.
CREATE INDEX files_quota ON files (user_id, size) WHERE expires_at IS NULL;
