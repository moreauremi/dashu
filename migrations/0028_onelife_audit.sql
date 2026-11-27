-- 1Life › Fiche client, réglages et Audit (SPEC, « 1Life — Fiche client et
-- réglages », « 1Life — Audit »).
--
-- Tout suit les dossiers et les notes : une ligne ne se lit que si son
-- dossier ou sa note se voit (`space_access`, qui cache la corbeille), et
-- l'effacement pour de bon emporte tout (`ON DELETE CASCADE`).

-- L'encart « Identité » d'un client : une valeur par champ du registre
-- (`src/onelife/fields.rs`). Une clé inconnue (champ retiré) est ignorée.
CREATE TABLE onelife_client_fields (
    folder_id  INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    PRIMARY KEY (folder_id, key)
);

-- Les réglages de 1Life, par compte. Un texte vide : la valeur par défaut
-- (le nom du profil pour le consultant, le modèle intégré, le premier
-- modèle d'IA de ONELIFE_AI_MODELS).
ALTER TABLE onelife_settings ADD COLUMN consultant_name TEXT NOT NULL DEFAULT '';
ALTER TABLE onelife_settings ADD COLUMN consultant_role TEXT NOT NULL DEFAULT '';
ALTER TABLE onelife_settings ADD COLUMN consultant_email TEXT NOT NULL DEFAULT '';
ALTER TABLE onelife_settings ADD COLUMN approver TEXT NOT NULL DEFAULT '';
ALTER TABLE onelife_settings ADD COLUMN templates_folder_id INTEGER REFERENCES folders (id) ON DELETE SET NULL;
-- `integre`, `note:<id>` ou `fichier:<id>`.
ALTER TABLE onelife_settings ADD COLUMN default_template TEXT NOT NULL DEFAULT '';
ALTER TABLE onelife_settings ADD COLUMN ai_model TEXT NOT NULL DEFAULT '';

-- Un audit : un client (son dossier) et la note où se prennent les notes.
-- Il est au compte qui l'a commencé. Son statut se déduit de ses versions.
CREATE TABLE onelife_audits (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    client_id  INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    notes_id   INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE,
    -- `integre`, `note:<id>` ou `fichier:<id>`.
    template   TEXT    NOT NULL DEFAULT 'integre',
    title      TEXT    NOT NULL,
    -- AAAA-MM-JJ
    audit_date TEXT    NOT NULL,
    approver   TEXT    NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);

CREATE INDEX onelife_audits_by_user ON onelife_audits (user_id, created_at DESC);
CREATE INDEX onelife_audits_by_notes ON onelife_audits (notes_id);

-- Une version générée : une note ordinaire, et ce que l'IA a coûté.
CREATE TABLE onelife_audit_versions (
    id            INTEGER PRIMARY KEY,
    audit_id      INTEGER NOT NULL REFERENCES onelife_audits (id) ON DELETE CASCADE,
    number        INTEGER NOT NULL,
    note_id       INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE,
    model         TEXT    NOT NULL,
    input_tokens  INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    -- Coût estimé, en millionièmes de dollar.
    cost_micro    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    UNIQUE (audit_id, number)
);

CREATE INDEX onelife_audit_versions_by_note ON onelife_audit_versions (note_id);

-- Les points non repris d'une version : ce que l'IA a trouvé dans les notes
-- sans le mettre dans le document. Jamais dans la note.
CREATE TABLE onelife_audit_points (
    id         INTEGER PRIMARY KEY,
    version_id INTEGER NOT NULL REFERENCES onelife_audit_versions (id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    text       TEXT    NOT NULL,
    done       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX onelife_audit_points_by_version ON onelife_audit_points (version_id, position);

-- Chaque demande envoyée à l'IA : l'indicateur « Génération… », le message
-- d'un échec, et la limite par jour (comptée même si l'audit est effacé).
CREATE TABLE onelife_audit_runs (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    audit_id    INTEGER REFERENCES onelife_audits (id) ON DELETE SET NULL,
    model       TEXT    NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'done', 'failed')),
    message     TEXT    NOT NULL DEFAULT '',
    version_id  INTEGER REFERENCES onelife_audit_versions (id) ON DELETE SET NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE INDEX onelife_audit_runs_by_user ON onelife_audit_runs (user_id, started_at);
-- Une génération à la fois par compte, même avec deux clics simultanés.
CREATE UNIQUE INDEX onelife_audit_runs_one_at_a_time ON onelife_audit_runs (user_id) WHERE status = 'running';
