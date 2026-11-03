# dashu

Tableau de bord open source et auto-hébergeable qui réunit sur une seule interface la vie pro, la vie perso et la maison.

> **Projet en conception.** Rien n'est encore installable. Ce dépôt contient pour l'instant la page d'accueil de [dashu.dev](https://dashu.dev).

## L'idée

Chaque personne a un compte unique et passe d'une vue à l'autre :

- **Équipe** : classe, association ou petite entreprise qui partage un tableau commun (agenda, tâches, tableur, état des services).
- **Perso** : agenda, météo, actu, budget, notes. Visible par son seul propriétaire.
- **Maison** : lumières, chauffage, consommation, via Home Assistant. Actions protégées par la double authentification.
- **Générale** : un résumé des widgets épinglés depuis les autres vues.

Les widgets se branchent sur quelques connecteurs génériques plutôt que sur des intégrations codées une par une : ICS, CSV, API REST/JSON, RSS/Atom, ping HTTP, Home Assistant, webhook entrant (n8n).

## Feuille de route

| Version | Contenu |
| --- | --- |
| v1.0 | Comptes, équipes, vues, connecteurs en lecture, mode édition, bibliothèque de widgets |
| v1.1 | Home Assistant en lecture, webhook entrant |
| v1.2 | Actions sur la maison (2FA, journal, liste blanche d'appareils) |
| v2 | OAuth Google et Microsoft, plugins, catalogue de modèles et de thèmes |

## Contenu du dépôt

```
website/                     page d'accueil statique de dashu.dev (HTML + CSS, sans dépendance)
.github/workflows/pages.yml  publication automatique sur GitHub Pages
```

Pour voir la page en local :

```bash
python3 -m http.server -d website 8000
# puis http://localhost:8000
```

## Licence

À choisir avant la première version (MIT ou AGPL).
