-- Barre de raccourcis du téléphone : les entrées choisies dans Réglages,
-- séparées par des virgules (« dashboard,notes,calendar,search »).
-- NULL = le choix par défaut (voir src/tabbar.rs).
ALTER TABLE users ADD COLUMN mobile_tabs TEXT;
