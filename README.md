# Hermes Dashboard (dashu)

Depuis le 12 novembre 2026, l'application s'affiche sous le nom **Hermes Dashboard**, avec le logo Hermes commun aux projets de Rémi. Le nom de code reste dashu : dépôt, binaire, commandes (`dashu set-password`, `dashu-update`…), services et variables d'environnement ne changent pas.

Espace de travail personnel tout-en-un, auto-hébergé : un dashboard, des notes Markdown et un emploi du temps, répartis en espaces (Perso, École et Taff au départ, puis ceux qu'on crée) derrière une seule connexion.

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
| Espaces | Perso, École et Taff au départ. Dans Réglages, on en crée (8 au plus), on change leur nom, leur couleur et leur icône, on les archive (cachés du sélecteur, contenu toujours cherchable, restaurables) et on les supprime : le contenu passe alors dans un autre espace, ou disparaît avec lui après une alerte. Chaque contenu appartient à un espace ; on le choisit à la création (tâche, envoi de fichier) et on le corrige après (« Déplacer vers… » pour les notes, tâches et fichiers). Perso est la vue d'ensemble : son dashboard, ses notes, ses fichiers et son agenda réunissent tous les espaces (filtre « Seulement Perso »). Les pages d'un espace ont une adresse qui le nomme (`/e/ecole/notes`), et chaque onglet garde le sien. L'espace se voit dans le titre de l'onglet, au-dessus du titre des pages et dans un liseré de sa couleur. Réglage optionnel : ouvrir l'app sur l'espace du cours en cours. |
| Dashboard | Grille bento modulaire, propre à chaque espace : widgets Agenda (calendrier en grille comme la page Agenda : aujourd'hui, trois jours ou la semaine selon la largeur ; prochains cours), Tâches du jour, Notes, Fichiers récents et Réglages rapides. Bouton « Modifier » : on déplace un widget en le glissant (ou aux flèches du clavier), on change sa taille (S, M, L d'un clic, ou n'importe quelle taille par le coin, jusqu'à 4 × 4 cases), on en ajoute, on en retire. Chaque widget se charge indépendamment et ne défile jamais : ce qui ne tient pas à sa taille est retiré, avec un « + N de plus ». Sur téléphone, la grille passe à 2 colonnes. |
| Profil et affichage | Prénom, nom et photo de profil (recadrée et réduite dans le navigateur, réenregistrée par le serveur en JPEG de 256 px) ; sans photo, les initiales sur un fond tiré au hasard. Thème dans Réglages ; taille de l'interface (petite, normale, grande) en bas de la barre latérale. La barre latérale reste ancrée ou se détache : cachée à gauche, elle glisse par-dessus la page au survol du bord de l'écran. |
| Notes | Éditeur en une seule vue, comme une page Notion : le texte se met en forme pendant qu'on écrit. `/` ouvre le menu des blocs (texte, titres 1 à 3, listes à puces, numérotées ou à cocher, tableau, citation, séparateur, bloc de code, lien vers une adresse, une note ou un fichier, image, fichier, date du jour) ; les raccourcis Markdown marchent aussi pendant la frappe (`#`, `-`, `1.`, `[]`, `>`, ```` ``` ````, `---`, `**gras**`, `*italique*`, `~~barré~~`). Une sélection montre une barre de mise en forme (gras, italique, barré, code, lien). Le Markdown reste le format enregistré : recherche, API et extraits n'en voient pas d'autre. Sauvegarde automatique, tags, recherche plein texte, épinglage, déplacement entre espaces. |
| Agenda | Import d'un flux iCal (Pronote) par URL, synchro toutes les heures, vues jour et semaine. |
| Fichiers (v2, en cours) | Envoi par glisser-déposer (sur n'importe quelle page) ou par bouton, plusieurs fichiers à la fois, liste par espace, téléchargement, déplacement, suppression. Une image collée dans une note y est rangée et insérée. Menu « Convertir » par fichier : compression, photo en JPEG, PNG ou WebP, vidéo en MP4, son en MP3, pages d'un PDF ; puis « Remplacer l'original » si on veut. Règles par espace (vidéos en MP4, HEIC doublées d'un JPEG à l'arrivée). Onglet Outils : un fichier déposé y reste 24 heures hors des espaces, le temps de le convertir et de le garder ou le télécharger. Les conversions en cours se voient dans la barre latérale de toutes les pages. Voir [Fichiers](#7-fichiers-v2) et [Compression](#8-compression-v2). |

**Barre de commandes.** Hors d'un champ, `/` ouvre une barre où l'on tape des commandes, comme dans le chat de Minecraft ou un terminal : tout se fait au clavier. `Tab` complète (encore `Tab` : la suggestion suivante), `↑` `↓` choisissent, `Entrée` exécute, effacer le `/` referme la barre. Un début de mot suffit (`/g ag` vaut `/go agenda`), accents et majuscules ne comptent pas. Un espace se tape par son nom actuel : renommé « Général », Perso devient `/space general`, même si son adresse reste `/e/perso/`. Dans un champ (l'éditeur de notes compris, où `/` ouvre le menu des blocs), `Échap` en sort, puis `/`. Sur téléphone, le bouton « Commandes » ouvre la barre et on touche les suggestions. `/help` liste tout ; les principales :

| Commande | Effet |
| --- | --- |
| `/go dashboard`, `notes`, `fichiers`, `agenda`, `outils`, `reglages` | Ouvre la page ; `/go agenda demain`, `lundi`, `14/11` (vue jour), `/go agenda semaine 25/12` |
| `/open <titre ou mot du texte>` | Ouvre une note ou un fichier, suggestions pendant la frappe |
| `/search <texte>` | Notes de l'espace filtrées par ce texte |
| `/space <espace>` | Change d'espace |
| `/add task <titre> [@espace] [+quand]` | Crée une tâche, pour aujourd'hui ou un autre jour (`+demain`, `+lundi`, `+25/11` : elle apparaît ce jour-là) |
| `/add note [titre] [@espace]`, `/add file`, `/add space <nom>`, `/add agenda <url> [@espace] [nom]` | Crée une note, envoie un fichier, crée un espace (on y entre), ajoute un flux iCal |
| `/done <tâche>`, `/undone <tâche>`, `/delete task <tâche>` | Coche, décoche, supprime une tâche du jour, choisie par son titre |
| `/move <espace>`, `/delete` | Sur une note ou un fichier ouvert : le déplace, le supprime |
| `/tag <tag>`, `/untag <tag>`, `/pin` | Sur une note ouverte : tags, épinglage |
| `/convert <format>`, `/compress`, `/download`, `/keep <espace>` | Sur un fichier ouvert : le convertit (ce que son menu « Convertir » propose), le télécharge, le garde (onglet Outils) |
| `/theme sombre\|clair\|systeme`, `/size petit\|normal\|grand`, `/sidebar` | Thème, taille de l'interface, barre latérale ancrée ou détachée |
| `/refresh`, `/logout` | Resynchronise l'agenda, se déconnecte |

Une suppression demande un second `Entrée`.

Dans un texte que l'on écrit (titre d'une tâche ou d'une note, tag, recherche, nom d'un espace ou d'un agenda), `#date` devient la date du jour (`13/11/2026`) et `#cours` la matière du cours qui a lieu maintenant : `/add note #cours du #date` crée « MATHS du 13/11/2026 ». Hors d'un cours, `#cours` est refusé. Tout autre `#mot` reste tel quel.

L'historique des commandes (`↑` sur un `/` seul) reste dans le navigateur. Les raccourcis `N` (nouvelle note) et `G` puis `D`, `N`, `F`, `C` ou `O` (Dashboard, Notes, Fichiers, Agenda, Outils) restent aussi. Dans l'éditeur, `Ctrl+S` enregistre tout de suite, `Ctrl+K` ajoute un lien (une adresse, ou le nom d'une note ou d'un fichier), `Ctrl+B`, `Ctrl+I`, `Ctrl+E` et `Ctrl+Maj+S` mettent en gras, en italique, en code ou barré, `Ctrl+Alt+1` à `3` font un titre, `Ctrl+Entrée` coche une case, et `Tab` passe à la cellule suivante d'un tableau (une ligne s'ajoute au bout). Sur Mac, `Cmd` à la place de `Ctrl`.

## Installation locale

Il faut [Rust](https://rustup.rs) (stable, 1.85 ou plus récent) et un compilateur C (gcc, clang ou MSVC : SQLite est compilé avec le projet). Node n'est pas nécessaire.

```bash
git clone -b v1 https://github.com/moreauremi/dashu.git
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
| `FILES_DIR` | non | Active les fichiers : dossier où dashu range leur contenu (`/srv/dashu/fichiers` en production). Sans lui, la page Fichiers l'indique et n'accepte aucun envoi. |
| `MAX_UPLOAD_MB` | non | Taille maximale d'un fichier, en Mo. 2048 par défaut. |
| `WORK_DIR` | non | Active la compression : dossier de travail partagé avec le service de conversion (`/srv/dashu/tmp` en production). Demande `FILES_DIR`. |
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
2. Menu **Google Auth Platform** (ou **API et services > Écran de consentement OAuth**) : nom de l'application, adresse d'assistance, audience **Externe**. L'application peut rester en mode **Test** : dashu ne demande que l'identité, aucune vérification par Google n'est nécessaire. En mode Test, seules les adresses listées dans **Audience > Utilisateurs tests** peuvent passer l'écran de Google : y mettre ton adresse, et une seconde adresse Google absente de `ALLOWED_EMAILS` pour vérifier que dashu la refuse.
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
| `GET /api/v1/spaces` | Les espaces du compte : slug, nom, couleur. Le slug (`perso`, `ecole`, `taff`, ou tiré du nom à la création) ne change pas quand on renomme un espace. |
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

Sur l'hôte Proxmox, un conteneur Debian 13 non privilégié : 2 cœurs, 4 Go de mémoire (la compilation en demande environ 1,2 Go, les conversions de la v2 davantage ; dashu seul tient dans 50 Mo), 16 Go de disque. Trouver puis télécharger le modèle Debian 13 :

```bash
pveam update && pveam available --section system | grep debian-13
```

```bash
pveam download local debian-13-standard_<version>_amd64.tar.zst
```

Puis créer le conteneur (ici le numéro 120 ; `pvesh get /cluster/nextid` donne le prochain libre) :

```bash
pct create 120 local:vztmpl/debian-13-standard_<version>_amd64.tar.zst --hostname dashu --cores 2 --memory 4096 --swap 1024 --rootfs local-lvm:16 --net0 name=eth0,bridge=vmbr0,ip=dhcp --unprivileged 1 --features nesting=1 --onboot 1
```

Tailscale a besoin de `/dev/net/tun` : ajouter ces deux lignes à `/etc/pve/lxc/120.conf`, puis démarrer le conteneur (`pct start 120`) :

```ini
lxc.cgroup2.devices.allow: c 10:200 rwm
lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file
```

**Second disque.** Les sauvegardes, et à partir de la v2 les fichiers, vont sur un autre disque physique que la base. Sur l'hôte, le disque est déclaré comme stockage LVM-thin (ici `ssd2` ; dans l'interface : **Disques > LVM-Thin > Créer**, ou la commande ci-dessous), puis deux volumes en sont montés dans le conteneur. Proxmox les crée, les formate et gère leurs droits dans un conteneur non privilégié ; ils se branchent même à chaud.

```bash
pvesh create /nodes/<nœud>/disks/lvmthin --device /dev/disk/by-id/<disque> --name ssd2 --add_storage 1
```

```bash
pct set 120 -mp0 ssd2:45,mp=/srv/dashu
```

```bash
pct set 120 -mp1 ssd2:5,mp=/var/backups/dashu
```

`/srv/dashu` (45 Go) accueillera les fichiers de la v2. `/var/backups/dashu` (5 Go) reçoit les sauvegardes de la base : les scripts écrivent déjà là, rien d'autre à changer. Sur un conteneur déjà installé, copier d'abord le contenu de `/var/backups/dashu` ailleurs, monter le volume, puis remettre les fichiers (propriétaire `dashu:dashu`, droits `750`). Un volume s'agrandit à chaud : `pct resize 120 mp0 +10G`.

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

Si le shell ouvert par `pct enter` ne trouve pas `dashu-update` ou `dashu-restore`, c'est que `/usr/local/sbin` manque dans son `PATH` : `echo 'export PATH="/usr/local/sbin:/usr/sbin:/sbin:$PATH"' >> ~/.bashrc`, puis rouvrir le shell.

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
- Côté Hermès, tout est versionné dans [`deploy/hermes/`](deploy/hermes) : la commande `dashu` (Python, sans dépendance) qui appelle l'API avec l'adresse et le jeton rangés dans `~/.config/dashu/`, et le skill qui dit à Hermès quand et comment s'en servir. [`install.sh`](deploy/hermes/install.sh) installe ou met à jour les deux, et met en place l'accès d'admin s'il manque. Sans danger à relancer, il se lance en root sur l'hôte Proxmox après un `dashu-update` (remplacer `<n>` par le numéro du conteneur dashu) :

```bash
bash <(pct exec <n> -- cat /opt/dashu/src/deploy/hermes/install.sh)
```

### 7. Fichiers (v2)

Le contenu des fichiers va dans `FILES_DIR`, sur le second disque (`/srv/dashu/fichiers`) ; la base n'en garde que le nom, la taille, le type et l'empreinte SHA-256. Sur le disque, un fichier porte une clé aléatoire, jamais le nom choisi par l'utilisateur : un nom piégé comme `../../etc/passwd` ne peut désigner aucun autre fichier.

- **Envoi, par morceaux** : l'offre gratuite de Cloudflare refuse toute requête de plus de 100 Mo. `static/js/files.js` découpe donc chaque fichier en morceaux de 50 Mo (deux fichiers à la fois, une barre de progression par fichier) :
  1. `POST /files/uploads?name=…&size=…` ouvre l'envoi ; une taille au-delà de `MAX_UPLOAD_MB` est refusée tout de suite ;
  2. chaque morceau part dans un `PUT /files/uploads/{id}?offset=…`, et dashu l'ajoute au fichier partiel `FILES_DIR/.envois/<id>.part`, au fil de l'eau, en comptant les octets ;
  3. au dernier morceau, le fichier rejoint la liste.

  La mémoire de dashu ne dépend donc pas de la taille du fichier. Un morceau raté est renvoyé (trois essais). Si dashu attend un autre morceau, il répond 409 avec l'en-tête `x-upload-received`, et le navigateur reprend de là. Un envoi en cours survit à un redémarrage de dashu ; abandonné, il est effacé au bout de 24 heures.
- **Téléchargement** : toujours en pièce jointe (`Content-Disposition: attachment`), avec `Content-Security-Policy: sandbox` et `nosniff` : un fichier HTML ou SVG piégé ne s'exécute jamais dans la page de dashu.
- **Vignettes** : à l'arrivée d'une photo (HEIC comprise), d'une vidéo ou d'un PDF, le service de conversion (voir [Compression](#8-compression-v2)) fabrique une vignette de 256 px, rangée à côté du fichier (`<clé>.thumb.jpg`), et un grand aperçu pour une photo HEIC, la première page d'un PDF ou l'image d'une vidéo avant lecture (`<clé>.preview.jpg`). Les fichiers arrivés avant, ou pendant que le service était arrêté, sont rattrapés au démarrage de dashu. Sans vignette, la liste montre l'icône du type.
- **Aperçu** (`/files/{id}`, en cliquant sur le nom) : la photo en grand, la vidéo ou le son avec un lecteur, la première page d'un PDF, le texte d'un fichier texte (le Markdown mis en forme et nettoyé comme une note). Seuls les types sans code (images bitmap, vidéos, sons) sont servis tels quels au navigateur (`/files/{id}/raw`), avec les demandes partielles (`Range`) qui permettent à Safari de lire une vidéo et d'avancer dedans. Un HTML ou un SVG s'affiche comme du texte, jamais comme une page.

`install.sh` crée le dossier et renseigne `FILES_DIR` quand `/srv/dashu` est monté. Sur un serveur déjà installé, en root dans le conteneur :

```bash
install -d -o dashu -g dashu -m 750 /srv/dashu/fichiers
```

```bash
echo 'FILES_DIR=/srv/dashu/fichiers' >> /etc/dashu/dashu.env
```

Le service n'écrit que dans les dossiers listés par `ReadWritePaths` ([`hardening.conf`](deploy/lxc/hardening.conf)). `dashu-update` ne réinstalle pas ce fichier : après la mise à jour, le recopier depuis le code récupéré, recharger systemd et redémarrer dashu :

```bash
install -m 644 /opt/dashu/src/deploy/lxc/hardening.conf /etc/systemd/system/dashu.service.d/ && systemctl daemon-reload && systemctl restart dashu
```

### 8. Compression et conversions (v2)

Le menu « Convertir » d'un fichier (sur sa ligne, sur son aperçu, ou `/convert` dans la barre de commandes) ne propose que ce que son type permet. Chaque sortie est une copie, dans le même espace ; l'original est gardé, sauf si on choisit ensuite « Remplacer l'original » (la copie prend alors son nom).

| Fichier | Conversions |
| --- | --- |
| Photo JPEG, PNG, WebP, GIF, HEIC | Compresser (ci-dessous), en JPEG, en PNG, en WebP (sans perte), dans sa définition d'origine |
| Vidéo | Compresser, en MP4 (H.264 et AAC, définition d'origine, HDR ramené en SDR), le son en MP3 |
| Son | En MP3 |
| PDF | Compresser, extraire des pages (« 2-4 ») |

Markdown et texte vers PDF, HTML ou DOCX viendront avec pandoc et Typst (étape 8 de la v2). Une règle par espace (Réglages > Espaces) peut convertir à l'arrivée : vidéos en MP4, photos HEIC doublées d'un JPEG.

L'onglet **Outils** reçoit un fichier pas encore rangé : il y reste 24 heures, hors des listes, le temps de le convertir, puis de le garder dans un espace ou de le télécharger. Passé ce délai, le nettoyage horaire l'efface.

La compression crée une copie plus légère, « nom (compressé) » :

| Fichier | Résultat | Outil |
| --- | --- | --- |
| Vidéo | MP4 H.264, 1080p au plus (verticale comprise), qualité CRF 23, son AAC 128 kb/s. Une vidéo HDR d'iPhone est ramenée en couleurs standard. | ffmpeg |
| Photo (JPEG, PNG, WebP) | JPEG, 2560 px au plus sur le grand côté, qualité 82, orientation de l'appareil appliquée | crate `image` (Rust) |
| Photo HEIC (iPhone) | Pareil, après décodage | libheif (`heif-dec`), puis `image` |
| PDF | Réglage « ebook » : images à 150 dpi | Ghostscript (`-dSAFER`) |

Si la copie n'est pas plus petite, elle n'est pas gardée et la page le dit.

**Pourquoi un service à part.** Lire un fichier envoyé par quelqu'un d'autre, c'est confier des octets inconnus à un programme complexe. dashu ne le fait donc jamais lui-même. Il dépose une tâche dans `WORK_DIR/jobs/<id>/` (une copie du fichier et `job.json`), et c'est `dashu worker`, lancé par [`dashu-worker.service`](deploy/lxc/dashu-worker.service), qui fait le travail :

- son propre compte (`dashu-worker`), aucun réseau, 1,5 Go de mémoire au plus, priorité basse ;
- il ne voit que `/srv/dashu/tmp` : ni la base, ni `dashu.env` et ses secrets, ni les fichiers rangés, ni les sauvegardes ;
- deux conversions à la fois au plus, chacune avec sa limite de temps (1 h pour une vidéo, 10 min pour un PDF, 2 min pour une photo HEIC) ;
- après un arrêt brutal, les conversions commencées reprennent de zéro.

dashu relève le travail toutes les 2 secondes : progression, résultat (rangé parmi les fichiers), ou message d'échec. La page suit l'avancement et recharge la liste à la fin. Le dossier de travail est partagé par le groupe `dashu-jobs`, dont dashu fait partie : `jobs/` est en `2770` (ce qui s'y crée prend ce groupe), et chaque tâche y a son dossier en `770`.

`install.sh` installe tout quand `/srv/dashu` est monté. Sur un serveur déjà installé, en root dans le conteneur, après le `dashu-update` qui apporte cette version :

```bash
apt-get install -y ffmpeg ghostscript libheif-examples
```

```bash
groupadd --system dashu-jobs && useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin --gid dashu-jobs dashu-worker && usermod -a -G dashu-jobs dashu
```

```bash
install -d -o dashu -g dashu-jobs -m 2770 /srv/dashu/tmp /srv/dashu/tmp/jobs && echo 'WORK_DIR=/srv/dashu/tmp' >> /etc/dashu/dashu.env
```

```bash
cd /opt/dashu/src/deploy/lxc && install -m 644 dashu-worker.service /etc/systemd/system/ && install -m 644 hardening.conf /etc/systemd/system/dashu.service.d/ && systemctl daemon-reload && systemctl enable --now dashu-worker && systemctl restart dashu
```

Vérifier : `systemctl status dashu-worker`, puis `journalctl -u dashu-worker -n 20`. La ligne « service de conversion démarré » dit aussi si le filtre `zscale` (couleurs des vidéos HDR) est présent. `dashu-update` redémarre le service de conversion avec dashu.

### Variante : Docker

Le dépôt contient aussi un `Dockerfile` (image `scratch` de 7 Mo : le binaire seul) et un `docker-compose.yml` qui lance dashu et `cloudflared` côte à côte. La CI construit l'image à chaque push. Dans un conteneur LXC avec Docker (options `nesting=1` et `keyctl=1`) :

```bash
git clone -b v1 https://github.com/moreauremi/dashu.git /opt/dashu && cd /opt/dashu && cp .env.example .env
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

Chaque nuit à 3 h 30 (heure du conteneur, UTC par défaut : 4 h 30 ou 5 h 30 à Paris), `dashu-backup.timer` lance [`dashu-backup`](deploy/lxc/dashu-backup) : copie à chaud de la base (commande SQLite `VACUUM INTO`, par `dashu backup`), sans arrêter le service. La copie est **rouverte et vérifiée** (`PRAGMA integrity_check`) avant d'être gardée. Les copies vont dans `/var/backups/dashu` :

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

Une sauvegarde n'est fiable que si on a déjà réussi à la restaurer. L'exercice, sans rien perdre :

1. `systemctl start dashu-backup` (le service que lance la minuterie chaque nuit), puis créer une note « test restauration » dans dashu.
2. `dashu-restore dashu-….db` avec la sauvegarde qui vient d'être faite : la note doit avoir disparu.
3. `dashu-restore avant-restauration-….db` avec le fichier indiqué à la fin de l'étape 2 : la note doit être revenue.

> `/var/backups/dashu` est un volume du second disque (`mp1`, voir « Le conteneur ») : une panne du disque de la base n'emporte pas ses sauvegardes. Sans second disque, les sauvegardes restent sur le disque du conteneur : elles protègent alors d'une erreur ou d'une mise à jour ratée, pas d'une panne du disque.

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
│   ├── files/              fichiers : envoi, liste, téléchargement (v2)
│   ├── tools/              compression : tâches (mod.rs) et service de conversion (worker.rs)
│   ├── notes/              liste, éditeur, tags, rendu Markdown
│   ├── calendar/           lecture iCal, synchro, grille jour/semaine
│   ├── commands/           barre de commandes « / » : liste, lecture d'une ligne, exécution
│   ├── settings/           thème, mot de passe, Google, sources iCal
│   └── web/                erreurs, CSRF, en-têtes de sécurité, /styleguide
├── templates/              gabarits askama : pages et fragments htmx (préfixe _)
├── static/                 CSS, JS, icônes, polices (embarqués dans le binaire)
├── frontend/               source de l'éditeur de notes (ProseMirror), bundle static/js/editor.js
├── tests/                  tests d'intégration + fixtures/pronote.ics
├── deploy/lxc/             installation sans Docker : install.sh, services systemd, dashu-update, -backup, -restore, -admin
├── deploy/hermes/          commande `dashu` et skill de l'assistant Hermès, et leur install.sh
├── scripts/                backup.sh (variante Docker), build-assets.mjs, dev.cmd
├── Dockerfile, docker-compose.yml
└── website/                page d'accueil publique de dashu.dev (GitHub Pages)
```

**Modifier une requête SQL.** Les requêtes sont vérifiées à la compilation contre le schéma, à partir du dossier `.sqlx` (le fichier `.cargo/config.toml` active ce mode hors-ligne). Après avoir touché à une requête ou à une migration, régénérer ces métadonnées avec [sqlx-cli](https://crates.io/crates/sqlx-cli) :

```bash
cargo install sqlx-cli@0.8.6 --no-default-features --features sqlite
```

Toujours sur une base jetable, jamais sur `dashu.db` : sqlx-cli active les clés étrangères, alors que certaines migrations (0007) exigent qu'elles soient coupées, comme le fait dashu au démarrage (`src/db.rs`). Sur une base qui a du contenu, la migration 0007 refuse de tourner dans ces conditions plutôt que de tout effacer.

```bash
export DATABASE_URL=sqlite://target/prepare.db
```

```bash
sqlx database create && sqlx migrate run && cargo sqlx prepare
```

**Retoucher le CSS sans recompiler.** Les fichiers statiques sont embarqués dans le binaire. En développement, la variable `DASHU_STATIC_DIR=static` les fait relire sur le disque à chaque requête (c'est ce que fait `scripts/dev.cmd` sous Windows).

**Mettre à jour htmx, l'éditeur (ProseMirror), les icônes ou les polices, ou modifier `frontend/`.** Ces fichiers sont produits par un script et versionnés dans `static/` ; Node ne sert qu'à ça :

```bash
npm install && npm run build
```

**Guide de style.** La page `/styleguide` (connecté) montre toutes les briques de l'interface dans les deux thèmes. C'est la référence avant d'ajouter un écran.

## Choix techniques à connaître

Écarts ou précisions par rapport à la stack du cahier des charges :

| Sujet | Choix | Raison |
| --- | --- | --- |
| Dépendances front | htmx, et pour l'éditeur ProseMirror et markdown-it ; rien d'autre | Règle de la spec. `static/js/app.js` (barre de commandes, raccourcis, horloge) est écrit à la main. esbuild, lucide-static et fontsource ne servent qu'à fabriquer les fichiers de `static/` et ne sont pas servis. |
| Éditeur de notes : ProseMirror (et non plus CodeMirror) | Une seule vue mise en forme, menu `/` (depuis le 13 novembre 2026) | Justification exigée par la spec. CodeMirror édite du texte brut : il fallait l'aperçu à côté. ProseMirror édite un document mis en forme (c'est la base de la plupart des éditeurs façon Notion) ; markdown-it lit le Markdown à l'ouverture, `prosemirror-markdown` le réécrit à chaque modification. Le serveur ne change pas : il reçoit toujours du Markdown. Le bundle pèse autant que l'ancien (environ 116 Ko en gzip) : les 2 000 entités HTML de markdown-it et la détection des adresses nues sont remplacées par deux petits fichiers (`frontend/stub-*.js`). Le code n'est pas coloré pendant l'édition (il l'est dans le rendu du serveur). |
| sqlx 0.8 (et non 0.9) | Version imposée par `tower-sessions-sqlx-store` | Deux versions de sqlx ne peuvent pas lier SQLite dans le même binaire. |
| reqwest 0.12 (et non 0.13) | Version qu'utilise `openidconnect` 4 | Un seul client HTTP, en rustls + ring, sans dépendance système. |
| syntect | Coloration du code des notes, côté serveur | « Code avec coloration » sans ajouter de JavaScript. Les classes produites sont filtrées par ammonia. |
| Fichiers statiques | Embarqués dans le binaire, empreinte dans le nom | Un seul fichier à déployer ; cache navigateur d'un an sans risque de version périmée. |
| Styles en ligne | Aucun | La CSP les interdit. L'agenda se place avec des classes générées par `build.rs` ; les menus de l'éditeur (`/`, mise en forme, liens, tableaux) avec une feuille construite en JavaScript (`adoptedStyleSheets`). |
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
