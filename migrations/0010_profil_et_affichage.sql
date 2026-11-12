-- Profil et affichage, réglés par chaque utilisateur.

-- Prénom et nom, affichés dans la barre latérale ; vides au départ (l'email
-- les remplace).
ALTER TABLE users ADD COLUMN first_name TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN last_name TEXT NOT NULL DEFAULT '';

-- Fond de l'avatar par défaut (les initiales) : un accent tiré au hasard une
-- fois pour toutes. Vide : pas encore tiré (src/auth/session.rs s'en charge).
ALTER TABLE users ADD COLUMN avatar_color TEXT NOT NULL DEFAULT '';

-- Photo de profil : JPEG carré de 256 px, NULL = les initiales. Le numéro de
-- version change à chaque photo : l'adresse de l'image change avec lui, et
-- le navigateur ne ressert pas l'ancienne de son cache.
ALTER TABLE users ADD COLUMN avatar BLOB;
ALTER TABLE users ADD COLUMN avatar_version INTEGER NOT NULL DEFAULT 0;

-- Taille de l'interface : 'small', 'large', NULL = normale.
ALTER TABLE users ADD COLUMN ui_size TEXT;

-- Barre latérale détachée : cachée à gauche, elle apparaît au survol du bord.
ALTER TABLE users ADD COLUMN sidebar_floating BOOLEAN NOT NULL DEFAULT 0;
