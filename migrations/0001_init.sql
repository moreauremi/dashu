-- Schéma initial de dashu v1.
--
-- Conventions :
--   * Tous les instants sont des entiers Unix (secondes, UTC). L'affichage en
--     Europe/Paris se fait côté Rust (src/time.rs).
--   * Chaque contenu porte un user_id : toutes les requêtes filtrent dessus.
--   * La table des sessions est créée à part par tower-sessions (src/db.rs).

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    email         TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT,            -- NULL tant qu'aucun mot de passe n'est défini (compte créé via Google)
    google_sub    TEXT UNIQUE,     -- identifiant stable du compte Google lié, NULL sinon
    theme         TEXT CHECK (theme IN ('dark', 'light')),  -- NULL = suivre le système
    created_at    INTEGER NOT NULL
);

CREATE TABLE spaces (
    id       INTEGER PRIMARY KEY,
    user_id  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    slug     TEXT    NOT NULL CHECK (slug IN ('perso', 'ecole', 'taff')),
    name     TEXT    NOT NULL,
    color    TEXT    NOT NULL CHECK (color IN ('orange', 'blue', 'green')),  -- nom d'un accent du design system
    position INTEGER NOT NULL,
    UNIQUE (user_id, slug)
);

CREATE TABLE notes (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id   INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    title      TEXT    NOT NULL DEFAULT '',
    body_md    TEXT    NOT NULL DEFAULT '',   -- source Markdown ; le HTML est généré à l'affichage
    pinned     BOOLEAN NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX notes_by_space ON notes (user_id, space_id, pinned DESC, updated_at DESC);

-- Recherche plein texte. Table « external content » : FTS5 n'y stocke que
-- l'index, le texte reste dans notes. remove_diacritics : « eleve » trouve « élève ».
CREATE VIRTUAL TABLE notes_fts USING fts5 (
    title,
    body_md,
    content = 'notes',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER notes_fts_insert AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts (rowid, title, body_md) VALUES (new.id, new.title, new.body_md);
END;

CREATE TRIGGER notes_fts_delete AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts (notes_fts, rowid, title, body_md) VALUES ('delete', old.id, old.title, old.body_md);
END;

CREATE TRIGGER notes_fts_update AFTER UPDATE OF title, body_md ON notes BEGIN
    INSERT INTO notes_fts (notes_fts, rowid, title, body_md) VALUES ('delete', old.id, old.title, old.body_md);
    INSERT INTO notes_fts (rowid, title, body_md) VALUES (new.id, new.title, new.body_md);
END;

CREATE TABLE tags (
    id      INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name    TEXT    NOT NULL COLLATE NOCASE,
    UNIQUE (user_id, name)
);

CREATE TABLE note_tags (
    note_id INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (note_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX note_tags_by_tag ON note_tags (tag_id);

CREATE TABLE tasks (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id   INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    title      TEXT    NOT NULL,
    done       BOOLEAN NOT NULL DEFAULT 0,
    due_date   TEXT,               -- jour calendaire à Paris, 'AAAA-MM-JJ' (un jour, pas un instant)
    done_at    INTEGER,            -- pour garder une tâche cochée visible jusqu'à la fin de la journée
    created_at INTEGER NOT NULL
);
CREATE INDEX tasks_by_space ON tasks (user_id, space_id, due_date);

CREATE TABLE calendar_sources (
    id              INTEGER PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id        INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    name            TEXT    NOT NULL,
    ical_url        TEXT    NOT NULL,  -- secret : jamais renvoyée en entier au navigateur
    last_fetched_at INTEGER,           -- dernière synchro réussie
    last_attempt_at INTEGER,           -- dernière tentative, réussie ou non
    last_error      TEXT,              -- NULL si la dernière tentative a réussi
    created_at      INTEGER NOT NULL
);

-- Cache local des événements : entièrement remplacé à chaque synchro réussie.
CREATE TABLE calendar_events (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES calendar_sources (id) ON DELETE CASCADE,
    uid       TEXT    NOT NULL,
    title     TEXT    NOT NULL,
    location  TEXT    NOT NULL DEFAULT '',
    starts_at INTEGER NOT NULL,
    ends_at   INTEGER NOT NULL,
    all_day   BOOLEAN NOT NULL DEFAULT 0
);
CREATE INDEX calendar_events_by_time ON calendar_events (source_id, starts_at);
