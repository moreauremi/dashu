<p align="center">
  <img src=".github/logo-hermes.svg" alt="Hermes Dashboard" width="560">
</p>

<p align="center">
  <strong>Hermes Dashboard</strong> · tableau de bord, notes, emploi du temps et fichiers, auto-hébergés
</p>

<p align="center">
  <a href="https://dashu.dev">dashu.dev</a> ·
  <a href="https://github.com/moreauremi/dashu/tree/v1">le code (branche v1)</a> ·
  <a href="https://github.com/moreauremi/dashu/blob/v1/README.md">la documentation</a>
</p>

---

Hermes Dashboard réunit un tableau de bord, des notes, l'emploi du temps Pronote et des fichiers, rangés par espaces et par dossiers derrière une seule connexion. Il tient dans **un seul binaire Rust**, hébergé à la maison.

Le nom de code du projet reste **dashu** : c'est le nom du dépôt, du binaire et des commandes (`dashu set-password`, `dashu-update`…).

## Où en est le projet

- **v1 terminée et en service.** Tous les critères d'acceptation sont vérifiés, la plupart sur le serveur réel. L'application sert au quotidien, sur ordinateur et sur iPhone.
- **Ajouté depuis, le 12 novembre 2026** : espaces à la carte, dashboard modulaire (grille bento propre à chaque espace, widget Calendrier), profil avec photo, taille de l'interface et barre latérale détachable.
- **Ajouté le 13 novembre 2026** : barre de commandes `/`, éditeur de notes en une seule vue (façon Notion), Perso qui montre tous les espaces, espace dans l'adresse, archivage d'un espace.
- **v2 en cours : les fichiers.** En service : l'envoi, l'aperçu, la compression et les conversions, l'onglet Outils, les images dans les notes et les dossiers, puis, depuis le 15 novembre 2026, les tags et la recherche, le partage par lien temporaire, la sauvegarde nocturne des fichiers sur l'autre disque et la conversion des documents. Reste : renommer un fichier, et les fichiers dans l'API de l'assistant (voir la [feuille de route](#feuille-de-route)).

Le détail, critère par critère, est dans [STATUS.md](https://github.com/moreauremi/dashu/blob/v1/STATUS.md). Le cahier des charges est dans [SPEC.md](https://github.com/moreauremi/dashu/blob/v1/SPEC.md).

## Ce que ça fait

| Module | Contenu |
| --- | --- |
| Espaces | Le contenu est rangé par espaces (par exemple Perso, École, Entreprise), chacun avec sa couleur et son icône, jusqu'à 8 en tout. On passe de l'un à l'autre en un clic, ou par balayage sur téléphone. Perso sert de vue d'ensemble : son dashboard, ses notes et son agenda réunissent ceux de tous les espaces, et il ne se supprime pas. Un espace s'archive (fin d'année, fin de stage) et se restaure. Avant de supprimer un autre espace, l'application montre ce qu'il contient et propose de tout transférer ailleurs. |
| Dashboard | Une grille bento propre à chaque espace : calendrier (aujourd'hui, trois jours ou la semaine selon la largeur), prochains cours, tâches, notes, fichiers, réglages rapides. On ajoute, déplace et agrandit chaque widget (petit, moyen, grand, ou jusqu'à 4 × 4 cases) à la souris, au clavier ou au doigt ; deux colonnes sur téléphone. Chaque widget se charge seul : un widget lent ne retarde pas les autres. |
| Notes | Une seule vue, comme une page Notion : le texte se met en forme pendant qu'on écrit, `/` ouvre le menu des blocs (titres, listes, cases à cocher, tableau, citation, code, lien, image, fichier). Le Markdown reste le format enregistré. Sauvegarde automatique, tags, épinglage, recherche plein texte qui ignore les accents. Une note se range dans un dossier depuis l'éditeur. |
| Agenda | Flux iCal de Pronote importé par son lien, resynchronisé toutes les heures, en vue jour ou semaine. |
| Fichiers (v2) | Des dossiers dans chaque espace (trois niveaux au plus : Espace / Dossier / Sous-dossier), pour les fichiers et les notes ; on range par une fenêtre qui montre tout, ou en glissant à la souris. Tags (les mêmes que les notes) et recherche sur le nom. Partage d'un fichier par un lien valable 1 heure, 1 jour ou 7 jours, révocable : la page du lien ne montre que le nom et la taille, et télécharge. Envoi par glisser-déposer, jusqu'à 2 Go par fichier. Vignettes et aperçu des photos (HEIC comprises), vidéos, PDF et textes. Compression et conversion (photo en JPEG, PNG ou WebP, vidéo en MP4, son en MP3, pages d'un PDF, Markdown et texte en PDF, HTML ou DOCX). L'onglet Outils garde 24 heures un fichier pas encore rangé, le temps de le convertir. |
| Profil et affichage | Prénom, nom et photo de profil (sans photo, les initiales sur une couleur tirée au hasard). Thème clair ou sombre, taille de l'interface, barre latérale ancrée ou cachée à gauche, qui apparaît au survol du bord. |
| Connexion | Mot de passe ou Google, sur le même compte. Pas d'inscription publique : seuls les emails autorisés entrent. |
| API | Une API JSON, désactivée par défaut et protégée par une clé, pour qu'un assistant gère les tâches et les notes et lise l'agenda. |

`/` ouvre une barre de commandes : ouvrir une note ou un fichier, ajouter une tâche (`/add task Rendre le DM +demain`), changer d'espace, tout au clavier. Sur iPhone, l'application s'installe sur l'écran d'accueil, comme une app.

## Sous le capot

Rust (axum), SQLite, htmx et askama. Les pages sont rendues par le serveur et htmx s'occupe de l'interactivité. Pas de framework JavaScript : htmx et l'éditeur de notes (ProseMirror), rien d'autre.

Mesures de la v1 sur le serveur réel, via Cloudflare en 4G simulée : Lighthouse mobile de 98 à 100, 7,6 Mo de mémoire, 0,5 ms de réponse serveur, image Docker de 7 Mo.

**Sécurité.** Mots de passe en Argon2id, connexion Google par OpenID Connect avec PKCE, protection CSRF, limite de tentatives de connexion, Content-Security-Policy stricte (aucun script en ligne). Chaque requête SQL filtre sur l'utilisateur connecté, et des tests vérifient qu'un second compte ne voit rien du premier. Un fichier envoyé n'est jamais exécuté dans la page. Les conversions tournent dans un service à part, sans réseau, qui ne voit ni la base ni les secrets.

**Hébergement.** Un conteneur LXC Debian sur un serveur Proxmox, le binaire sous systemd. On y accède par un tunnel Cloudflare et par Tailscale, sans ouvrir aucun port sur la box. La base est sauvegardée et vérifiée chaque nuit sur un second disque, les fichiers sont copiés chaque nuit sur le premier, et la restauration a été testée sur le serveur.

## Installer

Le code de l'application est sur la branche **`v1`** (la branche `main` ne contient que cette page et le site). Il faut [Rust](https://rustup.rs) 1.85 ou plus récent et un compilateur C.

```bash
git clone -b v1 https://github.com/moreauremi/dashu.git
```

```bash
cd dashu
```

```bash
cp .env.example .env
```

Renseigner `SESSION_SECRET`, `ALLOWED_EMAILS` et `BASE_URL` dans `.env`, créer le compte, puis lancer le serveur, qui répond sur <http://localhost:3000> :

```bash
cargo run -- set-password toi@example.com
```

```bash
cargo run
```

Le [README de la branche v1](https://github.com/moreauremi/dashu/blob/v1/README.md) détaille tout le reste : les variables d'environnement, le déploiement sur un serveur (LXC, systemd, Tailscale, tunnel Cloudflare), la variante Docker, les sauvegardes et la restauration, la connexion Google et l'API.

## Feuille de route

Une version à la fois, utilisée au quotidien avant de passer à la suivante.

| Version | Contenu | État |
| --- | --- | --- |
| v1 | Espaces, dashboard, notes Markdown, agenda Pronote, API pour un assistant ; puis espaces à la carte, dashboard modulaire, profil et affichage, barre de commandes, éditeur façon Notion | En service |
| v2 | Fichiers par espace et par dossier, partage par lien temporaire, onglet Outils (conversions) | En cours, détail ci-dessous |
| v3 | OCR : texte des images et des PDF scannés | Prévue |
| v4 | Médias : retouche et détourage de photos dans le navigateur | Prévue |
| v5 | Audio : lecture des notes à voix haute, transcription des vocaux et des cours | Prévue |
| v6 | IA : un assistant qui cherche dans les notes et les fichiers | Prévue |

Avancement de la v2 (une case cochée : en service sur le serveur) :

- [x] Fichiers par espace, envoi par glisser-déposer, gros fichiers par morceaux de 50 Mo
- [x] Dossiers et sous-dossiers, pour les fichiers et les notes
- [x] Tags et recherche dans les fichiers
- [x] Vignettes et aperçu des photos (HEIC comprises), vidéos et PDF
- [x] Partage par lien temporaire et révocable
- [x] Sauvegarde nocturne des fichiers, sur un autre disque que la base
- [x] Compression et conversion des photos, vidéos, sons et PDF, dans un service isolé
- [x] Conversion des documents : Markdown et texte vers PDF, HTML ou DOCX
- [x] Onglet Outils : déposer un fichier, le convertir, puis le garder ou le télécharger
- [x] Images collées et liens vers les fichiers dans les notes
- [x] Widget Fichiers sur le dashboard
- [ ] Renommer un fichier
- [ ] Les fichiers dans l'API de l'assistant

## Branches du dépôt

| Branche | Contenu |
| --- | --- |
| [`v1`](https://github.com/moreauremi/dashu/tree/v1) | L'application, ses tests et sa documentation. Elle reçoit toutes les évolutions, v2 comprise. |
| `main` | Cette page et `website/`, la page d'accueil de [dashu.dev](https://dashu.dev), publiée par GitHub Pages à chaque push. |

Pour voir la page d'accueil en local :

```bash
python3 -m http.server -d website 8000
```

Puis ouvrir <http://localhost:8000>.

## Licence

À choisir avant la première version publique (MIT ou AGPL).
