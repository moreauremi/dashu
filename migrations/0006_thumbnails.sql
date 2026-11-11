-- Vignettes et aperçus des fichiers (v2, étape 3).
--
-- Fabriqués par le service de conversion (tâche « thumbnail »), rangés à côté
-- du fichier : `<clé>.thumb.jpg` (256 px, la vignette de la liste) et
-- `<clé>.preview.jpg` (grand aperçu d'une photo HEIC ou d'un PDF, image d'une
-- vidéo avant lecture).
--   thumb   : 0 pas encore, 1 prête, -1 impossible (type sans vignette, échec)
--   preview : 0 aucun, 1 prêt
ALTER TABLE files ADD COLUMN thumb INTEGER NOT NULL DEFAULT 0;
ALTER TABLE files ADD COLUMN preview INTEGER NOT NULL DEFAULT 0;

-- Fichier concerné par une tâche (vignette) : la tâche disparaît avec lui.
ALTER TABLE jobs ADD COLUMN file_id INTEGER REFERENCES files (id) ON DELETE CASCADE;
