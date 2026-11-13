-- Onglet Outils : un fichier déposé là n'est pas encore rangé. Il garde un
-- espace (obligatoire), mais n'apparaît dans aucune liste tant qu'on ne l'a
-- pas gardé, et il est effacé à la date `expires_at` (24 heures après son
-- arrivée). NULL : un fichier rangé, comme tous ceux d'avant.
ALTER TABLE files ADD COLUMN expires_at INTEGER;
CREATE INDEX files_expiring ON files (expires_at) WHERE expires_at IS NOT NULL;

-- L'envoi d'un fichier vers l'onglet Outils.
ALTER TABLE uploads ADD COLUMN for_tools INTEGER NOT NULL DEFAULT 0;

-- Conversion demandée par l'utilisateur (`convert`), en plus de la
-- compression et des vignettes : la sortie choisie (`jpg`, `mp3`…) et, pour
-- un PDF, la plage de pages.
ALTER TABLE jobs ADD COLUMN target TEXT;
