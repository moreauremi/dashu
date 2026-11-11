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
- [Déploiement : homelab, Tailscale et tunnel Cloudflare](#déploiement--homelab-tailscale-et-tunnel-cloudflare)
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

En local elles sont lues dans le fichier `.env` ; en production dans `/etc/dashu/dashu.env`, que systemd transmet au service (ou `.env` avec Docker). Ces fichiers ne sont jamais versionnés, seul `.env.example` l'est.

| Variable | Obligatoire | Rôle |
| --- | --- | --- |
| `SESSION_SECRET` | oui | Clé de signature du cookie de session, 64 caractères minimum. La changer déconnecte tout le monde. |
| `ALLOWED_EMAILS` | oui | Emails autorisés, séparés par des virgules. Retirer un email coupe l'accès de ce compte immédiatement. |
| `BASE_URL` | oui | URL publique, sans barre finale (`https://app.dashu.dev`). Sert aussi d'adresse de retour à Google. En `https`, le cookie devient `Secure` et l'en-tête HSTS est envoyé. |
| `DATABASE_URL` | non | Emplacement de la base. Par défaut `sqlite://dashu.db` ; en production `sqlite:///var/lib/dashu/dashu.db` (Docker : `sqlite:///data/dashu.db`). |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | non | Identifiants OAuth de Google. Sans eux, le bouton « Continuer avec Google » n'est pas affiché. |
| `API_TOKEN` | non | Active l'API JSON (`/api/v1`) : jeton attendu dans `Authorization: Bearer …`, 32 caractères minimum. Sans lui, l'API répond 404. |
| `API_USER_EMAIL` | avec `API_TOKEN` | Compte au nom duquel agit l'API. Doit figurer dans `ALLOWED_EMAILS`. |
| `TUNNEL_TOKEN` | Docker seulement | Jeton du tunnel Cloudflare, lu par le conteneur `cloudflared`. |
| `BACKUP_DIR` | Docker seulement | Dossier de l'hôte où vont les sauvegardes. |
| `BIND_ADDR` | non | Adresse d'écoute, `0.0.0.0:3000` par défaut ; `127.0.0.1:3000` en production. |
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
2. Menu **Google Auth Platform** (ou **API et services > Écran de consentement OAuth**) : nom de l'application, adresse d'assistance, audience **Externe**. L'application peut rester en mode **Test** : dashu ne demande que l'identité, aucune vérification par Google n'est nécessaire. En mode Test, seules les adresses listées dans **Audience > Utilisateurs tests** peuvent passer l'écran de Google : y mettre ton adresse (et celle qui sert à tester le refus, voir STATUS.md).
3. **Clients > Créer un client**, type **Application Web**. Dans **URI de redirection autorisés**, ajouter exactement :
   - `https://app.dashu.dev/auth/google/callback`
   - `http://localhost:3000/auth/google/callback` (pour tester en local)
4. Copier l'ID client et le code secret dans la configuration (`GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` dans `/etc/dashu/dashu.env`, ou `.env` en local), vérifier que `BASE_URL` vaut `https://app.dashu.dev`, puis redémarrer dashu.

dashu ne demande à Google que l'identité et l'adresse email (`openid email`). L'adresse de retour est construite à partir de `BASE_URL` : la connexion Google se fait donc depuis `https://app.dashu.dev`, pas depuis l'adresse Tailscale (le mot de passe, lui, marche partout).

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
  -d '{"title": "Réviser le VLAN", "space": "ecole"}' https://app.dashu.dev/api/v1/tasks
```

Volontairement absent : la suppression de notes, les réglages, le mot de passe et les sources iCal. Les dates renvoyées sont en heure de Paris, au format ISO 8601.

## Déploiement : homelab, Tailscale et tunnel Cloudflare

dashu tourne **sans Docker**, dans un conteneur LXC Debian du serveur Proxmox de la maison : le binaire est compilé sur place et lancé par systemd. Il n'écoute que sur `127.0.0.1:3000`. Deux chemins y mènent, sans ouvrir un seul port sur la box :

```text
                  ┌───────────────────── conteneur LXC « dashu » ──────────────────────┐
partout ──HTTPS──> Cloudflare ══tunnel══> cloudflared ──┐
                  │                                     ├──HTTP──> dashu 127.0.0.1:3000 ──> /var/lib/dashu/dashu.db
tailnet ──HTTPS──> tailscale serve ─────────────────────┘
                  └─────────────────────────────────────────────────────────────────────┘
```

| Adresse | Sert à |
| --- | --- |
| `https://app.dashu.dev` | L'application, depuis n'importe où (tunnel Cloudflare). C'est le `BASE_URL`. |
| `https://dashu.<tailnet>.ts.net` | La même application, pour les appareils du réseau Tailscale. Reste joignable si Cloudflare tombe. |
| `https://dashu.dev` | La page d'accueil publique du projet (dossier `website/`, GitHub Pages). |

Une autre méthode, avec Docker, est décrite plus bas : [Variante : Docker](#variante--docker).

### 1. Le conteneur

Sur l'hôte Proxmox, un conteneur Debian 13 non privilégié : 2 cœurs, 2 Go de mémoire (la compilation en a besoin, dashu seul tient dans 50 Mo), 16 Go de disque. Trouver puis télécharger le modèle Debian 13 :

```bash
pveam update && pveam available --section system | grep debian-13
```

```bash
pveam download local debian-13-standard_<version>_amd64.tar.zst
```

Puis créer le conteneur (ici le numéro 120 ; `pvesh get /cluster/nextid` donne le prochain libre) :

```bash
pct create 120 local:vztmpl/debian-13-standard_<version>_amd64.tar.zst --hostname dashu --cores 2 --memory 2048 --swap 1024 --rootfs local-lvm:16 --net0 name=eth0,bridge=vmbr0,ip=dhcp --unprivileged 1 --features nesting=1 --onboot 1
```

Tailscale a besoin de `/dev/net/tun` : ajouter ces deux lignes à `/etc/pve/lxc/120.conf`, puis démarrer le conteneur (`pct start 120`) :

```ini
lxc.cgroup2.devices.allow: c 10:200 rwm
lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file
```

### 2. dashu

Dans le conteneur (`pct enter 120`), en root :

```bash
apt-get update && apt-get install -y curl
```

```bash
curl -fsSL https://raw.githubusercontent.com/moreauremi/dashu/v1/deploy/lxc/install.sh | bash -s -- toi@example.com https://app.dashu.dev
```

[`deploy/lxc/install.sh`](deploy/lxc/install.sh) installe Rust, clone la branche dans `/opt/dashu/src`, compile (10 à 20 minutes sur un vieux processeur), crée l'utilisateur système `dashu`, écrit la configuration dans `/etc/dashu/dashu.env` (un `SESSION_SECRET` est généré), installe le service, la sauvegarde nocturne et les scripts d'exploitation. Il se termine en affichant la commande qui crée le compte :

```bash
runuser -u dashu -- sh -c 'set -a; . /etc/dashu/dashu.env; exec dashu set-password "$0"' toi@example.com
```

Ce qui est installé :

| Élément | Rôle |
| --- | --- |
| `/usr/local/bin/dashu` | Le binaire. |
| `/etc/dashu/dashu.env` | Les variables d'environnement (lisible par root et le groupe `dashu` seulement). |
| `/var/lib/dashu/dashu.db` | La base SQLite. |
| `dashu.service` | Le service, sous l'utilisateur `dashu`, 256 Mo de mémoire au plus, durci (`deploy/lxc/hardening.conf` : système en lecture seule sauf `/var/lib/dashu`, aucune capacité). |
| `dashu-backup.timer` | La sauvegarde, chaque nuit à 3 h 30. |
| `dashu-update`, `dashu-backup`, `dashu-restore`, `dashu-admin` | Les scripts d'exploitation, dans `/usr/local/sbin`. |

Après une modification de `/etc/dashu/dashu.env` : `systemctl restart dashu`.

### 3. Tailscale

Dans le conteneur :

```bash
curl -fsSL https://tailscale.com/install.sh | sh
```

```bash
tailscale up --hostname dashu
```

```bash
tailscale serve --bg 3000
```

`tailscale serve` publie dashu en HTTPS sur le réseau Tailscale, avec un certificat valide. Il faut MagicDNS et HTTPS activés dans la console Tailscale.

### 4. Tunnel Cloudflare

1. Le domaine `dashu.dev` utilise les serveurs DNS de Cloudflare (offre gratuite).
2. Dans **Zero Trust > Réseaux > Tunnels**, créer un tunnel de type **Cloudflared**, choisir Debian et lancer dans le conteneur les commandes affichées (elles installent `cloudflared` comme service). Le jeton du tunnel est un secret : il ne quitte pas le conteneur.
3. Dans les **routes publiées** du tunnel, ajouter `app.dashu.dev` vers `http://localhost:3000`.
4. Dans **Analytics & Logs > Web Analytics**, désactiver l'injection automatique pour `dashu.dev` (**Manage Site**, réglage **Disable** ; il faut parfois d'abord ajouter le site à Web Analytics pour voir ce réglage). Sinon Cloudflare ajoute un script de mesure à chaque page, que la CSP de dashu bloque, avec une erreur dans la console. Vérification : `curl -s https://app.dashu.dev/login | grep -c beacon` doit répondre `0`.

### 5. Mise à jour

```bash
dashu-update
```

Le script récupère la branche (notée dans `/etc/dashu/branch`), recompile, sauvegarde la base (`avant-maj-*.db`), remplace le binaire et redémarre. Si la nouvelle version ne répond pas sur `/health` dans les 20 secondes, il remet l'ancien binaire et la base d'avant. Si la compilation échoue, rien ne change en service et le lancement suivant réessaie. La version en service est notée dans `/etc/dashu/deployed-commit`. Les scripts de `deploy/lxc/` sont remis à jour en même temps que le binaire. Les migrations de la base s'appliquent au démarrage.

### 6. Supervision et accès d'un assistant

- `https://app.dashu.dev/health` répond `ok` sans authentification : à interroger avec Uptime Kuma ou un équivalent.
- Logs : `journalctl -u dashu -f`. Ajouter `LOG_FORMAT=json` dans `/etc/dashu/dashu.env` pour un format exploitable par un collecteur.
- Un assistant (Hermès) peut administrer dashu en SSH sans avoir la main sur le conteneur : sa clé est limitée à [`dashu-admin`](deploy/lxc/dashu-admin) dans `/root/.ssh/authorized_keys` (`restrict,command="/usr/local/sbin/dashu-admin" ssh-ed25519 …`). Six actions existent : `status`, `logs [N]`, `restart`, `update`, `update-status`, `backup`. La restauration n'en fait pas partie.

### Variante : Docker

Le dépôt contient aussi un `Dockerfile` (image `scratch` de 7 Mo : le binaire seul) et un `docker-compose.yml` qui lance dashu et `cloudflared` côte à côte. La CI construit l'image à chaque push. Dans un conteneur LXC avec Docker (options `nesting=1` et `keyctl=1`) :

```bash
git clone https://github.com/moreauremi/dashu.git /opt/dashu && cd /opt/dashu && cp .env.example .env
```

Remplir `.env` (avec `DATABASE_URL=sqlite:///data/dashu.db`, `TUNNEL_TOKEN` et `BACKUP_DIR`), puis :

```bash
docker compose up -d --build
```

```bash
docker compose exec dashu /dashu set-password toi@example.com
```

Mise à jour : `git pull && docker compose up -d --build`. Sauvegarde : planifier [`scripts/backup.sh`](scripts/backup.sh) (`30 3 * * *` dans `crontab -e`) ; il explique aussi la restauration dans le volume.

## Sauvegardes et restauration

Chaque nuit à 3 h 30, `dashu-backup.timer` lance [`dashu-backup`](deploy/lxc/dashu-backup) : copie à chaud de la base (commande SQLite `VACUUM INTO`, par `dashu backup`), sans arrêter le service. La copie est **rouverte et vérifiée** (`PRAGMA integrity_check`) avant d'être gardée. Les copies vont dans `/var/backups/dashu` :

| Fichier | Fait par | Gardé |
| --- | --- | --- |
| `dashu-AAAA-MM-JJ_HHMMSS.db` | la sauvegarde de la nuit, ou `dashu-admin backup` | 14 jours |
| `avant-maj-*.db` | `dashu-update`, juste avant de remplacer le binaire | 30 jours |
| `avant-restauration-*.db` | `dashu-restore`, juste avant de remplacer la base | 30 jours |

Sauvegarder tout de suite : `systemctl start dashu-backup`.

**Restaurer.** [`dashu-restore`](deploy/lxc/dashu-restore) sans argument liste les sauvegardes, la plus récente en premier :

```bash
dashu-restore
```

```bash
dashu-restore dashu-2026-11-12_033000.db
```

Le script vérifie que le fichier est une base dashu saine (sinon il ne touche à rien), met la base actuelle de côté dans `avant-restauration-*.db`, arrête dashu, remplace la base, redémarre et vérifie que `/health` répond. Si dashu ne démarre pas avec la sauvegarde, la base d'avant est remise en place automatiquement. Une restauration se défait donc en restaurant le fichier `avant-restauration-*.db` qu'elle indique. Les sessions étant dans la base, revenir à la veille peut déconnecter les appareils connectés depuis.

Une sauvegarde n'est fiable que si on a déjà réussi à la restaurer. L'exercice est décrit dans [STATUS.md](STATUS.md).

> Les sauvegardes sont sur le disque du conteneur, donc sur le même disque physique que la base. Elles protègent d'une erreur ou d'une mise à jour ratée, pas d'une panne du disque. Pour ça, monter un dossier d'un second disque sur `/var/backups/dashu` (option `mp0` du conteneur) ou sauvegarder le conteneur entier avec Proxmox (`vzdump`) vers un autre support.

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
- **Service** : utilisateur système sans shell, écoute sur `127.0.0.1` seulement, système en lecture seule sauf `/var/lib/dashu`, aucune capacité Linux (`deploy/lxc/hardening.conf`). Configuration lisible par root et le groupe `dashu` seulement.
- **Image Docker** (variante) : image `scratch` (le binaire seul), utilisateur sans privilège, système de fichiers en lecture seule, aucune capacité Linux, aucun port publié.

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
├── deploy/lxc/             installation sans Docker : install.sh, services systemd, dashu-update, -backup, -restore, -admin
├── scripts/                backup.sh (variante Docker), build-assets.mjs, dev.cmd
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
| Fichiers statiques | Embarqués dans le binaire, empreinte dans le nom | Un seul fichier à déployer ; cache navigateur d'un an sans risque de version périmée. |
| Styles en ligne | Aucun | La CSP les interdit. L'agenda se place avec des classes générées par `build.rs` ; CodeMirror vit dans un shadow root. |
| Thèmes | Fonction CSS `light-dark()` | Chaque couleur est écrite une fois. Demande un navigateur de 2024 ou plus récent (Chrome 123, Firefox 120, Safari 17.5). |
| Dates | Entiers Unix (UTC) en base | Affichage en Europe/Paris par `src/time.rs`, heure d'été comprise. |
| Déploiement | Binaire natif + systemd dans un LXC, pas Docker | La spec prévoyait Docker Compose. Sur un vieux serveur, un binaire de 15 Mo sous systemd évite une couche de plus ; l'image Docker reste construite par la CI. |

## Limites connues de la v1

- Les récurrences iCal (`RRULE`) ne sont pas développées. Pronote n'en produit pas ; un autre calendrier pourrait en contenir.
- Changer de mot de passe ne ferme pas les sessions déjà ouvertes sur d'autres appareils.
- La validation d'une URL iCal porte sur le nom saisi : un nom public qui résoudrait vers une adresse privée n'est pas détecté.
- `dashu set-password` affiche le mot de passe pendant la saisie.
- La page d'onglet garde l'ancien titre d'une note tant qu'elle n'est pas rechargée.

La suite (fichiers, outils PDF, médias, audio, assistant) est décrite dans la roadmap de [SPEC.md](SPEC.md).

## Licence

À choisir avant la première version publique (MIT ou AGPL).
