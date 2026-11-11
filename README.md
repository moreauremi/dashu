# dashu

Espace de travail personnel tout-en-un, auto-hébergé : un dashboard, des notes Markdown et un emploi du temps, répartis sur trois espaces (Perso, École, Taff) derrière une seule connexion.

dashu tient dans **un seul binaire Rust**. Les pages sont rendues par le serveur et htmx s'occupe de l'interactivité. Le cahier des charges complet est dans [SPEC.md](SPEC.md), l'état d'avancement dans [STATUS.md](STATUS.md).

## Sommaire

- [Ce que fait la v1](#ce-que-fait-la-v1)
- [Installation locale](#installation-locale)
- [Variables d'environnement](#variables-denvironnement)
- [Comptes et connexion](#comptes-et-connexion)
- [Créer les identifiants Google](#créer-les-identifiants-google)
- [API pour un assistant](#api-pour-un-assistant)
- [Déploiement : homelab et tunnel Cloudflare](#déploiement--homelab-et-tunnel-cloudflare)
- [Sauvegardes et restauration](#sauvegardes-et-restauration)
- [Sécurité](#sécurité)
- [Développement](#développement)
- [Choix techniques à connaître](#choix-techniques-à-connaître)
- [Limites connues de la v1](#limites-connues-de-la-v1)

## Ce que fait la v1

| Module | Contenu |
| --- | --- |
| Authentification | Email + mot de passe (Argon2id) ou Google, sur le même compte. Inscription fermée : seuls les emails de `ALLOWED_EMAILS` entrent. |
| Espaces | Perso, École, Taff. Chaque contenu appartient à un espace ; on en change en un clic, sans rechargement. |
| Dashboard | Prochains cours, tâches du jour, notes récentes. Chaque widget se charge indépendamment. |
| Notes | Éditeur Markdown avec aperçu en direct, sauvegarde automatique, tags, recherche plein texte, épinglage, déplacement entre espaces. |
| Agenda | Import d'un flux iCal (Pronote) par URL, synchro toutes les heures, vues jour et semaine. |

Raccourcis clavier : `Ctrl+K` (ou `Cmd+K`) ouvre la palette de commandes, `N` crée une note, `G` puis `D`, `N` ou `C` mène au Dashboard, aux Notes ou à l'Agenda. Dans l'éditeur, `Ctrl+S` enregistre tout de suite.

## Installation locale

Il faut [Rust](https://rustup.rs) (stable, 1.85 ou plus récent) et un compilateur C (gcc, clang ou MSVC : SQLite est compilé avec le projet). Node n'est pas nécessaire.

```bash
git clone https://github.com/moreauremi/dashu.git
```

```bash
cd dashu
```

```bash
cp .env.example .env
```

Dans `.env`, renseigner au minimum :

```ini
SESSION_SECRET=<64 caractères, par exemple : openssl rand -hex 32>
ALLOWED_EMAILS=toi@example.com
BASE_URL=http://localhost:3000
DATABASE_URL=sqlite://dashu.db
```

Créer le compte (le mot de passe est demandé au clavier, 12 caractères minimum), puis lancer le serveur :

```bash
cargo run -- set-password toi@example.com
```

```bash
cargo run
```

dashu répond sur <http://localhost:3000>. La base `dashu.db` est créée et migrée au premier démarrage.

Lancer les tests :

```bash
cargo test
```

## Variables d'environnement

En local elles sont lues dans le fichier `.env` ; en production c'est Docker Compose qui les transmet. Le fichier `.env` n'est jamais versionné, seul `.env.example` l'est.

| Variable | Obligatoire | Rôle |
| --- | --- | --- |
| `SESSION_SECRET` | oui | Clé de signature du cookie de session, 64 caractères minimum. La changer déconnecte tout le monde. |
| `ALLOWED_EMAILS` | oui | Emails autorisés, séparés par des virgules. Retirer un email coupe l'accès de ce compte immédiatement. |
| `BASE_URL` | oui | URL publique, sans barre finale (`https://dashu.dev`). En `https`, le cookie devient `Secure` et l'en-tête HSTS est envoyé. |
| `DATABASE_URL` | non | Emplacement de la base. Par défaut `sqlite://dashu.db` ; dans Docker `sqlite:///data/dashu.db`. |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | non | Identifiants OAuth de Google. Sans eux, le bouton « Continuer avec Google » n'est pas affiché. |
| `API_TOKEN` | non | Active l'API JSON (`/api/v1`) : jeton attendu dans `Authorization: Bearer …`, 32 caractères minimum. Sans lui, l'API répond 404. |
| `API_USER_EMAIL` | avec `API_TOKEN` | Compte au nom duquel agit l'API. Doit figurer dans `ALLOWED_EMAILS`. |
| `TUNNEL_TOKEN` | en production | Jeton du tunnel Cloudflare, lu par le conteneur `cloudflared`. |
| `BACKUP_DIR` | en production | Dossier de l'hôte où vont les sauvegardes. |
| `BIND_ADDR` | non | Adresse d'écoute, `0.0.0.0:3000` par défaut. |
| `LOG_FORMAT` | non | `json` pour des logs en JSON, une ligne par événement. |
| `RUST_LOG` | non | Niveau de log, `info` par défaut. |

## Comptes et connexion

Il n'y a volontairement **pas de page d'inscription**. Un compte se crée de deux façons :

1. **Par Google** : la première connexion d'un email présent dans `ALLOWED_EMAILS` crée le compte. Un mot de passe peut ensuite être défini dans Réglages.
2. **En ligne de commande**, sur le serveur : `dashu set-password <email>` crée le compte ou remplace son mot de passe (c'est aussi la procédure en cas d'oubli).

Pourquoi : un formulaire d'inscription sans vérification d'email laisserait quiconque connaît une adresse autorisée créer le compte avant son propriétaire. Google prouve que l'adresse appartient au visiteur ; la ligne de commande prouve qu'on a la main sur le serveur.

Les deux méthodes mènent au même compte : Google est rattaché par l'email vérifié qu'il renvoie.

## Créer les identifiants Google

Gratuit, à faire une fois dans la [Google Cloud Console](https://console.cloud.google.com/).

1. Créer un projet (par exemple « dashu »).
2. Menu **API et services > Écran de consentement OAuth** : type d'utilisateur **Externe**, un nom d'application et ton adresse de contact. Tant que l'application reste en mode « Test », ajouter ton adresse dans **Utilisateurs tests**.
3. Menu **API et services > Identifiants > Créer des identifiants > ID client OAuth**, type **Application Web**.
4. Dans **URI de redirection autorisés**, ajouter exactement :
   - `https://dashu.dev/auth/google/callback`
   - `http://localhost:3000/auth/google/callback` (pour tester en local)
5. Copier l'ID client et le code secret dans `.env` (`GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`), puis redémarrer dashu.

dashu ne demande à Google que l'identité et l'adresse email (`openid email`).

## API pour un assistant

Une API JSON permet à un programme (par exemple un assistant personnel) de gérer les tâches et les notes et de lire l'agenda. Elle est **désactivée par défaut** : il faut renseigner `API_TOKEN` (par exemple `openssl rand -hex 32`) et `API_USER_EMAIL` dans `.env`, puis redémarrer.

Chaque requête porte l'en-tête `Authorization: Bearer <API_TOKEN>`. Il n'y a ni cookie ni session : la protection CSRF ne s'applique pas, et à l'inverse un cookie de session n'ouvre pas l'API. Les réponses et les erreurs sont en JSON (`{"error": "…"}`).

| Méthode et route | Rôle |
| --- | --- |
| `GET /api/v1/spaces` | Les espaces : `perso`, `ecole`, `taff`. |
| `GET /api/v1/tasks?space=ecole&status=open` | Tâches (`status` : `open` par défaut, `done` ou `all`). |
| `POST /api/v1/tasks` | Crée une tâche : `{"title", "space", "due_date"?}` (date `AAAA-MM-JJ`, aujourd'hui par défaut). |
| `GET`, `PATCH`, `DELETE /api/v1/tasks/{id}` | Lit, modifie (`title`, `done`, `due_date`) ou supprime une tâche. |
| `GET /api/v1/notes?q=vlan&space=ecole&tag=bts&limit=20` | Notes récentes, ou recherche plein texte avec `q`. |
| `POST /api/v1/notes` | Crée une note : `{"title", "body_md"?, "space", "tags"?}`. |
| `GET /api/v1/notes/{id}` | Note complète, avec son Markdown. |
| `PATCH /api/v1/notes/{id}` | `title`, `body_md` (remplace) ou `append` (ajoute à la fin), `pinned`, `space`, `add_tags`. |
| `GET /api/v1/calendar?from=2026-11-16&to=2026-11-20&space=ecole` | Événements de l'agenda (7 jours à partir d'aujourd'hui par défaut, 62 au plus). |
| `POST /api/v1/calendar/refresh` | Resynchronise tout de suite les flux iCal. |

Exemple :

```bash
curl -s -H "Authorization: Bearer $API_TOKEN" -H "Content-Type: application/json" \
  -d '{"title": "Réviser le VLAN", "space": "ecole"}' https://dashu.dev/api/v1/tasks
```

Volontairement absent : la suppression de notes, les réglages, le mot de passe et les sources iCal. Les dates renvoyées sont en heure de Paris, au format ISO 8601.

## Déploiement : homelab et tunnel Cloudflare

Le serveur ouvre une connexion sortante vers Cloudflare, qui relaie `dashu.dev` vers le conteneur. Aucun port n'est à ouvrir sur la box.

```text
navigateur ──HTTPS──> Cloudflare ══tunnel══> cloudflared ──HTTP──> dashu:3000 ──> /data/dashu.db
                                             └────────── réseau interne Docker ──────────┘
```

> **Avant de basculer le domaine.** `dashu.dev` sert aujourd'hui la page d'accueil du dossier `website/`, publiée par GitHub Pages (`.github/workflows/pages.yml`). Faire pointer le domaine vers le tunnel la remplace par l'application. Si la page d'accueil doit rester en ligne, lui donner un autre nom (par exemple `www.dashu.dev`) avant l'étape 1.

### 1. Cloudflare

1. Le domaine `dashu.dev` doit utiliser les serveurs DNS de Cloudflare (offre gratuite). Le certificat HTTPS est fourni par Cloudflare.
2. Dans **Zero Trust > Networks > Tunnels**, créer un tunnel de type **Cloudflared** et copier son jeton : c'est `TUNNEL_TOKEN`.
3. Dans l'onglet **Public Hostname** du tunnel, ajouter `dashu.dev` avec le service `http://dashu:3000`.

### 2. La machine

Un conteneur LXC Debian sur Proxmox (non privilégié, avec les options `nesting=1` et `keyctl=1` nécessaires à Docker), puis [Docker Engine et le plugin Compose](https://docs.docker.com/engine/install/debian/).

### 3. dashu

```bash
git clone https://github.com/moreauremi/dashu.git /opt/dashu
```

```bash
cd /opt/dashu && cp .env.example .env
```

Remplir `.env` (voir le tableau plus haut), préparer le dossier des sauvegardes, puis démarrer :

```bash
mkdir -p /mnt/backup/dashu && chown 10001:10001 /mnt/backup/dashu
```

```bash
docker compose up -d --build
```

La première construction compile le binaire (plusieurs minutes) ; les suivantes réutilisent le cache. Créer ensuite le compte, soit en se connectant avec Google, soit avec :

```bash
docker compose exec dashu /dashu set-password toi@example.com
```

### 4. Supervision et logs

- `https://dashu.dev/health` répond `ok` sans authentification : à interroger avec Uptime Kuma ou un équivalent.
- Docker vérifie lui-même la santé du conteneur ; `cloudflared` ne démarre que lorsque dashu répond.
- Logs : `docker compose logs -f dashu`. Ajouter `LOG_FORMAT=json` dans `.env` pour un format exploitable par un collecteur.

### 5. Mise à jour

```bash
cd /opt/dashu && git pull && docker compose up -d --build
```

Les migrations de la base s'appliquent au démarrage.

## Sauvegardes et restauration

`scripts/backup.sh` fait une copie à chaud de la base (commande SQLite `VACUUM INTO`, lancée par `dashu backup` dans le conteneur), **la rouvre pour la vérifier**, puis supprime les copies de plus de 14 jours. Le serveur n'est pas arrêté.

À planifier une fois avec `crontab -e` sur la machine qui héberge Docker :

```cron
30 3 * * * /opt/dashu/scripts/backup.sh >> /var/log/dashu-backup.log 2>&1
```

Restaurer une sauvegarde (le nom du projet Compose est celui du dossier, ici `dashu`) :

```bash
cd /opt/dashu && docker compose stop dashu
```

```bash
docker run --rm -v dashu_data:/data -v /mnt/backup/dashu:/backups:ro alpine sh -c "rm -f /data/dashu.db-wal /data/dashu.db-shm && cp /backups/dashu-AAAA-MM-JJ_HHMMSS.db /data/dashu.db && chown 10001:10001 /data/dashu.db"
```

```bash
docker compose start dashu
```

Une sauvegarde n'est fiable que si on a déjà réussi à la restaurer : faire l'exercice une fois après la mise en service.

## Sécurité

- **Mots de passe** hachés en Argon2id, un sel par hachage, 12 caractères minimum. La vérification prend le même temps que le compte existe ou non, et le message d'erreur est le même.
- **Google** : OpenID Connect « authorization code » avec PKCE, vérification du `state`, du `nonce` et de la signature du jeton ; seul un email vérifié par Google est accepté.
- **Sessions** stockées côté serveur (SQLite). Cookie signé, `HttpOnly`, `Secure`, `SameSite=Lax`, 30 jours glissants ; l'identifiant de session change à la connexion.
- **CSRF** : jeton par session exigé sur toute requête qui modifie des données, dans l'en-tête `X-CSRF-Token` (htmx) ou le champ `_csrf` (formulaires classiques).
- **Anti-bruteforce** : 5 tentatives de connexion par minute et par adresse IP. Derrière le tunnel, l'adresse est lue dans `CF-Connecting-IP`, que Cloudflare écrit lui-même.
- **Content-Security-Policy stricte** : ni script ni style en ligne, pas d'`eval`. Même si une balise `<script>` survivait dans une note, le navigateur ne l'exécuterait pas.
- **Notes** : le HTML issu du Markdown passe toujours par ammonia (liste blanche de balises et d'attributs).
- **Isolation** : toutes les requêtes SQL filtrent sur l'utilisateur connecté. Des tests vérifient qu'un second compte ne peut ni lire ni modifier les données du premier.
- **Flux iCal** : l'URL Pronote contient un jeton d'accès. Elle n'est jamais réaffichée en entier ni écrite dans les logs. Seules les URL `https` publiques sont acceptées.
- **API** : désactivée tant que `API_TOKEN` est vide. Le jeton (32 caractères minimum) est comparé en temps constant et n'apparaît jamais dans les logs. L'API n'agit que sur le compte `API_USER_EMAIL` et ne peut ni supprimer une note, ni toucher aux réglages, au mot de passe ou aux sources iCal.
- **Image Docker** : image `scratch` (le binaire seul), utilisateur sans privilège, système de fichiers en lecture seule, aucune capacité Linux, aucun port publié.

## Développement

```text
dashu/
├── Cargo.toml
├── build.rs                empreintes des fichiers statiques, classes de la grille de l'agenda
├── migrations/             schéma SQL, appliqué au démarrage
├── .sqlx/                  métadonnées des requêtes (vérification à la compilation sans base)
├── src/
│   ├── main.rs             serveur, set-password, backup, healthcheck
│   ├── lib.rs              état partagé et routeur
│   ├── config.rs           variables d'environnement
│   ├── db.rs               pool SQLite, mode WAL, migrations
│   ├── assets.rs           fichiers statiques embarqués
│   ├── time.rs             Europe/Paris, dates en français
│   ├── auth/               identifiants, Google, sessions, limiteur de tentatives
│   ├── spaces/             espaces et espace actif
│   ├── dashboard/          page d'accueil et widgets
│   ├── notes/              liste, éditeur, tags, rendu Markdown
│   ├── calendar/           lecture iCal, synchro, grille jour/semaine
│   ├── search/             palette Ctrl+K
│   ├── settings/           thème, mot de passe, Google, sources iCal
│   └── web/                erreurs, CSRF, en-têtes de sécurité, /styleguide
├── templates/              gabarits askama : pages et fragments htmx (préfixe _)
├── static/                 CSS, JS, icônes, polices (embarqués dans le binaire)
├── frontend/               source de l'éditeur CodeMirror
├── tests/                  tests d'intégration + fixtures/pronote.ics
├── scripts/                backup.sh, build-assets.mjs, dev.cmd
├── Dockerfile, docker-compose.yml
└── website/                page d'accueil publique de dashu.dev (GitHub Pages)
```

**Modifier une requête SQL.** Les requêtes sont vérifiées à la compilation contre le schéma, à partir du dossier `.sqlx` (le fichier `.cargo/config.toml` active ce mode hors-ligne). Après avoir touché à une requête ou à une migration, régénérer ces métadonnées avec [sqlx-cli](https://crates.io/crates/sqlx-cli) :

```bash
cargo install sqlx-cli@0.8.6 --no-default-features --features sqlite
```

```bash
sqlx database create && sqlx migrate run && cargo sqlx prepare
```

**Retoucher le CSS sans recompiler.** Les fichiers statiques sont embarqués dans le binaire. En développement, la variable `DASHU_STATIC_DIR=static` les fait relire sur le disque à chaque requête (c'est ce que fait `scripts/dev.cmd` sous Windows).

**Mettre à jour htmx, CodeMirror, les icônes ou les polices.** Ces fichiers sont produits par un script et versionnés dans `static/` ; Node ne sert qu'à ça :

```bash
npm install && npm run build
```

**Guide de style.** La page `/styleguide` (connecté) montre toutes les briques de l'interface dans les deux thèmes. C'est la référence avant d'ajouter un écran.

## Choix techniques à connaître

Écarts ou précisions par rapport à la stack du cahier des charges :

| Sujet | Choix | Raison |
| --- | --- | --- |
| Dépendances front | htmx et CodeMirror, rien d'autre | Règle de la spec. `static/js/app.js` (palette, raccourcis, horloge) est écrit à la main. esbuild, lucide-static et fontsource ne servent qu'à fabriquer les fichiers de `static/` et ne sont pas servis. |
| sqlx 0.8 (et non 0.9) | Version imposée par `tower-sessions-sqlx-store` | Deux versions de sqlx ne peuvent pas lier SQLite dans le même binaire. |
| reqwest 0.12 (et non 0.13) | Version qu'utilise `openidconnect` 4 | Un seul client HTTP, en rustls + ring, sans dépendance système. |
| syntect | Coloration du code des notes, côté serveur | « Code avec coloration » sans ajouter de JavaScript. Les classes produites sont filtrées par ammonia. |
| Fichiers statiques | Embarqués dans le binaire, empreinte dans le nom | L'image Docker ne contient qu'un fichier ; cache navigateur d'un an sans risque de version périmée. |
| Styles en ligne | Aucun | La CSP les interdit. L'agenda se place avec des classes générées par `build.rs` ; CodeMirror vit dans un shadow root. |
| Thèmes | Fonction CSS `light-dark()` | Chaque couleur est écrite une fois. Demande un navigateur de 2024 ou plus récent (Chrome 123, Firefox 120, Safari 17.5). |
| Dates | Entiers Unix (UTC) en base | Affichage en Europe/Paris par `src/time.rs`, heure d'été comprise. |

## Limites connues de la v1

- Les récurrences iCal (`RRULE`) ne sont pas développées. Pronote n'en produit pas ; un autre calendrier pourrait en contenir.
- Changer de mot de passe ne ferme pas les sessions déjà ouvertes sur d'autres appareils.
- La validation d'une URL iCal porte sur le nom saisi : un nom public qui résoudrait vers une adresse privée n'est pas détecté.
- `dashu set-password` affiche le mot de passe pendant la saisie.
- La page d'onglet garde l'ancien titre d'une note tant qu'elle n'est pas rechargée.

La suite (fichiers, outils PDF, médias, audio, assistant) est décrite dans la roadmap de [SPEC.md](SPEC.md).

## Licence

À choisir avant la première version publique (MIT ou AGPL).
