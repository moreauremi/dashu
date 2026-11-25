-- Téléchargeur de liens (SPEC v2, « Outils : téléchargeur de liens »).
--
-- Un fichier venu d'un lien YouTube, Instagram ou TikTok est marqué : il ne
-- se partage jamais par lien, et une conversion de ce fichier hérite de la
-- marque. Les tâches du téléchargeur sont des lignes de `jobs` comme les
-- autres (kind = 'download', target = 'mp4' ou 'mp3', sans fichier de départ).
ALTER TABLE files ADD COLUMN from_link INTEGER NOT NULL DEFAULT 0;
