# État de la v1

Mis à jour le 11 novembre 2026, branche `v1`.

**En bref.** La v1 est terminée : **tous les critères d'acceptation sont vérifiés**, la plupart sur le serveur réel. dashu tourne sur le homelab, dans un conteneur LXC Debian du serveur Proxmox, joignable sur `https://app.dashu.dev` (tunnel Cloudflare) et sur le réseau Tailscale. Il sert au quotidien, sur ordinateur et sur iPhone (en app sur l'écran d'accueil). La CI GitHub tourne à chaque PR.

Une case cochée veut dire « vérifié », et la ligne dit où : sur le serveur réel, par les tests, ou les deux.

## Critères d'acceptation

### Fonctionnel

- [x] **Connexion par identifiants et par Google sur le même compte**
  - Identifiants : en service sur le serveur, et tests `tests/auth.rs`.
  - Google : vérifié sur le serveur le 11 novembre 2026. « Continuer avec Google » avec l'adresse du compte ouvre le même compte (mêmes notes). Client OAuth « Application Web » d'un projet Google dédié, en mode Test.
- [x] **Un email absent de `ALLOWED_EMAILS` est refusé, avec les deux méthodes**
  - Identifiants : test `email_hors_liste_refuse_meme_avec_le_bon_mot_de_passe` ; `dashu set-password` refuse aussi un email hors liste.
  - Google : vérifié sur le serveur avec un second compte Google, hors liste : retour à la page de connexion, refusé. Le message dit maintenant « Ce compte Google n'a pas accès à dashu. » (test `compte_google_refuse_message_clair`).
- [x] **6 tentatives de login ratées en une minute déclenchent un blocage temporaire** : test `sixieme_tentative_en_une_minute_bloquee` (5 refus, puis 429, par adresse IP). Derrière le tunnel, l'adresse vient de `CF-Connecting-IP`.
- [x] **Les 3 espaces existent à la première connexion et le changement d'espace filtre dashboard, notes et agenda** : en usage sur le serveur, et tests `trois_espaces_des_la_premiere_connexion`, `le_changement_d_espace_filtre_le_contenu`, `vue_semaine`.
- [x] **Une note créée, modifiée, taguée, retrouvée par la recherche, déplacée, puis supprimée** : test `cycle_de_vie_d_une_note`, et en usage.
- [x] **Le flux iCal Pronote Campus s'affiche correctement en vue semaine, heure d'été comprise** : le vrai flux Pronote Campus est branché sur l'espace École et s'affiche ; l'agenda de Perso réunit ceux de tous les espaces. L'heure d'été est vérifiée par les tests (`heure_d_ete_et_heure_d_hiver`, `vue_semaine_en_heure_d_ete`, avec `tests/fixtures/pronote.ics`) ; le prochain passage réel est le 28 mars 2027.
- [x] **La palette `Ctrl+K` trouve une note et ouvre une page** : vu dans le navigateur.
- [x] **Tout fonctionne sur téléphone** : utilisé sur iPhone, dans Safari et en app sur l'écran d'accueil (sans zoom, icône, barre de raccourcis en bas, changement d'espace par balayage).

### Technique

- [x] **`cargo clippy` sans avertissement, `cargo test` vert en CI** : GitHub Actions (`.github/workflows/ci.yml`) à chaque PR : `cargo fmt --check`, `clippy -D warnings`, 96 tests, puis l'image Docker. Vert sur les PR 1 à 9.
- [x] **Tests d'intégration sur : login, isolation des données entre deux utilisateurs, CRUD des notes, parsing iCal avec un `.ics` d'exemple** : `tests/auth.rs` (18), `tests/isolation.rs` (4), `tests/notes.rs` (5), `tests/calendar.rs` (12), plus `tests/api.rs` (9) et `tests/tabbar.rs` (4).
- [x] **Cibles de performance atteintes** : mesurées sur le serveur réel, via Cloudflare, en 4G simulée. Lighthouse mobile : 99 (dashboard, médiane de 3), 100 (notes), 100 (agenda), 99 (éditeur). Mémoire 7,6 Mo, réponse du serveur 0,5 ms, image Docker 7 Mo. Détail dans le tableau plus bas.
- [x] **Aucune injection de script possible via le contenu d'une note** : tests du nettoyage (`src/notes/markdown.rs`), test `une_note_ne_peut_pas_injecter_de_script` sur toutes les pages où une note apparaît, et CSP sans `unsafe-inline` en seconde barrière.
- [x] **Sauvegarde nocturne vérifiée par une restauration réelle** : faite sur le serveur le 11 novembre 2026.
  - Sauvegarde lancée par le service de la sauvegarde nocturne (`systemctl start dashu-backup`), puis une note créée.
  - `dashu-restore` de cette sauvegarde : la note a disparu.
  - `dashu-restore` du fichier `avant-restauration-*.db` : la note est revenue.
  - `dashu-backup.timer` la lance chaque nuit à 3 h 30 (heure du conteneur, UTC). Copie vérifiée par `PRAGMA integrity_check`.
  - Cas d'échec testés hors du serveur avec le vrai binaire : fichier corrompu, base qui n'est pas dashu, base avec laquelle dashu ne démarre pas (retour automatique).
- [x] **README complet** : installation locale, variables d'environnement, déploiement tel qu'il tourne (LXC, systemd, Tailscale, tunnel Cloudflare), mise à jour, sauvegarde et restauration, identifiants Google. Les scripts du serveur sont versionnés dans `deploy/lxc/`.

## Ce qu'il reste

Rien pour la v1. À garder en tête :

- **Le 12 novembre au matin**, `dashu-restore` (sans argument) doit lister un fichier `dashu-2026-11-12_0330….db` : la preuve que la minuterie lance bien la sauvegarde seule.
- **Si `/usr/local/sbin` manque dans le `PATH`** du shell ouvert par `pct enter` : `echo 'export PATH="/usr/local/sbin:/usr/sbin:/sbin:$PATH"' >> ~/.bashrc`.

Fait le 11 novembre : script de mesure injecté par Cloudflare désactivé (Web Analytics), client OAuth Google créé, `BASE_URL` passé à `https://app.dashu.dev`. Le soir, les sauvegardes sont passées sur le second disque, comme le demandait la spec (voir « v2 : avancement »).

## v2 : avancement

Le cahier des charges et ses critères sont dans `SPEC.md`, partie v2. Faits : l'étape 1 (prérequis), l'étape 2 (fichiers), l'envoi par morceaux, la compression et l'aperçu (début de l'étape 3), avancés à la demande de Rémi. Une case reste vide tant que le critère n'est pas vérifié sur le serveur réel.

- [ ] **Le second disque est monté dans le conteneur et le reste après un redémarrage de l'hôte**
  - Fait le 11 novembre 2026 : l'ancien contenu du disque de 465 Go est effacé, et le disque est déclaré comme stockage LVM-thin `ssd2`. Deux volumes y sont pris : `mp0` de 45 Go sur `/srv/dashu`, `mp1` de 5 Go sur `/var/backups/dashu`.
  - Vérifié sur le serveur après un redémarrage du conteneur : les deux volumes sont montés, et dashu, cloudflared et Tailscale répondent.
  - Reste à vérifier au prochain redémarrage de l'hôte.
- [ ] **La base est sauvegardée sur le second disque, les fichiers sur le premier**
  - La base : vérifié sur le serveur. Les 12 sauvegardes existantes ont été recopiées sur le volume, avec les mêmes empreintes SHA-256. Une sauvegarde lancée par `dashu-backup` y a été écrite.
  - Les fichiers : à l'étape 6.
- [x] **Le conteneur a 4 Go de mémoire** : appliqué à chaud le 11 novembre 2026 (`pct set --memory 4096`).

**Fichiers, envoi par morceaux, compression** (déployés le 11 novembre 2026, version 43a7b21)

- Sur le serveur :
  - `FILES_DIR=/srv/dashu/fichiers` et `WORK_DIR=/srv/dashu/tmp` ;
  - `dashu-worker` actif, sous le compte `dashu-worker` (groupe `dashu-jobs`) ;
  - ffmpeg 7.1.5 (avec `zscale`), Ghostscript 10.05.1, libheif.
- Vérifié sur le serveur, en passant par le service de conversion réel :
  - une photo HEIC de 4032 × 3024 devient un JPEG de 2560 × 1920 (816 Ko → 391 Ko, 7 s) ;
  - un PDF avec une photo passe de 626 Ko à 57 Ko (7 s) ;
  - une vidéo 4K HDR (HLG) de 10 s devient un MP4 1080p en couleurs SDR (bt709), de 54,8 Mo à 3,7 Mo, en 45 s : une vidéo d'une minute prendra environ 4 à 5 minutes ;
  - mémoire du service au plus haut : 1086 Mo, pour une limite de 1536 Mo.
- Isolement vérifié pendant une conversion :
  - le service ne voit que l'interface réseau `lo` ;
  - la base, les fichiers rangés et `dashu.env` lui sont invisibles.
- Vérifié par les tests et dans un navigateur, sur un dashu local : envoi par morceaux d'un fichier de 120 Mo, identique à l'arrivée ; compression d'une vidéo ; vignettes ; aperçu, lecture et avance rapide d'une vidéo.
- Reste à faire par Rémi, sur `app.dashu.dev` : envoyer une vraie vidéo d'iPhone, la compresser et l'ouvrir dans l'aperçu.

## Mesures de performance

### Sur le serveur réel

Le 11 novembre 2026, via Cloudflare. Lighthouse en mode mobile (Moto G Power émulé, 4G lente simulée) : version 13.5 dans Chrome, fenêtre Invité, pour les pages connectées ; Lighthouse 12 en ligne de commande pour la page de connexion.

| Mesure | Cible | Mesuré |
| --- | --- | --- |
| Poids d'une page hors éditeur (HTML + CSS + JS, compressés) | < 60 Ko | 42 Ko sur `/login` (HTML 2 Ko, CSS 15 Ko, htmx 17 Ko, `app.js` 7 Ko) ; plus la police, 35 Ko |
| Poids de la page éditeur | < 250 Ko | 157 Ko de HTML + CSS + JS, 240 Ko avec les deux polices (mêmes fichiers mesurés en local) |
| Temps de réponse serveur d'une page | < 20 ms | 0,5 ms (`/login`, 20 requêtes depuis le conteneur, un pic à 3,4 ms) |
| Score Lighthouse Performance (mobile) | ≥ 95 | dashboard 98, 99, 99 ; notes 100 ; agenda 100 ; éditeur 99 ; connexion 97 à 99 |
| Mémoire du binaire au repos | < 50 Mo | 7,6 Mo (`systemctl show dashu -p MemoryCurrent`) |
| Taille de l'image Docker | < 30 Mo | 7 Mo (CI) |

Une première mesure du dashboard, dans le profil Chrome habituel avec ses extensions, donnait environ 94 : les extensions ajoutent du travail au chargement et faussent le temps de blocage.

### En local

Mesures du 10 novembre 2026, avant les ajouts qui ont fait passer `app.js` de 3 à 7 Ko. Binaire `release` (LTO) sous Windows 11, base de test, 200 requêtes par page. Ce ne sont pas les conditions de la spec (homelab, Cloudflare, 4G) : elles donnent un ordre de grandeur.

| Mesure | Cible | Mesuré en local |
| --- | --- | --- |
| Poids d'une page hors éditeur (HTML + CSS + JS, brotli) | < 60 Ko | 34 à 36 Ko, dont htmx 17 Ko, CSS 12 Ko, `app.js` 3 Ko |
| Poids de la page éditeur | < 250 Ko | 149 Ko, dont CodeMirror 114 Ko |
| Temps de réponse serveur d'une page | < 20 ms | 3 à 6 ms en moyenne selon la page |
| Score Lighthouse Performance (mobile) | ≥ 95 | 98 (dashboard), 100 (notes), 100 (agenda), 98 (éditeur) |
| Mémoire du binaire | < 50 Mo | 22 Mo au démarrage, 34 Mo après 2 000 requêtes |
| Taille de l'image Docker | < 30 Mo | non mesurée ; le binaire fait 15,6 Mo et l'image ne contient que lui |

Hors cible mais téléchargés : la police de l'interface (36 Ko) et le sprite d'icônes (2 Ko). La police de l'éditeur (40 Ko) n'est chargée que par les pages qui affichent du code.

Moyens imposés par la spec : compilation `release` avec LTO (fait), empreinte dans le nom des fichiers statiques et cache d'un an (fait, testé), compression brotli/gzip (fait), CodeMirror uniquement sur la page éditeur (fait), SQLite en mode WAL (fait).

## Écarts par rapport au cahier des charges

- **Déploiement sans Docker.** La spec prévoyait Docker Compose (dashu + `cloudflared`). dashu tourne en binaire natif sous systemd dans un LXC, avec `cloudflared` installé comme service à côté : une couche de moins sur un vieux serveur. Le `Dockerfile` et le `docker-compose.yml` restent maintenus (l'image est construite par la CI) et décrits dans le README comme variante.
- **L'application est sur `app.dashu.dev`**, pas sur `dashu.dev` : `dashu.dev` garde la page d'accueil publique du projet (GitHub Pages).
- **Accès supplémentaire par Tailscale** (`tailscale serve`), qui marche même si Cloudflare tombe.
- **Google en mode Test** : seuls les utilisateurs tests déclarés dans la Google Cloud Console passent l'écran de Google. Pour un outil perso, c'est suffisant, et ça ajoute une barrière devant `ALLOWED_EMAILS`.
- **Ajouts hors spec**, faits à la demande après la v1 de base : API JSON pour un assistant (Hermès) avec une administration SSH limitée à six actions, agenda de Perso qui réunit tous les espaces, événements de plus de 24 h dans le bandeau « journée », « île » de changement d'espace avec balayage, barre de raccourcis sur téléphone, app installable sur l'écran d'accueil.
- **Nom affiché « Hermes Dashboard »** depuis le 12 novembre 2026, avec le logo Hermes (barre latérale, connexion où il se déplie une fois, onglets, icônes, favicon). Seul ce qui est visible a changé : le code, le binaire et les commandes gardent le nom dashu.
- **Le dépôt n'était pas vide.** Il contenait la page d'accueil (`website/`) et son workflow GitHub Pages : ils sont laissés tels quels. Le README, qui décrivait un autre projet (équipes, domotique, connecteurs), a été remplacé.
- **Pas de page d'inscription.** Un compte naît à la première connexion Google ou avec `dashu set-password`. Raison dans le README : sans vérification d'email, un formulaire public permettrait de créer le compte d'une adresse autorisée à la place de son propriétaire.
- **Un commit pour toute l'application**, puis un par lot (tests, déploiement et documentation), au lieu d'un commit par étape : les sept étapes ont été écrites d'un seul tenant avant la première compilation. Les évolutions suivantes passent chacune par une PR.
- **Versions** : sqlx 0.8 et reqwest 0.12 plutôt que les dernières, pour rester compatibles avec `tower-sessions-sqlx-store` et `openidconnect`. Une crate hors liste, `syntect`, colore le code des notes côté serveur.
- **Colonnes ajoutées au modèle** pour des comportements demandés : `tasks.done_at` (une tâche cochée reste visible jusqu'à minuit), `calendar_sources.last_error` et `last_attempt_at` (afficher l'échec de synchro), `users.theme`, `users.mobile_tabs` et `calendar_events.all_day`.
- **Récurrences iCal (`RRULE`) non développées** : Pronote n'en produit pas.

Les autres limites connues sont dans le README.

## Pour la grille d'évaluation

- **Interventions** : une seule question posée (comment compiler, la machine n'ayant ni Rust, ni compilateur C, ni Docker). Rust et GCC ont été installés avec winget.
- **Bugs trouvés pendant le développement** par les tests et les essais, tous corrigés : un `<input type="password">` survivait au nettoyage d'une note ; une adresse comme `/notes/abc` répondait 400 au lieu d'une page 404 ; les couleurs des matières se répartissaient mal (quatre matières sur neuf de la même teinte) ; un retour arrière sur `/notes/new` aurait recréé une note.
- **Prix** : le nombre de jetons consommés n'est pas lisible depuis la session elle-même. Il se lit dans l'écran d'usage de l'application.
