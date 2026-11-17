-- Événements créés dans dashu, et leur copie dans Google Agenda (SPEC,
-- « Événements et Google Agenda »).
--
-- Comme les agendas iCal, un événement est à son auteur (`user_id`), même dans
-- un espace partagé : l'accès se décide par `user_id`, pas par space_access.

CREATE TABLE events (
    id              INTEGER PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id        INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    title           TEXT    NOT NULL,
    location        TEXT    NOT NULL DEFAULT '',
    starts_at       INTEGER NOT NULL,
    ends_at         INTEGER NOT NULL,
    all_day         INTEGER NOT NULL DEFAULT 0,
    -- À envoyer à Google Agenda (choix de l'utilisateur dans le formulaire).
    google          INTEGER NOT NULL DEFAULT 0,
    -- Identifiant de la copie dans Google, une fois créée.
    google_event_id TEXT,
    -- Changements pas encore arrivés chez Google : chaque modification
    -- l'augmente de 1, un envoi réussi retire ce qu'il a envoyé (une
    -- modification faite pendant l'envoi repart donc au suivant).
    google_dirty    INTEGER NOT NULL DEFAULT 0,
    -- Dernière erreur de Google pour cet événement, NULL si tout va bien.
    google_error    TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX events_by_user ON events (user_id, starts_at);
CREATE INDEX events_to_push ON events (user_id) WHERE google_dirty > 0;

-- Un agenda Google relié à un compte dashu. Le jeton de rafraîchissement
-- (refresh token) est chiffré : nonce de 12 octets suivi du texte chiffré
-- (ChaCha20-Poly1305, clé dérivée de SESSION_SECRET, voir
-- src/calendar/google.rs).
CREATE TABLE google_accounts (
    user_id       INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    email         TEXT    NOT NULL,
    refresh_token BLOB    NOT NULL,
    connected_at  INTEGER NOT NULL,
    last_error    TEXT
);

-- Copies à supprimer chez Google : l'événement n'existe plus dans dashu, mais
-- Google n'a pas encore confirmé la suppression.
CREATE TABLE google_deletions (
    user_id         INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    google_event_id TEXT    NOT NULL,
    PRIMARY KEY (user_id, google_event_id)
);

-- Tout ce qu'affiche l'agenda : les cours des flux iCal et les événements de
-- dashu. Un événement envoyé à Google par dashu revient dans le flux iCal de
-- l'agenda Google avec l'UID « <id Google>@google.com » : on ne le montre
-- qu'une fois. `event_id` : l'événement de dashu, NULL pour un cours iCal.
CREATE VIEW agenda_items (user_id, space_id, title, location, starts_at, ends_at, all_day, event_id) AS
    SELECT s.user_id, s.space_id, e.title, e.location, e.starts_at, e.ends_at, e.all_day, NULL
    FROM calendar_events e
    JOIN calendar_sources s ON s.id = e.source_id
    WHERE NOT EXISTS (
        SELECT 1 FROM events ev
        WHERE ev.user_id = s.user_id AND ev.google_event_id IS NOT NULL
          AND ev.google_event_id || '@google.com' = e.uid
    )
    UNION ALL
    SELECT user_id, space_id, title, location, starts_at, ends_at, all_day, id
    FROM events;
