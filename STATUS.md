# État de la v1

Mis à jour le 11 novembre 2026, branche `v1`.

**En bref.** dashu est en service sur le homelab : un conteneur LXC Debian du serveur Proxmox, joignable sur `https://app.dashu.dev` (tunnel Cloudflare) et sur le réseau Tailscale. Il sert au quotidien, sur ordinateur et sur iPhone (en app sur l'écran d'accueil). La CI GitHub tourne à chaque PR et passe. **Trois cases restent ouvertes**, toutes à faire sur le serveur : la connexion Google, une restauration réelle de sauvegarde, et les mesures de performance des pages connectées. La marche à suivre est plus bas.

Une case cochée veut dire « vérifié », et la ligne dit où : sur le serveur réel, par les tests, ou les deux.

## Critères d'acceptation

### Fonctionnel

- [ ] **Connexion par identifiants et par Google sur le même compte**
  - Identifiants : en service sur le serveur, et tests `tests/auth.rs`.
  - Google : codé (OpenID Connect, PKCE, `state`, `nonce`, rattachement par email vérifié) mais **pas encore branché** : il manque le client OAuth « Application Web ». Étape 1 plus bas.
- [ ] **Un email absent de `ALLOWED_EMAILS` est refusé, avec les deux méthodes**
  - Identifiants : test `email_hors_liste_refuse_meme_avec_le_bon_mot_de_passe` ; `dashu set-password` refuse aussi un email hors liste.
  - Google : à essayer avec un second compte Google (étape 1).
- [x] **6 tentatives de login ratées en une minute déclenchent un blocage temporaire** : test `sixieme_tentative_en_une_minute_bloquee` (5 refus, puis 429, par adresse IP). Derrière le tunnel, l'adresse vient de `CF-Connecting-IP`.
- [x] **Les 3 espaces existent à la première connexion et le changement d'espace filtre dashboard, notes et agenda** : en usage sur le serveur, et tests `trois_espaces_des_la_premiere_connexion`, `le_changement_d_espace_filtre_le_contenu`, `vue_semaine`.
- [x] **Une note créée, modifiée, taguée, retrouvée par la recherche, déplacée, puis supprimée** : test `cycle_de_vie_d_une_note`, et en usage.
- [x] **Le flux iCal Pronote Campus s'affiche correctement en vue semaine, heure d'été comprise** : le vrai flux Pronote Campus est branché sur l'espace École et s'affiche ; l'agenda de Perso réunit ceux de tous les espaces. L'heure d'été est vérifiée par les tests (`heure_d_ete_et_heure_d_hiver`, `vue_semaine_en_heure_d_ete`, avec `tests/fixtures/pronote.ics`) ; le prochain passage réel est le 28 mars 2027.
- [x] **La palette `Ctrl+K` trouve une note et ouvre une page** : vu dans le navigateur.
- [x] **Tout fonctionne sur téléphone** : utilisé sur iPhone, dans Safari et en app sur l'écran d'accueil (sans zoom, icône, barre de raccourcis en bas, changement d'espace par balayage).

### Technique

- [x] **`cargo clippy` sans avertissement, `cargo test` vert en CI** : GitHub Actions (`.github/workflows/ci.yml`) à chaque PR : `cargo fmt --check`, `clippy -D warnings`, 95 tests, puis l'image Docker. Vert sur les PR 1 à 9.
- [x] **Tests d'intégration sur : login, isolation des données entre deux utilisateurs, CRUD des notes, parsing iCal avec un `.ics` d'exemple** : `tests/auth.rs` (17), `tests/isolation.rs` (4), `tests/notes.rs` (5), `tests/calendar.rs` (12), plus `tests/api.rs` (9) et `tests/tabbar.rs` (4).
- [ ] **Cibles de performance atteintes** : page de connexion mesurée sur le serveur réel, via Cloudflare, en 4G simulée : Lighthouse mobile 97 à 99, 42 Ko de HTML + CSS + JS (tableau plus bas). L'image Docker fait 7 Mo (mesurée par la CI). **Restent** les pages connectées (dashboard, notes, agenda, éditeur), la mémoire du binaire et le temps de réponse du serveur, à mesurer sur place (étape 3).
- [x] **Aucune injection de script possible via le contenu d'une note** : tests du nettoyage (`src/notes/markdown.rs`), test `une_note_ne_peut_pas_injecter_de_script` sur toutes les pages où une note apparaît, et CSP sans `unsafe-inline` en seconde barrière.
- [ ] **Sauvegarde nocturne vérifiée par une restauration réelle** : la sauvegarde tourne chaque nuit à 3 h 30 sur le serveur (`dashu-backup.timer`, copie vérifiée par `PRAGMA integrity_check`). `dashu-restore` est testé hors du serveur avec le vrai binaire : restauration, annulation, fichier corrompu, base qui n'est pas dashu, base avec laquelle dashu ne démarre pas (retour automatique). **Reste à le faire une fois sur le serveur** (étape 2).
- [x] **README complet** : installation locale, variables d'environnement, déploiement tel qu'il tourne (LXC, systemd, Tailscale, tunnel Cloudflare), mise à jour, sauvegarde et restauration, identifiants Google. Les scripts du serveur sont versionnés dans `deploy/lxc/`.

## Ce qu'il te reste à faire

Les commandes se tapent dans le conteneur dashu (`pct enter <numéro>` sur l'hôte Proxmox), en root.

**0. Installer les nouveaux scripts (une fois).** Après la fusion de la PR qui ajoute `deploy/lxc/`, lancer `dashu-update` puis, s'il affiche « Mise à jour terminée » :

```bash
cd /opt/dashu/src && for s in dashu-update dashu-backup dashu-restore dashu-admin; do install -m 755 deploy/lxc/$s /usr/local/sbin/; done && git rev-parse HEAD > /etc/dashu/deployed-commit
```

Ensuite, `dashu-update` remet ces scripts à jour tout seul.

**1. Connexion Google** (15 min). Créer le client OAuth comme décrit dans le README (section « Créer les identifiants Google »), avec deux utilisateurs tests : ton adresse, et une seconde adresse Google qui n'est **pas** dans `ALLOWED_EMAILS`. Dans `/etc/dashu/dashu.env` : `BASE_URL=https://app.dashu.dev`, `GOOGLE_CLIENT_ID=…`, `GOOGLE_CLIENT_SECRET=…`, puis `systemctl restart dashu`. Dans une fenêtre de navigation privée sur `https://app.dashu.dev` :
  - « Continuer avec Google » avec ton adresse : tu arrives sur ton dashboard, avec tes notes (même compte) ;
  - même chose avec la seconde adresse : retour à la page de connexion, accès refusé.

**2. Restauration réelle** (10 min). Sans rien perdre :
  1. `dashu-restore` : liste les sauvegardes. Choisir celle de la nuit (`dashu-…_0330….db`).
  2. `dashu-restore dashu-…_0330….db` : dashu repart sur la base de la nuit. La base du moment est mise de côté, le script donne son nom (`avant-restauration-….db`).
  3. Vérifier dans le navigateur que tout est là, tel que la veille au soir (il faudra peut-être se reconnecter).
  4. `dashu-restore avant-restauration-….db` : retour à la base du moment. Vérifier que les changements du jour sont revenus.

**3. Performance** (10 min).
  - Lighthouse : Chrome sur ordinateur, connecté à `https://app.dashu.dev`, outils de développement > Lighthouse > mode **Navigation**, appareil **Mobile**, catégorie **Performances**. Une mesure sur le dashboard, les notes, l'agenda et l'éditeur d'une note. Lighthouse simule une connexion 4G lente.
  - Mémoire au repos : `systemctl show dashu -p MemoryCurrent` (cible : moins de 50 Mo).
  - Temps de réponse du serveur, hors réseau : `for i in $(seq 20); do curl -s -o /dev/null -w '%{time_starttransfer}\n' http://127.0.0.1:3000/login; done` (cible : moins de 0,020 s).

**À part.** Désactiver le script de mesure que Cloudflare injecte dans les pages (README, « Tunnel Cloudflare », point 4), et mettre les sauvegardes sur le second disque du serveur (README, fin de « Sauvegardes et restauration »).

## Mesures de performance

### Sur le serveur réel

Page de connexion, `https://app.dashu.dev/login`, via Cloudflare, Lighthouse 12 en mode mobile avec la 4G lente simulée, trois passages le 11 novembre 2026.

| Mesure | Cible | Mesuré |
| --- | --- | --- |
| Poids d'une page hors éditeur (HTML + CSS + JS, compressés) | < 60 Ko | 42 Ko (HTML 2 Ko, CSS 15 Ko, htmx 17 Ko, `app.js` 7 Ko) ; plus la police, 35 Ko |
| Score Lighthouse Performance (mobile) | ≥ 95 | 97, 98, 99 (première image à 1,6-1,9 s, aucun décalage de mise en page) |
| Poids de la page éditeur | < 250 Ko | à mesurer (étape 3) |
| Temps de réponse serveur d'une page | < 20 ms | à mesurer (étape 3) |
| Mémoire du binaire au repos | < 50 Mo | à mesurer (étape 3) |
| Taille de l'image Docker | < 30 Mo | 7 Mo (CI) |

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
- **Sauvegardes sur le même disque que la base**, pas encore sur le second disque du serveur comme le demande la spec. Elles protègent d'une erreur ou d'une mise à jour ratée, pas d'une panne de disque.
- **Ajouts hors spec**, faits à la demande après la v1 de base : API JSON pour un assistant (Hermès) avec une administration SSH limitée à six actions, agenda de Perso qui réunit tous les espaces, événements de plus de 24 h dans le bandeau « journée », « île » de changement d'espace avec balayage, barre de raccourcis sur téléphone, app installable sur l'écran d'accueil.
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
