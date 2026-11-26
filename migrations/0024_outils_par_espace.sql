-- Outils par espace (SPEC, « Outils par espace ») : chaque compte choisit,
-- espace par espace, les outils de la barre latérale (src/registry.rs).
--
-- Pas de ligne : la valeur par défaut du registre. La migration n'écrit
-- rien, donc tous les outils actuels restent affichés partout. Un réglage
-- d'affichage, pas une permission : aucun accès n'en dépend.
CREATE TABLE space_tools (
    user_id  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    space_id INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    tool     TEXT    NOT NULL,  -- identifiant du registre (`files`, `notes`…)
    shown    INTEGER NOT NULL CHECK (shown IN (0, 1)),
    PRIMARY KEY (user_id, space_id, tool)
) WITHOUT ROWID;
