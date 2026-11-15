-- Tags et recherche dans les fichiers (SPEC v2, « Fichiers », modèle de
-- données) : les mêmes tags que les notes, et une recherche plein texte sur
-- le nom, tenue à jour par des déclencheurs comme celle des notes.

CREATE TABLE file_tags (
    file_id INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (file_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX file_tags_by_tag ON file_tags (tag_id);

-- « cours-vlan.pdf » se découpe en cours, vlan, pdf ; accents ignorés.
CREATE VIRTUAL TABLE files_fts USING fts5 (
    name,
    content = 'files',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER files_fts_insert AFTER INSERT ON files BEGIN
    INSERT INTO files_fts (rowid, name) VALUES (new.id, new.name);
END;
CREATE TRIGGER files_fts_delete AFTER DELETE ON files BEGIN
    INSERT INTO files_fts (files_fts, rowid, name) VALUES ('delete', old.id, old.name);
END;
CREATE TRIGGER files_fts_update AFTER UPDATE OF name ON files BEGIN
    INSERT INTO files_fts (files_fts, rowid, name) VALUES ('delete', old.id, old.name);
    INSERT INTO files_fts (rowid, name) VALUES (new.id, new.name);
END;

-- Les fichiers déjà là.
INSERT INTO files_fts (rowid, name) SELECT id, name FROM files;
