-- Perso ne se supprime plus (src/spaces/mod.rs, OVERVIEW) : son agenda réunit
-- ceux de tous les espaces. La version précédente permettait de le supprimer :
-- un compte qui l'a fait le retrouve, vide, en tête de ses espaces. Un compte
-- sans aucun espace n'est pas concerné : ses espaces naissent à la connexion.
-- Si un autre espace s'appelle déjà « Perso », celui-ci prend un autre nom.
INSERT INTO spaces (user_id, slug, name, color, icon, position)
SELECT u.id,
       'perso',
       CASE WHEN EXISTS (SELECT 1 FROM spaces s WHERE s.user_id = u.id AND lower(s.name) = 'perso')
            THEN 'Vue d''ensemble' ELSE 'Perso' END,
       'orange',
       'home',
       (SELECT min(s.position) FROM spaces s WHERE s.user_id = u.id) - 1
FROM users u
WHERE EXISTS (SELECT 1 FROM spaces s WHERE s.user_id = u.id)
  AND NOT EXISTS (SELECT 1 FROM spaces s WHERE s.user_id = u.id AND s.slug = 'perso');
