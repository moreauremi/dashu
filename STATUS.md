# État de la v1

Mis à jour le 13 novembre 2026, branche `v1`.

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
- [x] **La palette `Ctrl+K` trouve une note et ouvre une page** : vu dans le navigateur. Remplacée le 13 novembre 2026 par la barre de commandes `/`, qui fait de même avec `/open` et `/go` (voir plus bas).
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
- **Corrigé le 13 novembre 2026 : le bouton « Compresser » échouait sur le serveur** (« Une erreur est survenue de notre côté »). Deux causes, propres au bac à sable systemd de dashu (`deploy/lxc/hardening.conf`), absent en local comme dans les tests :
  - dashu ouvrait le dossier de la tâche en `chmod 2770`, et `RestrictSUIDSGID=yes` refuse tout `chmod` qui pose le bit setgid. Les vignettes échouaient pour la même raison (la liste montrait des icônes). Le dossier passe maintenant en `770`, après la création des fichiers, qui ont déjà hérité du groupe `dashu-jobs`.
  - Chaque `ReadWritePaths=` est un point de montage à part : ranger le résultat de `/srv/dashu/tmp` vers `/srv/dashu/fichiers` par `rename` aurait échoué (EXDEV). dashu copie puis efface quand `rename` échoue.
  - La CI rejoue ce bac à sable (`systemd-run` avec les mêmes options) pour le test `bac_a_sable_du_serveur` : il vérifie que le bit setgid est bien refusé et les deux dossiers bien séparés, puis qu'une tâche se prépare et que son résultat se range.
  - Après `dashu-update`, dashu demande au démarrage les vignettes manquantes (200 fichiers au plus à la fois).
- Reste à faire par Rémi, sur `app.dashu.dev`, après `dashu-update` : envoyer une vraie vidéo d'iPhone, la compresser et l'ouvrir dans l'aperçu.

## Espaces et outils : avancement

Cahier des charges dans `SPEC.md` (« Espaces et outils »), accepté en entier le 13 novembre 2026. Tout est écrit et vérifié par les tests d'intégration ; **rien n'est encore vérifié sur le serveur** (il faut `dashu-update`, puis essayer depuis le téléphone et l'ordinateur).

- [x] **B. Ranger après coup** : espace au choix pour une tâche et un envoi, « Déplacer vers… » pour les tâches et les fichiers (la vignette suit). Tests `tests/ranger.rs`.
- [x] **D. Savoir où on est** : titre de l'onglet, rappel au-dessus du titre des pages, liseré de la couleur de l'espace. Test `on_sait_toujours_dans_quel_espace_on_est`.
- [x] **G. Menu « Convertir » et onglet Outils** : sorties possibles selon le type, refus de toute autre ; fichiers de l'onglet Outils hors des listes, gardés ou effacés au bout de 24 heures. Tests `tests/conversions.rs`, tests unitaires des formats d'image et des arguments ffmpeg et Ghostscript.
  - Les vraies conversions (ffmpeg, Ghostscript, libheif) ne sont vérifiées que par les tests unitaires de leurs arguments : à essayer sur le serveur avec une photo HEIC, une vidéo `.mov` et un PDF.
- [x] **H. Suivi partout** : « 1 conversion en cours » dans la barre latérale de toutes les pages, puis « Prêt : … ». Test `une_conversion_va_jusqu_au_resultat`.
- [x] **A. Perso montre tout** : dashboard, notes, fichiers et agenda de tous les espaces, filtre « Seulement Perso ». Test `perso_montre_tous_les_espaces`.
- [x] **C. L'espace dans l'adresse** : `/e/<slug>/…`, en-tête `X-Espace` par onglet, redirections et historique qui gardent l'espace. Tests `tests/adresse.rs`.
- [x] **E. Archiver un espace** ; **I. règles par espace** ; **J. remplacer l'original** ; **F. espace du moment**. Tests `tests/espaces_suite.rs`.
- [x] **K. Déposer partout, coller une image dans une note** ; **L. la palette connaît la page** (aujourd'hui la barre de commandes). Tests `deposer_partout_et_coller_une_image`, `la_barre_connait_la_page`. Le glisser-déposer et le collage eux-mêmes (JavaScript) se vérifient dans le navigateur.
- Migrations 0011 (`files.expires_at`, `uploads.for_tools`, `jobs.target`) et 0012 (`spaces.archived_at`, `auto_mp4`, `auto_heic_jpeg`, `users.moment_space`).
- **Barre latérale détachée** (13 novembre 2026) : le glissement était saccadé ; elle a maintenant sa propre couche (`will-change`) et une courbe par sens. Mesuré dans le navigateur de la démo : une image toutes les 16 ms à l'arrivée comme au départ.

## Barre de commandes : avancement

Cahier des charges dans `SPEC.md` (« Barre de commandes »), accepté le 13 novembre 2026. Rien n'est encore vérifié sur le serveur (il faut `dashu-update`).

Lot 1 : fait, tests `tests/commandes.rs` (6), tests unitaires de la lecture d'une ligne et des jours (`src/commands/`).

- [x] `/` ouvre la barre hors d'un champ, effacer le `/` la ferme ; `Ctrl+K` et `/search/palette` n'existent plus. Test `la_barre_remplace_la_palette`.
- [x] `Tab` complète une commande, un espace, une page ; `Tab` encore passe à la suivante. Vu dans le navigateur (instance locale), au clavier et au toucher (largeur téléphone).
- [x] `/go agenda demain` ouvre l'agenda du lendemain ; `/g ag` est compris ; un début de mot ambigu est refusé. Test `aller_a_une_page`.
- [x] `/add task Réviser @ecole` depuis Perso ; `/add note Idée` ouvre l'éditeur sur une note titrée. Test `ajouter_une_tache_ou_une_note`.
- [x] `/open` trouve une note par un mot de son texte et un fichier par son nom, jamais ceux d'un autre compte ; espace archivé compris. Tests `ouvrir_une_note_ou_un_fichier`, `archiver_puis_restaurer_un_espace`, `les_notes_d_alice_sont_invisibles_et_intouchables_pour_bob`.
- [x] Sur un fichier : ses conversions et les espaces où le déplacer ; `/move` d'une note ; une commande de fichier tapée ailleurs est refusée avec un message. Tests `la_barre_connait_la_page`, `agir_sur_la_note_ou_le_fichier_ouvert`.
- [x] Depuis l'éditeur, une commande n'y perd pas la dernière frappe (envoi de secours avant la commande). Vu dans le navigateur.
- [ ] `/add file` ouvre le sélecteur de fichiers : vérifié jusqu'à l'ouverture (le sélecteur du système ne se pilote pas) ; l'envoi lui-même est celui du bouton.
- Poids : `app.js` passe de 8,6 à 14,3 Ko compressé (gzip, mesuré en local ; 14,6 Ko avec le lot 2) ; une page hors éditeur reste sous la cible de 60 Ko (environ 48 Ko). La police à chasse fixe de la barre ne se charge qu'à sa première ouverture.

Lot 2 : fait, tests `les_taches_au_clavier`, `la_note_au_clavier`, `le_fichier_au_clavier`, `les_reglages_au_clavier` (`tests/commandes.rs`).

- [x] `/add task Rendre le DM +demain` crée une tâche visible demain, pas aujourd'hui ; un jour se tape aussi par son début (`+dem`, `+mar`).
- [x] `/done` puis `/undone` cochent et décochent une tâche choisie par son titre ; `/delete task` la supprime. La confirmation (second `Entrée`) vue dans le navigateur.
- [x] `/tag`, `/untag`, `/pin`, `/delete` sur une note ; `/download` (vraie navigation, `HX-Redirect`), `/keep`, `/delete` sur un fichier.
- [x] `/theme`, `/size`, `/sidebar`, `/refresh`, `/logout`, `/add space` (on entre dans le nouvel espace), `/add agenda` (mêmes vérifications d'adresse que Réglages) font ce que font leurs boutons.
- La première synchro d'un agenda ajouté par `/add agenda` n'est testée que dans son refus (adresse non HTTPS) : les tests n'ont pas de réseau.
- [x] Variables `#date` et `#cours` (demandées le 13 novembre 2026) dans les titres, tags, noms et recherches ; `#cours` refusé hors d'un cours ; « Exercice #3 » intact. Test `date_et_cours_dans_les_textes`, test unitaire `variables`. Pas encore essayé pendant un vrai cours Pronote.
- [x] Un espace se tape par son nom actuel, plus par son identifiant d'adresse (signalé par Rémi le 13 novembre 2026 : Perso renommé « Général », puis un nouvel espace « Perso » ; `/space perso` menait à Général). Tests `les_espaces_suivent_leur_nom`, `les_espaces_par_leur_nom`.

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
  - **Écran de lancement** (12 novembre 2026) : au chargement complet d'une page connectée (ouverture de l'app, adresse tapée, rechargement), fond noir et logo Hermes/ qui se replie en H/, puis l'écran s'efface sur la page en 1,5 s. Rendu par le serveur et animé en CSS, il s'affiche dès le premier rendu sans attendre le JavaScript. Jamais pendant une navigation htmx (en-tête `HX-Request`), caché si le système demande de réduire les animations. Test `ecran_de_lancement_au_chargement_seulement`. Vérifié dans le navigateur, sur ordinateur et en taille téléphone ; pas encore sur le serveur ni sur l'iPhone.
- **Espaces libres** (12 novembre 2026) : on crée un espace (8 au plus, couleur et icône au choix), on le renomme, on le supprime depuis Réglages. Avant la suppression, une page dit ce que l'espace contient (fichiers, notes, tâches, agendas) et propose de tout transférer dans un autre espace ; sinon tout disparaît, fichiers effacés du disque. Le slug d'un espace renommé ne change pas (API, Hermès). Tests `tests/spaces.rs`.
  - La migration 0007 reconstruit la table `spaces` (ses contraintes limitaient le slug et la couleur). SQLite l'exige clés étrangères coupées, sinon tout le contenu partirait en cascade : dashu lance donc ses migrations sur une connexion à part, sans clés étrangères, puis vérifie `foreign_key_check` (`src/db.rs`). Test `migration_0007_garde_tout_le_contenu` sur une base en service simulée.
  - Un espace supprimé ne revient plus à la connexion suivante : les trois espaces de départ ne sont créés que pour un compte qui n'en a aucun.
  - Perso ne se supprime pas (13 novembre 2026) : c'est la vue d'ensemble, son agenda réunit ceux de tous les espaces, sur la page Agenda, dans le widget « Prochains cours » et dans l'API (`space=perso`). Renommé, il garde ce rôle. La migration 0008 le rend, vide, à un compte qui l'avait supprimé.
- **Dashboard modulaire** (12 novembre 2026) : grille bento propre à chaque espace (colonne `spaces.dashboard`, migration 0009). Six widgets : Agenda en grille (calendrier : aujourd'hui, trois jours ou la semaine selon la largeur, ouvert sur l'heure en cours) et en liste (prochains cours), Tâches, Notes, Fichiers, Réglages rapides, en S, M, L ou toute taille jusqu'à 4 × 4 cases ; on les déplace, agrandit, ajoute et retire en mode édition (`static/js/dashboard.js`, sans bibliothèque ni style en ligne : la taille passe par des classes). La disposition est enregistrée à chaque changement (`POST /dashboard/layout`). Le balayage entre espaces est coupé pendant l'édition. Tests `tests/dashboard.rs`.
- **Profil et affichage** (12 novembre 2026) : prénom, nom, photo de profil (initiales sur un fond tiré au hasard sans photo), taille de l'interface (zoom de toute l'app, les unités d'écran divisées par `--zoom`), barre latérale ancrée ou détachée (façon mode compact de Zen : cachée à gauche, visible au survol du bord). Réglages passe en bas de la barre latérale, le thème ne se choisit plus que dans Réglages. Migration 0010, tests `tests/profile.rs`. Les widgets du dashboard ne défilent plus : ce qui dépasse est retiré à l'arrivée ; le widget Calendrier montre les heures qui tiennent autour de l'heure en cours.
- **Le dépôt n'était pas vide.** Il contenait la page d'accueil (`website/`) et son workflow GitHub Pages : ils sont laissés tels quels. Le README, qui décrivait un autre projet (équipes, domotique, connecteurs), a été remplacé.
- **Pas de page d'inscription.** Un compte naît à la première connexion Google ou avec `dashu set-password`. Raison dans le README : sans vérification d'email, un formulaire public permettrait de créer le compte d'une adresse autorisée à la place de son propriétaire.
- **Un commit pour toute l'application**, puis un par lot (tests, déploiement et documentation), au lieu d'un commit par étape : les sept étapes ont été écrites d'un seul tenant avant la première compilation. Les évolutions suivantes passent chacune par une PR.
- **Versions** : sqlx 0.8 et reqwest 0.12 plutôt que les dernières, pour rester compatibles avec `tower-sessions-sqlx-store` et `openidconnect`. Une crate hors liste, `syntect`, colore le code des notes côté serveur.
- **Colonnes ajoutées au modèle** pour des comportements demandés : `tasks.done_at` (une tâche cochée reste visible jusqu'à minuit), `calendar_sources.last_error` et `last_attempt_at` (afficher l'échec de synchro), `users.theme`, `users.mobile_tabs`, `calendar_events.all_day`, `spaces.icon`, `spaces.dashboard` et le profil (`users.first_name`, `last_name`, `avatar`, `avatar_color`, `avatar_version`, `ui_size`, `sidebar_floating`).
- **Récurrences iCal (`RRULE`) non développées** : Pronote n'en produit pas.

Les autres limites connues sont dans le README.

## Pour la grille d'évaluation

- **Interventions** : une seule question posée (comment compiler, la machine n'ayant ni Rust, ni compilateur C, ni Docker). Rust et GCC ont été installés avec winget.
- **Bugs trouvés pendant le développement** par les tests et les essais, tous corrigés : un `<input type="password">` survivait au nettoyage d'une note ; une adresse comme `/notes/abc` répondait 400 au lieu d'une page 404 ; les couleurs des matières se répartissaient mal (quatre matières sur neuf de la même teinte) ; un retour arrière sur `/notes/new` aurait recréé une note.
- **Prix** : le nombre de jetons consommés n'est pas lisible depuis la session elle-même. Il se lit dans l'écran d'usage de l'application.
