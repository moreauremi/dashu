-- Espace archivé (fin d'année, fin de stage) : caché du sélecteur, des
-- listes et de la vue d'ensemble de Perso ; son contenu reste cherchable par
-- la palette, et il se restaure. NULL tant qu'il est en service.
ALTER TABLE spaces ADD COLUMN archived_at INTEGER;

-- Règles automatiques d'un espace, appliquées à l'arrivée d'un fichier :
-- vidéo convertie en MP4, photo HEIC doublée d'un JPEG. Désactivées par défaut.
ALTER TABLE spaces ADD COLUMN auto_mp4 INTEGER NOT NULL DEFAULT 0;
ALTER TABLE spaces ADD COLUMN auto_heic_jpeg INTEGER NOT NULL DEFAULT 0;

-- Réglage « espace du moment » : à l'ouverture de l'app, l'espace du cours en
-- cours ou du prochain cours du jour ; un jour sans cours, cet espace-ci.
-- NULL : réglage désactivé.
ALTER TABLE users ADD COLUMN moment_space INTEGER REFERENCES spaces (id) ON DELETE SET NULL;
