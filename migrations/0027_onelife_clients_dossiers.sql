-- 1Life › Clients sur les dossiers (SPEC, « 1Life — Clients ») : un client
-- est un sous-dossier direct du dossier des clients choisi par le compte.
-- Rien n'est copié : le nom, le contenu et la corbeille sont ceux du dossier.
--
-- Remplace le carnet de la migration 0026 : la table `onelife_clients` n'est
-- plus lue, mais reste en base pour ne rien perdre de ce qui y a été saisi.

-- Le dossier des clients de chaque compte. Effacé pour de bon, le dossier
-- vide le choix ; pendant son séjour en corbeille, le choix reste et le
-- module dit que le dossier n'est plus accessible.
CREATE TABLE onelife_settings (
    user_id           INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    clients_folder_id INTEGER REFERENCES folders (id) ON DELETE SET NULL,
    updated_at        INTEGER NOT NULL
);

-- Les infos d'un client, en Markdown (rendues comme les notes : pulldown-cmark
-- puis ammonia). Elles restent pendant le séjour du dossier en corbeille et
-- partent avec lui s'il est effacé pour de bon.
CREATE TABLE onelife_client_infos (
    folder_id  INTEGER PRIMARY KEY REFERENCES folders (id) ON DELETE CASCADE,
    body_md    TEXT    NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL,
    updated_by INTEGER REFERENCES users (id) ON DELETE SET NULL
);

-- Une journée chez un client : un événement de l'Agenda (`events`, qui garde
-- la date, les horaires et l'envoi à Google), relié ici à son client.
-- L'événement supprimé emporte le lien ; le dossier effacé pour de bon aussi,
-- et l'événement redevient un événement ordinaire.
CREATE TABLE onelife_client_days (
    event_id   INTEGER PRIMARY KEY REFERENCES events (id) ON DELETE CASCADE,
    folder_id  INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    label      TEXT    NOT NULL,
    place_kind TEXT    NOT NULL DEFAULT 'site' CHECK (place_kind IN ('site', 'distance')),
    place      TEXT    NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);

CREATE INDEX onelife_client_days_by_folder ON onelife_client_days (folder_id);
