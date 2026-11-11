-- Tâches de l'onglet Outils (v2) : pour l'instant, la compression d'un fichier.
--
-- dashu ne lance aucune conversion lui-même. Il prépare un dossier
-- `WORK_DIR/jobs/<id>/` (une copie du fichier et `job.json`), puis le service
-- `dashu-worker`, sans réseau et sans accès à la base, fait le travail et
-- dépose le résultat. dashu le relève et met cette ligne à jour.
CREATE TABLE jobs (
    id             INTEGER PRIMARY KEY,
    user_id        INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id       INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    kind           TEXT    NOT NULL,                   -- 'compress'
    source_name    TEXT    NOT NULL,                   -- nom du fichier de départ
    source_size    INTEGER NOT NULL,
    status         TEXT    NOT NULL DEFAULT 'queued',  -- queued, running, done, failed
    progress       INTEGER NOT NULL DEFAULT 0,         -- 0 à 100
    error          TEXT,
    result_file_id INTEGER REFERENCES files (id) ON DELETE SET NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE INDEX jobs_by_status ON jobs (status);
CREATE INDEX jobs_by_user ON jobs (user_id, updated_at DESC);
