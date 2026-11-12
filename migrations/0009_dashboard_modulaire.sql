-- Dashboard modulaire : chaque espace garde la disposition de ses widgets,
-- sous une forme courte « courses:2x2,tasks:1x2,notes:2x1 » (type, largeur en
-- colonnes, hauteur en rangées, dans l'ordre). NULL : la disposition par
-- défaut (src/dashboard/layout.rs).
ALTER TABLE spaces ADD COLUMN dashboard TEXT;
