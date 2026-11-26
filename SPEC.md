# dashu — Cahier des charges v1

Nov 10, 2026 · @Rémi Moreau

## Contexte et objectifs

dashu.dev est un espace de travail personnel tout-en-un, accessible en ligne, qui regroupe dans une seule interface les outils du quotidien pour trois usages : perso, école (BTS SIO SISR) et travail (alternance consultant ERP).

Objectifs de la v1 :

- **Une seule porte d'entrée** : un login (identifiants ou Google) donne accès à tout.
- **Interface 100 % sur mesure** : aucun dashboard tout fait, aucune app tierce visible. Des bibliothèques open source légères sont autorisées en dessous.
- **Rust au maximum** : backend, logique métier et outils lourds côté navigateur en Rust.
- **Rapide et fluide** : chargement quasi instantané, navigation sans rechargement complet.
- **Projet fini** : une v1 réduite mais complète et déployée, plutôt qu'une v1 ambitieuse jamais terminée.

Ce document sert aussi de base de test : il est donné tel quel à Claude Fable 5.2 pour évaluer sa capacité à réaliser le projet (voir la dernière section).

## Périmètre v1

La v1 couvre cinq modules. Tout le reste est explicitement hors périmètre et listé dans la roadmap.

| Module | Inclus en v1 | Hors v1 |
| --- | --- | --- |
| Authentification | Identifiants + Google, sessions, déconnexion, rate limiting | 2FA, gestion multi-utilisateurs avancée |
| Espaces | Perso, École, Taff ; bascule rapide ; tout contenu rattaché à un espace | Espaces créés librement par l'utilisateur |
| Dashboard d'accueil | Widgets par espace : prochains cours, tâches du jour, dernières notes, horloge/date | Widgets configurables par glisser-déposer |
| Notes Markdown | Créer, éditer (une seule vue mise en forme, menu `/`), supprimer, tags, recherche plein texte | Partage public, collaboration temps réel |
| Emploi du temps | Import iCal Pronote Campus par URL, vue jour/semaine, rafraîchissement auto | Édition des cours, synchro Google Calendar |

Hors périmètre v1 : cloud de fichiers, convertisseur, outils PDF, retouche photo, TTS, transcription, assistant IA. Ils arrivent dans les versions suivantes.

## Stack technique

Rust côté serveur avec du HTML rendu par le serveur et HTMX pour l'interactivité. C'est la combinaison la plus rapide au chargement pour un dashboard de ce type. Les versions des crates (bibliothèques Rust) sont choisies à jour au moment du développement, pas figées dans ce document.

| Couche | Choix | Rôle |
| --- | --- | --- |
| Runtime | tokio | Exécution asynchrone, base de tout le serveur |
| Serveur web | axum + tower-http | Routes, compression gzip/brotli, fichiers statiques, logs |
| Templates | askama | HTML compilé avec le binaire, erreurs détectées à la compilation |
| Interactivité | htmx (≈ 14 Ko) | Remplacement partiel de page, navigation sans rechargement (`hx-boost`) |
| Base de données | SQLite via sqlx | Fichier unique, requêtes vérifiées à la compilation, migrations intégrées |
| Recherche | SQLite FTS5 | Recherche plein texte dans les notes, sans service externe |
| Mots de passe | argon2 | Hachage recommandé pour les mots de passe |
| Sessions | tower-sessions (store SQLite) | Cookie de session sécurisé côté serveur |
| Login Google | openidconnect | OAuth2 / OpenID Connect avec Google |
| Anti-bruteforce | tower\_governor | Limite de tentatives sur les routes de login |
| Markdown | pulldown-cmark + ammonia | Conversion Markdown → HTML, puis nettoyage contre le XSS |
| iCal | icalendar + reqwest | Téléchargement et lecture du flux Pronote Campus |
| Dates | chrono + chrono-tz | Fuseau Europe/Paris, heure d'été |
| Éditeur de notes | ProseMirror + markdown-it (JS) | Seule brique JS significative, chargée uniquement sur la page éditeur. Remplace CodeMirror 6 depuis le 13 novembre 2026 (justification dans le README) |
| Styles | CSS écrit à la main, variables CSS | Design sur mesure, pas de framework CSS |

Règle : aucune dépendance front en dehors de htmx et de l'éditeur (ProseMirror, markdown-it) sans justification écrite dans le README.

## Architecture et structure du projet

Tout dashu tient dans un seul binaire Rust qui sert les pages, gère l'authentification et synchronise l'agenda. Il est placé derrière un tunnel Cloudflare.

&#91;embedded content: architecture v1 · 1 binaire Rust, 2 conteneurs\]

Le tunnel est ouvert depuis le serveur vers Cloudflare, donc aucun port de la box n'est exposé. Pour la connexion Google, le navigateur est redirigé chez Google, puis dashu vérifie le jeton reçu.

**Structure du dépôt** : un dossier par module, chacun avec ses routes, ses handlers, ses requêtes SQL et ses templates.

```text
dashu/
├── Cargo.toml
├── SPEC.md                 ce cahier des charges
├── migrations/             migrations SQL versionnées
├── src/
│   ├── main.rs             démarrage, config, routeur
│   ├── config.rs           variables d'environnement
│   ├── db.rs               pool SQLite, mode WAL, migrations
│   ├── auth/               identifiants, Google, sessions, middleware
│   ├── spaces/             espaces et espace actif
│   ├── dashboard/          page d'accueil et widgets
│   ├── notes/              CRUD, tags, rendu Markdown
│   ├── calendar/           routes + synchro iCal en tâche de fond
│   ├── commands/           barre de commandes « / », FTS5
│   └── web/                erreurs, CSRF, en-têtes de sécurité
├── templates/              askama : layout, pages, fragments htmx
├── static/                 css, js (htmx, éditeur ProseMirror), icônes, polices
├── tests/                  tests d'intégration + fixtures/ (.ics)
├── scripts/backup.sh
├── Dockerfile
├── docker-compose.yml      dashu + cloudflared
└── .env.example
```

## Authentification et sécurité

Deux méthodes de connexion qui mènent au même compte. Le site est exposé sur Internet, donc la sécurité fait partie de la v1, pas d'une version ultérieure.

**Connexion par identifiants**

- Email + mot de passe, haché en Argon2id avec un sel par utilisateur.
- Mot de passe : 12 caractères minimum.
- Message d'erreur identique que l'email existe ou non (pas de fuite d'information).

**Connexion Google**

- Flux OpenID Connect « authorization code » avec PKCE et vérification du `state` et du `nonce`.
- Liaison au compte par l'email vérifié renvoyé par Google.
- Identifiants Google (client ID et secret) créés dans la Google Cloud Console, gratuits, stockés en variables d'environnement.

**Inscription**

- Fermée par défaut : seul un email présent dans `ALLOWED_EMAILS` (variable d'environnement) peut créer un compte ou se connecter via Google. dashu est un outil perso, pas un service public.

**Sessions et protections**

- Cookie `HttpOnly`, `Secure`, `SameSite=Lax`, durée 30 jours glissants, régénéré à la connexion.
- Protection CSRF sur tous les formulaires et requêtes HTMX qui modifient des données (jeton dans un en-tête).
- Rate limiting : 5 tentatives de login par minute et par IP.
- En-têtes de sécurité : Content-Security-Policy stricte, `X-Content-Type-Options`, `Referrer-Policy`.
- HTML issu du Markdown systématiquement nettoyé par ammonia.
- Aucun secret dans le dépôt Git : fichier `.env.example` uniquement.

## Modèle de données et espaces

Chaque contenu appartient à un utilisateur et à un espace. L'espace actif est stocké dans la session et filtre toutes les pages.

| Table | Colonnes principales | Remarques |
| --- | --- | --- |
| `users` | id, email (unique), password\_hash (nullable), google\_sub (nullable), created\_at | Un compte peut avoir un mot de passe, Google, ou les deux |
| `sessions` | gérée par tower-sessions | — |
| `spaces` | id, user\_id, slug (`perso`, `ecole`, `taff`), name, color, position | Les 3 espaces sont créés automatiquement à la première connexion |
| `notes` | id, user\_id, space\_id, title, body\_md, pinned, created\_at, updated\_at | `body_md` = source Markdown ; le HTML est généré à l'affichage |
| `notes_fts` | table virtuelle FTS5 sur title + body\_md | Tenue à jour par des triggers SQL |
| `tags` / `note_tags` | id, user\_id, name / note\_id, tag\_id | Relation plusieurs-à-plusieurs |
| `tasks` | id, user\_id, space\_id, title, done, due\_date, created\_at | Minimaliste en v1 : sert au widget « tâches du jour » |
| `calendar_sources` | id, user\_id, space\_id, name, ical\_url, last\_fetched\_at | L'URL Pronote est un secret : jamais affichée en entier |
| `calendar_events` | id, source\_id, uid, title, location, starts\_at, ends\_at | Cache local, remplacé à chaque synchro |

Règles :

- Toutes les dates sont stockées en UTC et affichées en Europe/Paris.
- Chaque requête filtre sur `user_id` : un utilisateur ne voit jamais les données d'un autre, même si l'inscription s'ouvre un jour.
- Les migrations sqlx sont versionnées dans `migrations/` et appliquées au démarrage.

## Fonctionnalités v1 détaillées

### Pages et routes

| Route | Contenu |
| --- | --- |
| `/login` | Formulaire identifiants + bouton « Continuer avec Google » |
| `/auth/google`, `/auth/google/callback` | Flux OpenID Connect |
| `/logout` | Déconnexion (POST) |
| `/` | Dashboard de l'espace actif |
| `/notes`, `/notes/new`, `/notes/{id}` | Liste, création, édition d'une note |
| `/calendar` | Emploi du temps, vue semaine par défaut |
| `/settings` | Mot de passe, compte Google lié, sources iCal |
| `/health` | Réponse `ok` pour la supervision, sans authentification |

### Shell de l'application

- Barre latérale : logo dashu, sélecteur d'espace (Perso / École / Taff, chacun avec sa couleur), liens Dashboard, Notes, Agenda, Réglages.
- Changement d'espace en un clic, sans rechargement complet (HTMX).
- **Palette de commandes** `Ctrl+K` / `Cmd+K` : recherche dans les notes et accès rapide aux pages et actions (« nouvelle note », « changer d'espace »). *Remplacée le 13 novembre 2026 par la barre de commandes `/` (voir « Barre de commandes », en fin de document).*
- Raccourcis clavier : `N` nouvelle note, `G` puis `D` / `N` / `C` pour aller au Dashboard / Notes / Calendrier.

### Dashboard

- En-tête : date du jour, heure, salutation.
- Widget **Prochains cours** : les 3 prochains événements de l'espace actif, avec salle et heure.
- Widget **Tâches du jour** : ajouter, cocher, supprimer, en place.
- Widget **Notes récentes** : 5 dernières notes modifiées, plus les notes épinglées.
- Chaque widget est un fragment HTMX chargé indépendamment : un widget lent ne bloque pas les autres.

### Notes

- Une seule vue, comme une page Notion (décidé le 13 novembre 2026, à la place de l'éditeur CodeMirror et de l'aperçu à côté) : le texte s'édite mis en forme. `/` ouvre le menu des blocs : texte, titres 1 à 3, listes à puces, numérotées ou à cocher, tableau, citation, séparateur, bloc de code, lien (adresse, note ou fichier), image, fichier, date du jour. Les raccourcis Markdown marchent pendant la frappe ; une sélection montre une barre de mise en forme.
- Le Markdown reste le format enregistré : seuls existent les blocs que le Markdown sait écrire. Le serveur le met en forme (nettoyé par ammonia) en attendant le chargement de l'éditeur.
- Sauvegarde automatique toutes les 2 secondes après une modification, indicateur « Enregistré ».
- Tags ajoutables depuis l'éditeur, filtre par tag dans la liste.
- Recherche plein texte avec extrait surligné du passage trouvé.
- Épingler, supprimer (avec confirmation), déplacer vers un autre espace.
- Support : titres, listes, cases à cocher, code (coloré dans le rendu du serveur), tableaux, liens, images.

### Emploi du temps

- Ajout d'une source iCal par URL (celle de Pronote Campus) rattachée à un espace.
- Synchro automatique toutes les heures en tâche de fond, plus un bouton « Rafraîchir ».
- Vues jour et semaine, navigation semaine précédente / suivante, ligne « maintenant ».
- Événements colorés selon la matière (couleur dérivée du titre du cours).
- En cas d'échec de synchro : dernière version en cache affichée avec la date de dernière synchro réussie.

## Interface et design sur mesure

L'identité visuelle est propre à dashu : sobre, dense en information, pensée pour travailler longtemps dessus. Aucun look de template ni de dashboard générique.

- **Thèmes** : sombre par défaut, clair disponible, bascule enregistrée par utilisateur ; respect de `prefers-color-scheme` à la première visite.
- **Couleurs** : définies uniquement en variables CSS (`--bg`, `--surface`, `--text`, `--accent`…) ; une couleur d'accent par espace (Perso, École, Taff) qui teinte la barre latérale et les éléments actifs.
- **Typographie** : une police sans-serif lisible pour l'interface et une police monospace pour l'éditeur et le code, auto-hébergées (pas d'appel à Google Fonts).
- **Icônes** : un jeu SVG open source léger, intégré en sprite unique (pas de police d'icônes).
- **Responsive** : utilisable sur téléphone (barre latérale repliée en menu), confortable sur écran large.
- **À éviter** : dégradés violets, effets de lueur, emojis dans l'interface, badges « Nouveau », animations décoratives.
- **Animations** : uniquement fonctionnelles (apparition d'un fragment HTMX, ouverture de la palette), moins de 150 ms, désactivées si `prefers-reduced-motion`.
- **Accessibilité** : navigation complète au clavier, focus visible, contrastes conformes WCAG AA, libellés sur tous les boutons d'icône.

Livrable attendu avant le code des pages : une page `/styleguide` (accessible seulement connecté) qui montre couleurs, typographie, boutons, champs, cartes et widgets dans les deux thèmes.

## Exigences de performance

Les cibles sont mesurées sur le serveur homelab réel, via Cloudflare, depuis un navigateur en 4G.

| Mesure | Cible |
| --- | --- |
| Poids d'une page hors éditeur (HTML + CSS + JS, compressés) | < 60 Ko |
| Poids de la page éditeur (avec l'éditeur) | < 250 Ko |
| Temps de réponse serveur d'une page (hors réseau) | < 20 ms |
| Score Lighthouse Performance (mobile) | ≥ 95 |
| Mémoire du binaire au repos | < 50 Mo |
| Taille de l'image Docker | < 30 Mo |

Moyens imposés :

- Compilation en mode `release` avec LTO (optimisation à l'édition de liens).
- Fichiers statiques versionnés par empreinte dans le nom, cache navigateur d'un an.
- Compression brotli/gzip sur toutes les réponses texte.
- L'éditeur (ProseMirror) chargé uniquement sur la page éditeur.
- SQLite en mode WAL (lectures et écritures simultanées sans blocage).

## Déploiement homelab + Cloudflare Tunnel

dashu tourne sur le serveur Proxmox de la maison et reste joignable depuis Internet sans ouvrir de port sur la box. Coût : 0 € (hors nom de domaine déjà possédé).

1. **Domaine** : les serveurs DNS de `dashu.dev` sont gérés par Cloudflare (plan gratuit). Le `.dev` impose le HTTPS, Cloudflare fournit le certificat.
2. **Machine** : un conteneur LXC Debian sur Proxmox, avec Docker et Docker Compose.
3. **Conteneurs** (un `docker-compose.yml` dans le dépôt) :
   - `dashu` : image multi-étapes, compilation Rust puis binaire seul dans une image minimale ; base SQLite dans un volume `/data`.
   - `cloudflared` : le tunnel Cloudflare, qui relaie `dashu.dev` vers `dashu:3000` sur le réseau interne Docker.
4. **Configuration** : variables d'environnement dans un `.env` non versionné (`DATABASE_URL`, `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `ALLOWED_EMAILS`, `SESSION_SECRET`, `BASE_URL`, `TUNNEL_TOKEN`).
5. **Sauvegardes** : copie à chaud de la base SQLite chaque nuit (commande `VACUUM INTO`) vers le second disque du serveur, conservation 14 jours, script fourni dans `scripts/backup.sh`.
6. **Supervision** : `/health` interrogé par Uptime Kuma ou un équivalent ; logs structurés lisibles par `docker logs`.
7. **CI** (GitHub Actions) : à chaque push, `cargo fmt --check`, `cargo clippy`, `cargo test`, puis construction de l'image Docker.

Mise à jour en production : `git pull && docker compose up -d --build`, décrite dans le README.

## Critères d'acceptation et tests

La v1 est terminée quand toutes les cases sont cochées sur le serveur réel.

**Fonctionnel**

- [ ] Connexion par identifiants et par Google sur le même compte
- [ ] Un email absent de `ALLOWED_EMAILS` est refusé, avec les deux méthodes
- [ ] 6 tentatives de login ratées en une minute déclenchent un blocage temporaire
- [ ] Les 3 espaces existent à la première connexion et le changement d'espace filtre dashboard, notes et agenda
- [ ] Une note créée, modifiée, taguée, retrouvée par la recherche, déplacée, puis supprimée
- [ ] Le flux iCal Pronote Campus s'affiche correctement en vue semaine, heure d'été comprise
- [ ] La palette `Ctrl+K` trouve une note et ouvre une page
- [ ] Tout fonctionne sur téléphone

**Technique**

- [ ] `cargo clippy` sans avertissement, `cargo test` vert en CI
- [ ] Tests d'intégration sur : login, isolation des données entre deux utilisateurs, CRUD des notes, parsing iCal (avec un fichier `.ics` d'exemple dans `tests/fixtures/`)
- [ ] Cibles de performance atteintes (section dédiée)
- [ ] Aucune injection de script possible via le contenu d'une note
- [ ] Sauvegarde nocturne vérifiée par une restauration réelle
- [ ] README complet : installation locale, variables d'environnement, déploiement, création des identifiants Google

## Roadmap v2 et suivantes

Chaque version ne démarre que lorsque la précédente est déployée et utilisée au quotidien.

| Version | Contenu | Briques envisagées |
| --- | --- | --- |
| v2 — Fichiers et outils | Fichiers par espace, partage par lien temporaire, onglet Outils (conversions, téléchargeur) : voir le cahier des charges v2 en fin de document | Stockage sur le second disque + métadonnées SQLite, envoi en continu axum ; ffmpeg, pandoc, poppler, qpdf en sous-processus isolés |
| v3 — OCR | Texte des images et des PDF scannés (les conversions prévues ici sont avancées en v2) | Tesseract en sous-processus |
| v4 — Médias | Retouche photo dans le navigateur, redimensionnement, détourage | `photon-rs` compilé en WASM (îlot Rust côté client) ; détourage via un modèle ONNX |
| v5 — Audio | Lecture audio des notes (TTS), transcription de vocaux et de cours | Piper ou Kokoro pour le TTS, `whisper-rs` pour la transcription |
| v6 — IA | Assistant qui cherche dans les notes et fichiers | API Claude, recherche FTS5 puis recherche sémantique |
| Plus tard | Kanban, suivi des heures d'alternance, flashcards, veille RSS, 2FA, boîte à outils IT | Modules maison en Rust |

Une version terminée et utilisée vaut mieux que trois versions commencées.

## Protocole de test pour évaluer Fable 5.2

Le test mesure ce que Fable 5.2 livre seul à partir de ce document, puis combien d'aide il lui faut pour finir.

**Conditions**

1. Dépôt GitHub vide `dashu`, ce cahier des charges exporté en Markdown à la racine (`SPEC.md`).
2. Fable 5.2 sélectionné comme modèle, dans Claude Code ou dans un chat Claude avec accès au dépôt.
3. Prompt de départ, identique pour tout futur test d'un autre modèle :

```markdown
Tu développes dashu à partir de SPEC.md, dans ce dépôt vide.
Respecte strictement la stack, le périmètre v1 et les critères d'acceptation.
Travaille par étapes : styleguide, auth, espaces, notes, agenda, dashboard, déploiement.
À chaque étape : code, tests, commit. Ne me pose une question que si la spec ne permet pas de trancher.
À la fin, coche les critères d'acceptation atteints dans un fichier STATUS.md.
```

4. Pendant le test, ne pas corriger le code soi-même : uniquement répondre aux questions et signaler les bugs constatés.

**Grille d'évaluation**

| Critère | Mesure | Note /5 |
| --- | --- | --- |
| Respect de la spec | Critères d'acceptation cochés et vérifiés à la main |  |
| Autonomie | Nombre d'interventions nécessaires jusqu'à une v1 qui tourne |  |
| Qualité du code | Clippy, structure, lisibilité, tests présents et utiles |  |
| Sécurité | Login, isolation des données, XSS, secrets hors du dépôt |  |
| Performance | Cibles de la section performance atteintes |  |
| Design | Rendu sur mesure, cohérent, sans look générique |  |
| Déploiement | En ligne sur dashu.dev via le tunnel en suivant le README |  |
| Prix | estime en prix x token ce que cette tâche à coûté |  |

Noter aussi la durée totale et les bugs trouvés après livraison : c'est ce qui dit le plus sur la fiabilité réelle.

---

# dashu — Cahier des charges v2

Nov 11, 2026 · @Rémi Moreau

## Contexte et objectifs

La v1 est terminée et sert au quotidien (voir `STATUS.md`). La v2 ajoute **les fichiers** et un **onglet Outils**. Option retenue le 11 novembre 2026 : des fichiers en « version courte », sans dossiers, et les conversions prévues en v3 avancées ici. *Les dossiers sont ajoutés le 13 novembre 2026 : voir « Dossiers », en fin de document.*

- **Un endroit pour ses fichiers**, rangés par espace comme les notes, retrouvés par tags et recherche.
- **Des conversions « tout en un »** : on dépose un fichier, dashu propose les formats possibles.
- **Rien d'exécuté** : aucun fichier envoyé ne peut lancer de code dans la page de dashu, et aucune conversion ne peut toucher au reste du serveur.
- **Livrée par morceaux** : chaque PR est utilisable seule, dans l'ordre donné à la fin.

Les règles de la v1 restent valables : stack, sécurité, cibles de performance, design, critères vérifiés sur le serveur réel.

## Périmètre v2

| Module | Inclus en v2 | Hors v2 |
| --- | --- | --- |
| Fichiers | Envoi par glisser-déposer (plusieurs fichiers), tags, recherche, aperçu, téléchargement, renommage, déplacement entre espaces, suppression | Client de synchronisation, versions d'un fichier (les dossiers, d'abord exclus, arrivent le 13 novembre 2026 : voir « Dossiers ») |
| Notes | Image collée dans l'éditeur, lien vers un fichier de l'espace | Galerie, redimensionnement dans la note |
| Partage | Lien temporaire (1 h, 1 jour ou 7 jours), révocable | Lien protégé par mot de passe, dépôt par un tiers |
| Outils : conversions | Images, Markdown et texte, PDF, audio et vidéo | DOCX et PPTX vers PDF (pas de LibreOffice), OCR (v3), retouche photo (v4) |
| Outils : téléchargeur | YouTube, Instagram et TikTok vers MP4 ou MP3, réservé au compte de Rémi | Tout autre site, partage du résultat |
| API | En option : lister et ajouter un fichier (pour Hermès) | Suppression de fichier par l'API |

## Prérequis serveur

1. **Second disque.** Le disque de 465 Go est vidé, puis déclaré comme stockage Proxmox (LVM-thin, nommé `ssd2`). dashu n'y prend que **50 Go** ; le reste reste libre pour autre chose. Deux volumes, montés dans le conteneur dashu par des points de montage Proxmox (Proxmox gère lui-même les droits dans un conteneur non privilégié) :
   - `mp0`, 45 Go, sur `/srv/dashu` : `fichiers/` et `tmp/` (conversions en cours) ;
   - `mp1`, 5 Go, sur `/var/backups/dashu` : les sauvegardes de la base passent sur le second disque sans changer une ligne des scripts, qui écrivent déjà là.
   Un volume se rallonge à chaud si besoin (`pct resize`).
2. **Sauvegardes croisées.** Chaque donnée existe sur deux disques physiques :
   - la base (premier disque) est sauvegardée chaque nuit sur le second disque (`mp1`), comme le demandait déjà la v1 ;
   - les fichiers (second disque) sont copiés chaque nuit sur le premier disque, dans un volume dédié créé à l'étape 6 (50 Go au plus, autant que les fichiers). La copie est incrémentale : un fichier inchangé n'est pas recopié, et 14 jours d'historique sont gardés.
3. **Mémoire** du conteneur : de 2 à 4 Go (`pct set <n> --memory 4096`). L'hôte en a 16 ; gratuit.
4. **Paquets** : ffmpeg, pandoc, poppler-utils, qpdf, ghostscript, libheif-examples ; Typst, yt-dlp et Deno en binaires officiels (trop anciens ou absents dans Debian). Pas de LibreOffice. `deploy/lxc/install.sh` et le README sont mis à jour.

## Fichiers

### Modèle de données

| Table | Colonnes principales | Remarques |
| --- | --- | --- |
| `files` | id, user\_id, space\_id, name, mime, size, sha256, storage\_key, created\_at, updated\_at | `storage_key` : 32 octets aléatoires, seul nom sur le disque |
| `files_fts` | table virtuelle FTS5 sur name | Tenue à jour par des triggers, comme les notes |
| `file_tags` | file\_id, tag\_id | Mêmes tags que les notes |
| `share_links` | id, file\_id, token\_hash, expires\_at, revoked\_at, created\_at | Le lien en clair n'est jamais stocké |
| `users.quota_bytes` | plafond de stockage du compte, en octets | NULL = la valeur par défaut (`FILES_QUOTA`) ; écrit seulement par `dashu quota` |
| `quota_changes` | id, email, before\_bytes, after\_bytes, changed\_at | Journal des changements de plafond |

Sur le disque, un fichier vit dans `/srv/dashu/fichiers/<2 premiers caractères de la clé>/<clé>`. Le nom choisi par l'utilisateur n'apparaît jamais dans un chemin, ce qui ferme la porte aux noms piégés (`../`).

### Envoi

- Glisser-déposer ou bouton, plusieurs fichiers à la fois, barre de progression par fichier.
- Envoi en continu (streaming) : le fichier va directement sur le disque, la mémoire de dashu ne dépend pas de sa taille.
- Taille maximale : `MAX_UPLOAD_MB`, 2 Go par défaut. **Par `app.dashu.dev`, Cloudflare limite une requête à 100 Mo** (offre gratuite) : le fichier part donc en morceaux de 50 Mo, recollés par dashu. Un morceau raté se renvoie seul, et un envoi en cours survit à un redémarrage. Ajouté le 11 novembre 2026, à la demande de Rémi.
- Un envoi interrompu ou refusé ne laisse rien sur le disque.

### Page Fichiers

- Liste de l'espace actif : nom, taille, date, tags ; tri par date ou par nom ; filtre par tag ; recherche sur le nom.
- Les noms de fichiers apparaissent dans la palette `Ctrl+K`.
- Actions : télécharger, renommer, déplacer vers un autre espace, ajouter un tag, supprimer (avec confirmation).
- Le dashboard gagne un widget « Fichiers récents » (5 derniers).

### Aperçu et sécurité du contenu

Un fichier envoyé est une donnée, jamais une page. Toute réponse qui sert un fichier porte `X-Content-Type-Options: nosniff` et `Content-Security-Policy: sandbox`.

| Type | Aperçu |
| --- | --- |
| PNG, JPEG, WebP, GIF | Image dans la page |
| PDF | Lu en entier dans la page par pdf.js (Mozilla) : pages dessinées au fil du défilement, zoom, texte sélectionnable. Le PDF arrive comme une donnée (`application/octet-stream`, `attachment`, `sandbox`) : le navigateur ne l'affiche jamais lui-même (revu le 18 novembre 2026, voir « Reste à trancher ») |
| Texte, Markdown, code | Rendu par le serveur dans un bloc échappé (le Markdown passe par ammonia, comme les notes) |
| HTML, SVG, tout le reste | Pas d'aperçu : téléchargement (`Content-Disposition: attachment`) |

### Notes

- Une image collée dans l'éditeur est envoyée comme fichier de l'espace, puis insérée en Markdown : `![nom](/files/<id>/raw)`.
- ammonia n'accepte une image que si son adresse est un fichier de dashu ; la CSP (`img-src 'self'`) bloque le reste.
- Un fichier se lie à une note par un lien Markdown, proposé par la palette.

### Partage par lien

- Durée au choix : 1 heure, 1 jour ou 7 jours. Révocable à tout moment depuis la page Fichiers.
- Le lien contient 32 octets aléatoires ; seule leur empreinte SHA-256 est en base.
- La page publique `/s/<jeton>` ne montre que le nom et la taille, et télécharge le fichier. Elle est limitée en nombre de requêtes, comme le login.
- Lien expiré, révoqué ou inconnu : même page « lien invalide », sans dire lequel des trois.

### Quota de stockage

Ajouté le 25 novembre 2026, à la demande de Rémi.

**Règle**

- Chaque compte a un plafond de stockage : **30 Gio par défaut** (30 × 1024³ octets). La valeur par défaut est une constante de la configuration du serveur (`FILES_QUOTA`, 30G sans elle), pas un réglage de l'interface.
- Le plafond d'un compte (colonne `users.quota_bytes`, NULL = la valeur par défaut) ne se change **que par la commande `dashu quota`**, sur le serveur. Aucune route HTTP, aucun écran de réglages, aucune action de l'API ni du connecteur Claude (donc d'Hermès) ne le change, même pour l'administrateur connecté au site. Changer la valeur par défaut s'applique à tous les comptes sans plafond à eux.

**Commande** (dans le binaire, comme `dashu backup` ; `dashu-quota` la lance sous l'utilisateur dashu, avec sa configuration)

- `dashu quota list` : chaque compte avec utilisé, plafond et pourcentage ; puis le total des plafonds attribués, les fichiers déjà stockés et la place libre sur le disque des fichiers.
- `dashu quota show <email>` : usage, plafond et derniers changements du compte.
- `dashu quota set <email> <taille>` : accepte 50G, 500M, 1,5T (unités binaires). Affiche l'ancien et le nouveau plafond, et journalise le changement (table `quota_changes` : date, compte, avant, après).
- `dashu quota reset <email>` : retour à la valeur par défaut, journalisé de même.
- Avertissement, sans rien bloquer, quand la somme des plafonds dépasse la place réelle du disque (fichiers déjà stockés + place libre).
- Refus propre, sans rien écrire : taille négative, nulle, sans unité ou d'un format inconnu ; compte inexistant.

**Ce qui compte**

- Tous les fichiers du compte (`files.user_id`, celui qui les a envoyés), tous espaces confondus, espaces partagés compris : fichiers envoyés, images collées et pièces jointes des notes, résultats de conversion rangés. Le contenu de la corbeille compte jusqu'à la suppression définitive.
- Ne comptent pas : les fichiers de l'onglet Outils pas encore gardés (Temporaire, effacés après 24 heures), les sauvegardes, les liens de partage (ils désignent un fichier déjà compté), les vignettes et aperçus.
- L'usage est la somme des tailles enregistrées en base, jamais un parcours du disque.

**À l'envoi**

- Refus immédiat si la taille annoncée dépasse la place restante : HTTP 413, « Espace plein : 29,6 Go utilisés sur 30 Go. Supprime des fichiers ou demande à l'administrateur d'augmenter ton plafond. »
- Les octets sont comptés pendant la réception, jamais d'après `Content-Length` : l'envoi est coupé dès le plafond franchi, et son fichier partiel supprimé.
- Le quota est revérifié dans la transaction SQLite qui enregistre le fichier, ouverte avec le verrou d'écriture (`BEGIN IMMEDIATE`) : deux envois en parallèle ne dépassent pas le plafond à eux deux.
- Un plafond baissé sous l'usage ne supprime rien : seuls les nouveaux envois sont refusés.
- Même quota pour l'envoi par l'API (Hermès) et par le connecteur Claude, pour un fichier de Temporaire qu'on garde dans un espace (il se met à compter), et pour le résultat d'une conversion rangé dans un espace (la tâche échoue avec le même message).

**Interface**

- Une jauge sur la page Fichiers, « 12,4 Go / 30 Go », en couleur d'alerte à partir de 90 %.
- Aucun bouton ni formulaire pour changer le plafond.

## Outils : conversions

Une zone de dépôt : on lâche un fichier, dashu affiche les sorties possibles selon son type.

| Entrée | Sorties | Outil |
| --- | --- | --- |
| PNG, JPEG, WebP, GIF, HEIC (photos iPhone) | PNG, JPEG, WebP, PDF | crate `image` ; HEIC décodé par libheif (`heif-dec`) |
| Markdown, TXT | PDF, HTML, DOCX | pandoc (moteur PDF : Typst) |
| PDF | Images, texte ; fusion, découpe, compression | poppler-utils (images, texte), qpdf (fusion, découpe), Ghostscript (compression : qpdf seul ne recompresse pas les images) |
| Vidéo, audio | MP4, MP3, compression | ffmpeg |

### Compression (avancée le 11 novembre 2026)

Demandée par Rémi avec l'envoi par morceaux : un bouton « Compresser » sur une vidéo, une photo (HEIC compris) ou un PDF crée une copie plus légère, « nom (compressé) », dans le même espace ; l'original est gardé. Vidéo : MP4 H.264, 1080p au plus, CRF 23, AAC 128 kb/s, HDR ramené en SDR. Photo : JPEG, 2560 px au plus, qualité 82. PDF : Ghostscript « ebook ». Une copie qui ne gagne rien n'est pas gardée. Elle pose les fondations de l'étape 7 (table `jobs`, service de conversion).

### File d'attente et isolement

- Une table `jobs` : type, fichier d'entrée, état, progression, erreur, fichier produit.
- **dashu ne lance aucune conversion lui-même** : il est limité à 256 Mo et n'a aucun droit. Il prépare un dossier `/srv/dashu/tmp/jobs/<id>/` (une copie du fichier, puis `job.json`). Le service `dashu-worker`, qui tourne en permanence, relève ces dossiers toutes les 2 secondes ; dashu relève ses résultats au même rythme.
- `dashu-worker` (le même binaire, sous-commande `worker`) tourne sous un utilisateur à lui, sans réseau (`PrivateNetwork=yes`), et ne voit que `tmp/` : ni la base, ni la configuration, ni les fichiers rangés. Ses limites : 1,5 Go de mémoire, une limite de temps par conversion (1 h pour une vidéo), priorité processeur basse. Il traite deux conversions à la fois au plus ; une conversion interrompue par un redémarrage reprend de zéro.
- La progression est affichée (pourcentage pour ffmpeg, étapes pour le reste) ; une erreur s'affiche en clair.
- Le résultat se télécharge, ou va dans les fichiers de l'espace en un clic.
- Tout ce qui est dans `tmp/` disparaît après 24 heures (`systemd-tmpfiles`).

## Outils : téléchargeur de liens

En dernier, une fois le reste en service.

- Un lien YouTube, Instagram ou TikTok devient un MP4 ou un MP3, avec yt-dlp et ffmpeg. Seuls ces trois sites sont acceptés, après lecture de l'adresse (pas une simple recherche de texte).
- Service séparé `dashu-download`. Il a besoin d'Internet, mais il lui est interdit de joindre le réseau local, Tailscale et la machine elle-même (`IPAddressDeny`), ce qui empêche de s'en servir pour atteindre le homelab.
- yt-dlp est mis à jour chaque semaine par une minuterie : il casse quand les plateformes changent.
- Instagram et TikTok demandent souvent un cookie de session. C'est un secret : fichier dans `/etc/dashu-telechargeur/`, lisible par ce service seul, jamais affiché.
- **Cadre** : les conditions d'utilisation des trois plateformes interdisent le téléchargement. L'outil est donc réservé au compte de Rémi (`DOWNLOADER_EMAIL`), un résultat ne se partage jamais par lien, et l'outil ne figure pas dans le portfolio.

Précisions du 25 novembre 2026, avant de l'écrire :

- **Où** : un formulaire en haut de l'onglet Outils (le lien, puis « Vidéo MP4 » ou « Son MP3 »), visible seulement pour `DOWNLOADER_EMAIL`. Pour tout autre compte, l'outil n'existe pas : pas de formulaire, et sa route répond 404. Il n'est ni dans l'API ni dans le connecteur Claude (« Hors de l'API »).
- **Adresses acceptées** : `https`, sans identifiant ni port, sur `youtube.com`, `www.`, `m.`, `music.youtube.com`, `youtu.be`, `instagram.com`, `www.instagram.com`, `tiktok.com`, `www.`, `m.`, `vm.`, `vt.tiktok.com`. L'adresse est lue (hôte exact, pas une recherche de texte), puis réécrite avant de partir vers yt-dlp. Le service la revérifie.
- **Le résultat** arrive dans Temporaire, comme un fichier déposé dans l'onglet Outils : 24 heures pour le garder dans un espace ou le télécharger. Son nom est le titre de la vidéo, nettoyé comme un nom envoyé. Il est marqué « venu d'un lien » (`files.from_link`) : il ne se partage jamais par lien, et une conversion de ce fichier hérite de la marque.
- **Limites** : la taille maximale des envois (2 Go par défaut), 30 minutes par téléchargement, un seul à la fois, trois en attente au plus. Une seule vidéo par lien (pas de liste de lecture), pas de direct. Vidéo en MP4 H.264 de 1080p au plus quand la plateforme le propose ; son en MP3.
- **yt-dlp et Deno** : YouTube impose depuis 2025 un moteur JavaScript à yt-dlp, et Deno est celui qu'il recommande. Le binaire officiel `yt-dlp_linux` (ou `_aarch64`) et Deno (version figée, comme Typst) sont téléchargés depuis GitHub et vérifiés par leur empreinte SHA-256. La minuterie de la semaine remet yt-dlp à sa dernière version, vérifiée de la même façon.
- **Pare-feu vérifié** : dans un conteneur LXC, systemd peut ignorer `IPAddressDeny` et lancer le service sans filtre. Le service vérifie donc au démarrage, et avant chaque téléchargement, qu'un envoi vers la machine elle-même, le réseau de la maison et Tailscale est bien refusé (en UDP : en TCP, le filtre jette le paquet sans refuser la connexion). Sinon il ne télécharge rien, et l'onglet Outils le dit.
- **Pare-feu dans un conteneur** (décidé avec Rémi le 25 novembre 2026, après l'essai sur le serveur) : le conteneur LXC non privilégié de Proxmox n'applique pas `IPAddressDeny`, et le service l'a détecté. Des règles nftables, dans le réseau propre au conteneur, bloquent les mêmes plages pour le seul compte `dashu-download` ; le téléchargeur ne démarre pas sans elles. Écartés : un conteneur à part, filtré par le pare-feu de Proxmox (plus isolé, mais un conteneur de plus et un dossier partagé), et un conteneur privilégié (tout dashu serait moins isolé de l'hôte).
- **DNS** : le serveur DNS de la maison est une adresse locale, que le pare-feu bloque. Le service a donc son propre `resolv.conf`, vers des résolveurs publics.
- **Activation** : par la commande `dashu-telechargeur activer <email>`, en root sur le serveur. L'installateur ne le propose pas : l'outil n'est pas destiné aux autres installations. Il faut les fichiers et les conversions (ffmpeg).

## API (en option)

`GET /api/v1/files?space=&q=&tag=` et `POST /api/v1/files` (envoi d'un fichier), avec le même jeton que le reste de l'API. Pas de suppression ni de partage par l'API.

## Exigences de performance

Les cibles de la v1 restent valables pour toutes les pages, y compris la page Fichiers et l'onglet Outils. En plus :

| Mesure | Cible |
| --- | --- |
| Mémoire de dashu pendant l'envoi d'un fichier de 1 Go | < 100 Mo |
| Temps de réponse des pages pendant une conversion vidéo | < 50 ms |
| Liste de 1 000 fichiers | < 20 ms côté serveur |

## Critères d'acceptation v2

La v2 est terminée quand toutes les cases sont cochées sur le serveur réel.

**Prérequis**

- [ ] Le second disque est monté dans le conteneur et le reste après un redémarrage de l'hôte
- [ ] La base est sauvegardée sur le second disque, les fichiers sur le premier
- [ ] Le conteneur a 4 Go de mémoire

**Fichiers**

- [ ] Envoi de plusieurs fichiers par glisser-déposer, dont un de 1 Go par `app.dashu.dev` (en morceaux, malgré la limite de Cloudflare) ; la mémoire de dashu reste sous la cible
- [ ] Un fichier trop gros est refusé avec un message clair, et rien ne reste sur le disque
- [ ] Un fichier tagué est retrouvé par la recherche, par son tag et dans la palette `Ctrl+K`
- [ ] Aperçu d'une image, d'un PDF et d'un texte ; téléchargement, renommage, déplacement, suppression
- [ ] Un fichier HTML et un SVG piégés ne s'exécutent jamais : ils sont proposés en téléchargement (test et essai réel)
- [ ] Une image collée dans l'éditeur s'affiche dans l'aperçu et apparaît dans les fichiers de l'espace
- [ ] Un lien de partage marche sans être connecté, puis est refusé une fois expiré ou révoqué
- [ ] Un second compte ne peut ni voir, ni télécharger, ni partager les fichiers du premier (tests)
- [ ] Restauration réelle : un fichier supprimé revient à partir de la sauvegarde de la nuit

**Quota de stockage**

- [ ] Un envoi qui dépasse la place restante est refusé à l'ouverture (413, message « Espace plein… »), sans rien laisser sur le disque (test)
- [ ] Un envoi pile à la limite passe ; un octet de plus est refusé (test)
- [ ] Un envoi dont le contenu dépasse la taille annoncée est coupé au plafond, fichier partiel supprimé (test)
- [ ] Deux envois en parallèle ne dépassent pas le plafond à eux deux (test)
- [ ] Un plafond baissé sous l'usage ne supprime rien et bloque les nouveaux envois, y compris garder un fichier de Temporaire (test)
- [ ] L'envoi par l'API respecte le même quota (test)
- [ ] La corbeille compte jusqu'à la suppression définitive (test)
- [ ] `dashu quota set` refuse une taille invalide ou un compte inconnu sans rien écrire ; un changement valide est journalisé ; `reset` revient au défaut (tests)
- [ ] `dashu quota list` avertit quand les plafonds dépassent la place du disque, sur le serveur réel
- [ ] La jauge de la page Fichiers passe en alerte à 90 % ; aucune page ni route ne change le plafond (tests)

**Outils**

- [ ] Chaque ligne du tableau des conversions produit un fichier valide (tests avec des fichiers d'exemple)
- [ ] Une photo HEIC d'iPhone devient un JPEG sur le serveur réel
- [ ] Trois conversions lancées ensemble : deux tournent, la troisième attend ; la progression s'affiche
- [ ] Une conversion qui dépasse le temps ou la mémoire est arrêtée avec un message clair
- [ ] Le service de conversion n'a pas accès au réseau (test)
- [ ] Les fichiers temporaires disparaissent après 24 heures

**Téléchargeur**

- [ ] Un lien YouTube donne un MP4 et un MP3
- [ ] Un autre site, ou une adresse du réseau local, est refusé (tests)
- [ ] Un autre compte n'a pas accès à l'outil, et un résultat ne peut pas être partagé
- [ ] La mise à jour hebdomadaire de yt-dlp est programmée

**Technique**

- [ ] `cargo clippy` sans avertissement, `cargo test` vert en CI
- [ ] Les cibles de performance v1 et v2 sont atteintes (Lighthouse mobile ≥ 95 sur la page Fichiers)
- [ ] README à jour : second disque, paquets, sauvegarde et restauration des fichiers, services de conversion

## Ordre de livraison

Une PR par ligne, chacune utilisable seule :

1. Prérequis : second disque, mémoire, sauvegardes de la base sur le second disque
2. Fichiers : modèle, envoi, liste, téléchargement, suppression ; puis envoi par morceaux et compression (avancés le 11 novembre)
3. Aperçu (fait le 11 novembre : vignettes dans la liste, page d'aperçu au clic), puis tags, recherche, palette, widget du dashboard
4. Partage par lien temporaire
5. Images collées et liens vers les fichiers dans les notes
6. Sauvegarde des fichiers sur le premier disque, exercice de restauration
7. Outils : zone de dépôt et conversions d'images et de PDF (la file d'attente et l'isolement existent depuis la compression)
8. Conversions de documents, d'audio et de vidéo
9. Téléchargeur de liens
10. En option : API des fichiers pour Hermès

## Décisions

Prises le 11 novembre 2026 :

- **Second disque** : il contenait une ancienne installation de Proxmox ; il est vidé entièrement. dashu n'en utilise que 50 Go.
- **LibreOffice** : non. Les DOCX et PPTX ne sont pas convertis en PDF.
- **Gros fichiers** : envoi par morceaux de 50 Mo (limite de Cloudflare), puis compression à la demande, avancée de l'étape 7.

Prises le 25 novembre 2026 :

- **Quota de stockage** : 30 Gio par compte par défaut, changé seulement par `dashu quota` sur le serveur. Les plafonds peuvent promettre plus que le disque n'en a (le volume des fichiers fait 45 Go : deux comptes à 30 Gio le dépassent déjà) ; `dashu quota` avertit sans bloquer. La place réelle comparée aux plafonds est « fichiers stockés + place libre », pas la place libre seule, qui compterait deux fois ce qui est déjà rangé.

Reste à trancher, dans la PR concernée :

- *(tranché le 15 novembre)* **Taille du volume de sauvegarde des fichiers** sur le premier disque (étape 6) : 50 Go par défaut (`mp2`, `local-lvm`), moins si `pvesm status` montre trop peu de place. Instantanés incrémentaux par liens durs (`rsync --link-dest`), 14 jours ; une restauration de la base remet aussi les fichiers absents de la même nuit.
- *(tranché le 11 novembre, revu le 18 novembre 2026)* **Aperçu des PDF** : d'abord la première page, rendue en image par le service de conversion, et le bouton Télécharger. Rémi a demandé le 18 novembre de lire tout le PDF dans dashu. Retenu : pdf.js, la bibliothèque de Mozilla, embarquée dans dashu et chargée seulement sur la page d'un PDF ; même rendu sur ordinateur et sur iPhone en mode app, texte sélectionnable, zoom. Le PDF n'est toujours jamais servi pour être affiché par le navigateur : pdf.js le reçoit comme une donnée et le dessine. Écartés : le lecteur du navigateur dans un cadre (il faut retirer l'en-tête `sandbox`, et Safari sur iPhone n'y montre souvent que la première page) et toutes les pages en images par le service de conversion (texte non sélectionnable, pas de zoom en mode app). Justification de la dépendance dans le README (« Écarts »).

# dashu — Espaces et outils (v2, suite)

Nov 13, 2026 · @Rémi Moreau

## Contexte

Proposé après un audit du fonctionnement des espaces et des outils, accepté en entier par Rémi le 13 novembre 2026 (« fait tout »). Quatre problèmes :

1. **Perso a deux rôles** : vue d'ensemble pour l'agenda, espace ordinaire pour le reste. Un devoir d'École n'apparaît pas dans Perso.
2. **L'espace actif est caché** dans la session : deux onglets sur deux espaces se contredisent, une adresse ne dit pas dans quel espace elle ouvre.
3. **On range avant, jamais après** : seules les notes se déplacent.
4. **Les outils sont introuvables** : un bouton par ligne de fichier, un suivi visible sur la seule page Fichiers.

## Périmètre

Lot 1 (rapide) :

- **B. Choisir l'espace, corriger après.** Le formulaire d'ajout de tâche et la zone d'envoi de fichiers proposent l'espace (l'espace actif par défaut ; dans Perso, tous les espaces). « Déplacer vers… » existe pour les notes (déjà), les tâches et les fichiers. Une vignette et un grand aperçu suivent leur fichier.
- **D. Toujours savoir où on est.** Le titre de l'onglet porte l'espace (« Notes · École · Hermes Dashboard ») ; l'en-tête de chaque page rappelle « École / Notes » ; un liseré de la couleur de l'espace borde le haut du contenu ; chaque bouton du sélecteur d'espaces porte le nom de son espace (infobulle et lecteur d'écran).
- **G. Les outils viennent au fichier.** Un menu « Convertir » par fichier ne propose que ce qui est possible pour son type, avec les outils déjà installés sur le serveur : photo (HEIC comprise) vers JPEG, PNG ou WebP ; vidéo vers MP4 ; vidéo ou audio vers MP3 ; PDF : compresser, extraire des pages (« 2-4 »). La compression reste dans ce menu. Un onglet **Outils** reçoit un fichier pas encore rangé : déposé, il est gardé 24 heures hors des listes ; on le convertit, puis on garde le fichier (ou son résultat) dans un espace, ou on le télécharge.
- **H. Le suivi partout.** Tant qu'une conversion tourne, la barre latérale l'indique sur toutes les pages (« 1 conversion en cours ») ; à la fin, un message dit ce qui est prêt, avec un lien.

Lot 2 (structure) :

- **A. Perso montre tout.** Dans Perso, le dashboard, les notes et les fichiers montrent le contenu de tous les espaces, chaque élément d'un autre espace marqué de son point de couleur et de son nom. Un filtre « Seulement Perso » montre le contenu propre de Perso. Ce qu'on crée depuis Perso va dans Perso, sauf choix contraire (B). L'API garde son comportement actuel (Hermès en dépend).
- **C. L'espace dans l'adresse.** Les pages d'un espace ont une adresse `/e/<slug>/…` (`/e/ecole/notes`, `/e/ecole/` pour le dashboard). Les liens et les redirections de l'application la gardent ; chaque requête htmx porte l'espace de son onglet (en-tête `X-Espace`) : deux onglets sur deux espaces ne se gênent plus. Une note ou un fichier s'ouvre toujours dans son espace. Les adresses sans `/e/` continuent de marcher (dernier espace utilisé).

Ensuite :

- **E. Archiver un espace** (sauf Perso) : il disparaît du sélecteur et des listes, son contenu reste cherchable dans la palette ; on le rouvre ou on le restaure depuis Réglages.
- **I. Règles automatiques par espace**, désactivées par défaut : « vidéos converties en MP4 à l'arrivée », « photos HEIC : un JPEG en plus ».
- **J. Remplacer l'original** après une compression ou une conversion : « Garder les deux » (par défaut) ou « Remplacer l'original », après confirmation.
- **K. Déposer partout, coller dans une note.** Un fichier lâché sur n'importe quelle page est envoyé dans l'espace de la page. Une image collée dans l'éditeur de notes est rangée dans l'espace de la note et insérée en Markdown (`![nom](/files/<id>/raw)`).
- **L. La palette connaît la page** : sur un fichier, « Compresser ce fichier », « Convertir en … », « Déplacer vers … » ; sur une note, « Déplacer vers … ».
- **F. Ouvrir sur l'espace du moment**, réglage désactivé par défaut : à l'ouverture de l'app sur le dashboard, l'espace du cours en cours ou du prochain cours du jour ; un jour sans cours, l'espace choisi dans le réglage.

## Sécurité

- Toute action vérifie que l'élément **et** l'espace de destination appartiennent à l'utilisateur connecté (déplacement, conversion, archivage, règles). Un slug d'espace inconnu dans une adresse ou un en-tête répond 404 ou est ignoré, jamais un autre compte.
- Les conversions passent toutes par le service isolé existant (sans réseau, sans accès à la base ni aux fichiers rangés). Le menu ne propose que des sorties connues ; le serveur refuse toute autre valeur. La plage de pages d'un PDF est lue comme deux nombres, jamais passée telle quelle à un outil.
- Une image collée dans une note est un fichier comme un autre : servie seulement à son propriétaire, et en ligne seulement pour JPEG, PNG, GIF et WebP (le reste se télécharge).
- Les fichiers de l'onglet Outils non gardés sont effacés au bout de 24 heures.

## Critères d'acceptation

- [ ] Une tâche et un fichier se déplacent vers un autre espace ; un fichier y garde sa vignette ; déplacer vers l'espace d'un autre compte est refusé
- [ ] Une tâche ajoutée depuis Perso peut aller dans École sans changer d'espace
- [ ] Le titre de l'onglet et l'en-tête nomment l'espace ; le sélecteur nomme chaque espace
- [ ] Le menu Convertir d'une photo HEIC propose JPEG, PNG et WebP, et pas MP3 ; une conversion demandée hors menu est refusée
- [ ] Un fichier déposé dans Outils n'apparaît dans aucune liste tant qu'il n'est pas gardé, et disparaît après 24 heures
- [ ] Une conversion en cours se voit depuis le dashboard, les notes et l'agenda ; sa fin est annoncée
- [ ] Dans Perso, une tâche, une note et un fichier d'École apparaissent, marqués « École » ; « Seulement Perso » les retire
- [ ] `/e/ecole/notes` ouvre les notes d'École même si la session était sur Entreprise ; un fragment htmx d'un onglet sur École reste sur École pendant qu'un autre onglet est sur Entreprise
- [ ] Un espace archivé n'est plus dans le sélecteur ; ses notes se trouvent encore par la palette ; il se restaure
- [ ] Une règle « vidéos en MP4 » crée la conversion à l'arrivée d'une vidéo dans cet espace, et seulement là
- [ ] « Remplacer l'original » supprime l'original et donne son nom à la copie
- [ ] Une image collée dans une note s'affiche dans l'aperçu et existe dans les fichiers de l'espace
- [ ] La palette, sur un fichier, propose ses conversions et les espaces où le déplacer
- [ ] Le réglage « espace du moment » ouvre École pendant un cours, l'espace choisi un jour sans cours
- [ ] Chaque point a ses tests d'intégration ; STATUS et README à jour

## Décisions

Prises le 13 novembre 2026, par défaut, pour avancer sans attendre :

- **A plutôt qu'une vue « Tout » à part** : c'est déjà le modèle de Perso pour l'agenda.
- **C par un préfixe `/e/<slug>`** retiré avant le routage : les routes existantes ne changent pas, et les adresses sans préfixe restent valides.
- **G avec les outils déjà installés** (ffmpeg, Ghostscript, libheif, crate `image`). Markdown et texte vers PDF, HTML ou DOCX demandent pandoc et Typst : ils restent à l'étape 8 de la v2. *(Fait le 15 novembre 2026 : pandoc en `--sandbox`, PDF par Typst, binaire officiel en version fixée.)*
- **Le WebP produit est sans perte** : l'encodeur de la crate `image` ne fait que ça.

---

# dashu — Barre de commandes

## Contexte

Demandé par Rémi le 13 novembre 2026 : remplacer la palette `Ctrl+K` par des commandes tapées après `/`, comme dans le chat de Minecraft ou un terminal, avec la complétion par `Tab`. Le but : **tout faire au clavier**, sans souris ni trackpad. Proposition acceptée le jour même, avec trois choix :

- l'ancienne recherche disparaît : effacer le `/` ferme la barre et rend la page ;
- plus de `Ctrl+K` du tout ;
- architecture mixte (voir « Décisions »).

## Fonctionnement

- **`/` ouvre la barre**, avec `/` déjà tapé, quand le curseur n'est pas dans un champ. Dans un champ (l'éditeur de notes compris, où `/` ouvre le menu des blocs), `Échap` en sort, puis `/` ouvre la barre. Sur téléphone, le bouton « Commandes » (barre du haut ou barre de raccourcis) l'ouvre aussi.
- **Effacer le `/`** ferme la barre. `Échap` aussi.
- **Grammaire** : `/commande [sous-commande] [arguments]`. Les mots sont en anglais (`/add`, `/go`), les valeurs en français comme dans l'interface (`agenda`, `ecole`), avec quelques alias (`fichiers` ou `files`). Majuscules et accents ne comptent pas. Un début de mot suffit s'il n'y a pas d'ambiguïté : `/g ag` vaut `/go agenda`. Un espace se tape par son **nom actuel** (`/space general` pour Perso renommé « Général »), jamais par l'identifiant fixé à sa création, qui reste dans les adresses (`/e/perso/`) : un nouvel espace « Perso » se tape `perso`, même si son identifiant est `perso-2`.
- **Complétion** : les suggestions s'affichent sous la saisie, la première en gris dans la ligne. `Tab` la complète ; `Tab` encore passe à la suivante, `Maj+Tab` revient. `↑` et `↓` parcourent la liste, `Entrée` exécute. Une ligne d'aide montre l'usage de la commande en cours (`/add task <titre> [@espace]`).
- **Erreurs en rouge**, dans la barre, sans la fermer : commande inconnue, ambiguë, argument manquant, commande qui ne s'emploie pas sur cette page.
- **Historique** : sur un `/` seul, `↑` rappelle les commandes précédentes (gardées dans le navigateur, 50 au plus).
- **Une suppression se confirme** par un second `Entrée`.
- **Variables** (demandées le 13 novembre 2026) : dans un texte que l'on écrit (titre d'une tâche, d'une note, d'un espace, nom d'un agenda, tag, recherche), `#date` devient la date du jour (`13/11/2026`) et `#cours` la matière du cours qui a lieu maintenant, comme l'écrit le widget « Prochains cours » (dans Perso, le cours de n'importe quel espace). `/add note #cours du #date` crée « MATHS du 13/11/2026 ». Hors d'un cours, `#cours` est refusé avec un message. Tout autre `#mot` reste tel quel (« Exercice #3 »). La complétion propose les deux, avec ce qu'elles donneront.
- Le résultat d'une commande s'annonce comme une action de l'interface : la page visée s'ouvre, avec le message habituel (« Tâche ajoutée »).

## Commandes

Celles qui agissent sur la page ouverte n'apparaissent que là où elles ont un sens.

Lot 1 :

| Commande | Effet |
| --- | --- |
| `/help` | Liste toutes les commandes et leur usage |
| `/go dashboard` · `notes` · `fichiers` · `agenda` · `outils` · `reglages` · `guide` | Ouvre la page |
| `/go agenda [jour\|semaine] [quand]` | Agenda d'un jour : `aujourdhui`, `demain`, `hier`, `lundi`… (le prochain), `14/11`, `14/11/2026` |
| `/open <titre>` | Ouvre une note ou un fichier ; les suggestions se cherchent pendant la frappe (notes : texte intégral, comme l'ancienne palette ; fichiers : nom). Sans texte : les notes récentes |
| `/search <texte>` | La liste des notes de l'espace, filtrée par ce texte |
| `/space <espace>` | Change d'espace |
| `/add task <titre> [@espace]` | Ajoute une tâche du jour |
| `/add note [titre] [@espace]` | Crée une note et ouvre l'éditeur |
| `/add file` | Ouvre le sélecteur de fichiers ; l'envoi part dans l'espace de la page |
| Sur une note ou un fichier : `/move <espace>` | Le déplace |
| Sur un fichier : `/convert <format>`, `/convert pages 2-4`, `/compress` | Les conversions que son type permet, comme son menu « Convertir » |

Lot 2 :

| Commande | Effet |
| --- | --- |
| `/add task <titre> [@espace] [+quand]` | Tâche pour un autre jour : `+demain`, `+lundi`, `+25/11` (elle apparaît ce jour-là) |
| `/done <tâche>`, `/undone <tâche>` | Coche ou décoche une tâche du jour, choisie parmi elles |
| `/delete task <tâche>` | Supprime une tâche (confirmée) |
| Sur une note ou un fichier : `/delete` | Le supprime (confirmé) |
| Sur une note : `/tag <tag>`, `/untag <tag>`, `/pin` | Tags et épinglage |
| Sur un fichier : `/download` ; sur un fichier de l'onglet Outils : `/keep <espace>` | Télécharger, garder |
| `/add space <nom>` | Crée un espace (couleur et icône proposées, modifiables dans Réglages) |
| `/add agenda <url iCal> [@espace] [nom]` | Ajoute une source d'agenda |
| `/theme sombre\|clair\|systeme`, `/size petit\|normal\|grand`, `/sidebar` | Thème, taille de l'interface, barre latérale ancrée ou détachée |
| `/refresh` | Resynchronise l'agenda |
| `/logout` | Se déconnecter |

Plus tard : `/add event`, avec des événements propres à dashu (aujourd'hui l'agenda ne fait que lire des flux iCal).

Les raccourcis à une lettre (`N`, `G` puis `D`…) restent.

## Sécurité

- Une commande n'est qu'une autre façon d'appeler les actions existantes : mêmes vérifications (l'élément **et** l'espace de destination appartiennent à l'utilisateur), même jeton CSRF (`POST /cmd`).
- Ce qui revient du serveur (titres de notes, noms de fichiers) s'affiche comme du texte, jamais comme du HTML.
- L'historique gardé dans le navigateur ne contient que des commandes tapées, jamais une réponse du serveur.

## Critères d'acceptation

Lot 1 :

- [ ] `/` ouvre la barre hors d'un champ ; effacer le `/` la ferme ; `Ctrl+K` n'ouvre plus rien et l'ancienne recherche (`/search/palette`) n'existe plus
- [ ] `Tab` complète une commande, un espace, une page ; `Tab` encore passe à la suggestion suivante
- [ ] `/go agenda demain` ouvre l'agenda du lendemain ; `/g ag` est compris ; `/d` est refusé comme ambigu quand plusieurs commandes commencent par `d`
- [ ] `/add task Réviser @ecole` crée la tâche dans École depuis Perso ; `/add note Idée` ouvre l'éditeur sur une note titrée « Idée »
- [ ] `/open` trouve une note par un mot de son texte, et un fichier par son nom ; jamais ceux d'un autre compte ; les notes d'un espace archivé restent trouvables
- [ ] Sur un fichier, la barre propose ses conversions et les espaces où le déplacer ; `/move` d'une note fonctionne ; une commande de fichier tapée ailleurs est refusée avec un message clair
- [ ] Depuis l'éditeur, une commande qui change de page n'y perd pas la dernière frappe

Lot 2 :

- [ ] `/add task Rendre le DM +demain` crée une tâche visible demain, pas aujourd'hui
- [ ] `/done` puis `/undone` cochent et décochent une tâche ; `/delete task` la supprime après confirmation
- [ ] `/tag`, `/untag`, `/pin`, `/delete` sur une note ; `/download`, `/keep`, `/delete` sur un fichier
- [ ] `/theme`, `/size`, `/sidebar`, `/refresh`, `/logout`, `/add space`, `/add agenda` font ce que font leurs boutons
- [ ] `/add note #cours du #date` pendant un cours donne « MATHS du 13/11/2026 » ; hors d'un cours, `#cours` est refusé ; « Exercice #3 » reste tel quel
- [ ] Perso renommé « Général » puis un nouvel espace « Perso » : `/space perso` mène au nouveau, `/space general` à l'ancien
- [ ] Chaque commande a son test d'intégration ; STATUS et README à jour

## Décisions

- **Architecture mixte** : la liste des commandes est définie une seule fois, en Rust, et envoyée à l'ouverture de la barre (`GET /cmd/catalogue`, avec ce qui dépend de la page : espaces, conversions, tags, tâches). La complétion se fait dans le navigateur, sans attendre le serveur : `Tab` répond tout de suite. L'exécution passe par `POST /cmd`, qui relit la ligne avec les mêmes règles et appelle le code existant : c'est testable dans `tests/`. Seules les suggestions de `/open` se cherchent sur le serveur (`GET /cmd/suggest`).
- **La navigation qui suit une commande** passe par l'en-tête `HX-Location` de htmx : même transition qu'un lien, adresse mise à jour dans l'historique, espace de l'onglet gardé.
- **`+` pour une date, `@` pour un espace** : sans ces signes, « Réviser pour demain » serait coupé en titre et date.
- **« lundi » veut dire le prochain lundi**, jamais aujourd'hui ; `14/11` est dans l'année en cours.
- **`#date` s'écrit `13/11/2026`**, et **`#cours` exige un cours en cours** (pas le prochain ni le précédent) : choix par défaut, faciles à changer (`Vars` dans `src/commands/mod.rs`).

---

# dashu — Dossiers

Nov 13, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 13 novembre 2026. Il revient sur la décision du 11 novembre (« des fichiers sans dossiers ») : les tags ne suffisent pas pour ranger les cours par matière. Les dossiers rangent les fichiers **et les notes**.

## Fonctionnement

**L'onglet Fichiers devient une arborescence.**

- À la racine : un dossier par espace (sauf les espaces archivés), plus un dossier **Temporaire**.
- Le dossier d'un espace, c'est l'espace lui-même : il apparaît à la création de l'espace, prend son nom quand on le renomme et ne se supprime pas depuis Fichiers (il part avec l'espace, depuis Réglages).
- Temporaire montre les fichiers déposés dans l'onglet Outils et pas encore gardés. Ils sont toujours effacés au bout de 24 heures.
- Dans le dossier d'un espace, des dossiers ; dans un dossier, des sous-dossiers. **Trois niveaux au plus** : Espace / Dossier / Sous-dossier.
- Un dossier se crée, se renomme et se supprime. Son nom fait 60 caractères au plus et ne se répète pas parmi ses voisins (majuscules comprises : « Math » et « math » sont le même nom).
- Supprimer un dossier vide demande une simple confirmation. Si le dossier contient quelque chose, une fenêtre d'avertissement dit ce qu'il contient et propose deux choix :
  - **Transférer dans le dossier parent** : ses notes et ses fichiers vont dans le dossier du dessus ; ses sous-dossiers montent d'un niveau (renommés « Exercices (2) » si le nom est déjà pris) ;
  - **Tout supprimer** : ses notes, ses fichiers et ses sous-dossiers sont effacés.
- Un fichier envoyé depuis un dossier y arrive. Une copie compressée ou convertie arrive dans le dossier de l'original ; « Remplacer l'original » lui donne aussi son dossier.
- À l'ouverture, l'onglet montre le dossier de l'espace actif ; depuis Perso, la racine (tous les espaces). Un fil d'Ariane, le chemin cliquable en haut de la page (Fichiers › École › Math), permet de remonter. Le filtre « Seulement Perso » des fichiers disparaît : c'est le dossier Perso.
- Adresses : `/files?dossier=racine`, `espace`, `temporaire` ou le numéro d'un dossier. Un dossier s'ouvre toujours dans son espace (`/e/ecole/files?dossier=12`).

**Les notes se rangent dans les dossiers.**

- Dans l'éditeur, à côté des tags, un sélecteur « Dossier » propose la racine de l'espace de la note et les dossiers du premier niveau de cet espace (pas les sous-dossiers). Sans choix, une note est à la racine de son espace.
- En bas du sélecteur, **Voir plus…** ouvre une fenêtre avec l'arborescence complète : tous les espaces, tous les dossiers et sous-dossiers. Choisir un autre espace y déplace la note.
- Une note apparaît dans son dossier de l'onglet Fichiers, à côté des fichiers.
- L'onglet Notes regroupe les notes par dossier du premier niveau : d'abord celles sans dossier, puis une section par dossier. Une note d'un sous-dossier compte dans la section de son dossier, avec le nom du sous-dossier. Recherche, tags et « Seulement Perso » marchent comme avant.

**Un seul sélecteur pour ranger.** La même fenêtre sert à déplacer un fichier (liste et aperçu), à garder un fichier de Temporaire ou de l'onglet Outils, à déplacer une note (bouton de l'en-tête de l'éditeur) et à déplacer un dossier. La barre de commandes garde `/move <espace>` et `/keep <espace>`, qui rangent à la racine de l'espace.

**Ajoutés le même jour, à la demande de Rémi :**

- **Déplacer un dossier entier**, avec son contenu et ses sous-dossiers, dans un autre dossier ou un autre espace. Les trois niveaux restent la règle : un dossier qui a des sous-dossiers ne va que directement dans un espace ; un dossier sans sous-dossier peut aussi devenir le sous-dossier d'un dossier du premier niveau. Un dossier ne va pas dans lui-même, ni là où un voisin porte déjà son nom (message clair, rien ne bouge). Changer d'espace emmène tout son contenu, conversions en cours comprises.
- **Glisser-déposer** dans l'onglet Fichiers : un fichier, une note ou un dossier lâché sur un dossier (une ligne de la liste, le dossier d'un espace à la racine, ou un pas du fil d'Ariane) y est rangé. Des fichiers venus de l'ordinateur lâchés sur un dossier y sont envoyés. C'est un raccourci de la souris : la fenêtre « Ranger » reste le chemin au clavier et au doigt.
- **Créer une note depuis Fichiers** : un bouton « Nouvelle note » dans le dossier d'un espace et dans un dossier ; la note naît dans le dossier ouvert et l'éditeur s'ouvre.

**Ce qui ne change pas** : les tags, la recherche, la barre de commandes, l'API. Une note créée par Hermès va à la racine ; une note déplacée par l'API va à la racine de son nouvel espace.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `folders` | id, user\_id, space\_id, parent\_id, name, created\_at | `parent_id` vide : dossier du premier niveau |
| `notes`, `files`, `uploads` | + `folder_id` | Vide : racine de l'espace |

Les dossiers n'existent que dans la base. Sur le disque, un fichier garde son nom aléatoire, quel que soit son dossier.

## Sécurité

- Toute action vérifie que le dossier appartient au compte connecté et que le dossier de destination est dans l'espace de destination. Le dossier d'un autre compte répond 404.
- Un élément et son dossier sont toujours dans le même espace : un déplacement vers un autre espace sans dossier choisi range à la racine.
- Un niveau de trop (un dossier dans un sous-dossier) est refusé par le serveur, pas seulement caché.
- Les noms de dossiers sont des données : échappés à l'affichage, jamais dans un chemin du disque.
- « Tout supprimer » efface aussi les fichiers du disque, comme la suppression d'un fichier.

## Critères d'acceptation

- [ ] La racine de Fichiers montre un dossier par espace (pas les archivés) et Temporaire ; renommer un espace renomme son dossier
- [ ] Un dossier se crée dans un espace et dans un dossier ; un troisième niveau est refusé ; deux voisins ne portent pas le même nom
- [ ] Un fichier envoyé depuis un dossier y arrive ; sa copie convertie aussi
- [ ] Temporaire montre un fichier déposé dans Outils ; le ranger le garde dans le dossier choisi
- [ ] Le sélecteur de l'éditeur propose les dossiers du premier niveau de l'espace de la note, ni ceux d'un autre espace ni les sous-dossiers ; une note sans choix est à la racine
- [ ] « Voir plus » propose tous les espaces et tous les dossiers ; choisir un dossier d'un autre espace y déplace la note
- [ ] Une note rangée apparaît dans son dossier de Fichiers ; l'onglet Notes la montre dans la section de son dossier
- [ ] Supprimer un dossier non vide : « Transférer » garde tout dans le parent, « Tout supprimer » efface notes, fichiers (disque compris) et sous-dossiers ; sans choix, le serveur refuse
- [ ] Un second compte ne voit, ne remplit, ne renomme ni ne supprime les dossiers du premier ; ranger dans un dossier d'un autre espace que celui demandé est refusé
- [ ] Supprimer un espace en gardant son contenu emmène ses dossiers
- [ ] Un dossier se déplace dans un autre espace avec ses notes, ses fichiers et ses sous-dossiers ; un dossier qui a des sous-dossiers ne va pas dans un dossier ; un dossier ne va ni dans lui-même ni à côté d'un homonyme
- [ ] Un fichier, une note ou un dossier lâché sur un dossier y est rangé ; des fichiers de l'ordinateur lâchés sur un dossier y arrivent
- [ ] « Nouvelle note » dans un dossier de Fichiers crée la note dans ce dossier ; un dossier d'un autre espace est refusé
- [ ] Chaque point a ses tests d'intégration ; STATUS et README à jour

## Décisions

Prises avec Rémi le 13 novembre 2026 :

- **Trois niveaux** (Espace / Dossier / Sous-dossier). Le sélecteur de l'éditeur ne montre que le premier ; « Voir plus » montre tout.
- **Dossier non vide** : après un avertissement, transférer son contenu dans le parent ou tout supprimer.
- **Temporaire** reste effacé au bout de 24 heures.
- **Ouverture** sur le dossier de l'espace actif ; sur la racine depuis Perso.
- **Onglet Notes** regroupé par dossier du premier niveau.

Par défaut, pour avancer :

- **Dossiers dans la base seulement** : rien ne bouge sur le disque, et aucun nom choisi par l'utilisateur n'entre dans un chemin.
- **Un espace supprimé en gardant son contenu** emmène ses dossiers dans l'espace qui le reçoit ; un nom déjà pris devient « Math (2) ».
- **Glisser-déposer, déplacer un dossier, créer une note depuis Fichiers** : d'abord laissés hors du lot, faits le même jour à la demande de Rémi (voir « Fonctionnement »). Un déplacement refusé (niveau de trop, homonyme) ne renomme rien : il le dit, et l'utilisateur choisit.

# dashu — Espaces partagés

Nov 16, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 16 novembre 2026. dashu accepte plusieurs comptes depuis `dashu-compte` (une adresse ajoutée à `ALLOWED_EMAILS`), mais chacun reste chez soi : un espace appartient à un seul compte. Un **espace partagé** réunit plusieurs comptes autour des mêmes notes, fichiers, dossiers et tâches, par exemple pour un projet de groupe du BTS.

## Fonctionnement

**Partager un espace.** N'importe quel espace se partage, sauf Perso, qui reste toujours privé. Il n'y a pas de type « partagé » à la création : on crée un espace comme avant, puis on y invite quelqu'un.

- Dans Réglages › Espaces, chaque espace (sauf Perso) a un bouton **Membres**. La page montre le propriétaire, les membres et les invitations en attente. Elle permet aussi d'inviter un compte par son adresse e-mail.
- L'adresse doit être celle d'un compte qui existe, c'est-à-dire qui s'est déjà connecté une fois. Sinon, un message le dit et rien n'est créé. On ne s'invite pas soi-même, et on n'invite pas deux fois la même personne ni quelqu'un qui est déjà membre. Un espace archivé ne reçoit pas d'invitation.
- Le propriétaire peut annuler une invitation en attente et retirer un membre.

**Recevoir une invitation.** Les invitations en attente s'affichent en haut de Réglages, avec le nom de l'espace et celui de la personne qui invite.

- **Accepter** : l'espace apparaît à la fin de l'île du haut et dans Réglages, avec la mention « Partagé par Alice ».
- **Refuser** : l'invitation disparaît, et le propriétaire peut réinviter plus tard.
- Accepter est refusé, avec un message clair, quand on a déjà 8 espaces : les espaces rejoints comptent dans la limite de l'île.

**Dans un espace partagé, tout le contenu est commun.**

- Tous les membres, propriétaire compris, voient, créent, modifient, rangent et suppriment les notes, les fichiers, les dossiers et les tâches de l'espace.
- Les tags posés par l'un sont vus par tous. Dans la liste et dans le filtre, deux tags de même nom n'en font qu'un.
- Les liens de partage des fichiers de l'espace sont visibles et révocables par tous les membres.
- Ranger un élément vers un autre espace marche entre tous les espaces auxquels on a accès. Sortir une note d'un espace partagé la retire donc aux autres membres.
- Le Perso de chaque membre réunit aussi le contenu des espaces partagés qu'il a rejoints, comme celui de ses autres espaces (sauf avec « Seulement Perso »).
- La barre de commandes et l'API (Hermès) voient les espaces partagés comme le site.

**Ce qui reste à chacun**, même dans un espace partagé :

- les **agendas** : un agenda iCal rattaché à un espace partagé n'est vu que par celui qui l'a ajouté, parce que son adresse est secrète ;
- le dossier **Temporaire** et les envois en cours. La copie gardée, compressée ou convertie arrive dans l'espace pour tous.

**Ce que seul le propriétaire fait** : renommer l'espace, changer sa couleur, son icône et ses règles automatiques, l'archiver, le supprimer, inviter et retirer des membres. Le serveur le vérifie, ce n'est pas seulement caché.

**Quitter.** Un membre quitte un espace depuis Réglages, avec une confirmation. Qu'il parte ou qu'il soit retiré :

- il n'a plus accès à l'espace dès la requête suivante ;
- ce qu'il y a créé reste dans l'espace ;
- ses agendas rattachés à l'espace reviennent dans son Perso ;
- les liens de partage qu'il a créés vers les fichiers de l'espace sont révoqués ;
- si l'espace était son espace « moment », ce réglage revient au défaut.

**Archiver ou supprimer un espace partagé** (propriétaire seulement) : il est archivé ou supprimé pour tous. « Transférer le contenu » ne propose que les espaces du propriétaire. Les agendas des membres reviennent dans leur Perso.

**Adresses.** Chaque membre a son propre identifiant d'adresse pour l'espace : celui du propriétaire (`/e/projet-bts/`), ou suffixé (`/e/projet-bts-2/`) s'il est déjà pris chez lui. Il n'est jamais `perso`.

**Notes modifiées à plusieurs.** L'éditeur enregistre tout seul toutes les deux secondes, et chaque enregistrement remplace tout le texte. Sans garde-fou, deux personnes sur la même note s'effaceraient l'une l'autre. Chaque note porte donc un **numéro de version**, que l'éditeur renvoie à chaque enregistrement.

- Un enregistrement fait sur une version dépassée est refusé, et rien n'est écrasé.
- L'indicateur d'état le dit : « Modifiée ailleurs entre-temps : copie tes changements, puis recharge la note ».
- La même protection vaut pour deux onglets du même compte, et pour une note modifiée par Hermès pendant qu'on l'édite.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `space_members` | space\_id, user\_id, slug, position, joined\_at | Un membre qui n'est pas le propriétaire. `slug` et `position` sont ceux de l'espace chez ce membre (adresse, place dans l'île). |
| `space_invitations` | id, space\_id, user\_id, invited\_by, created\_at | `user_id` : l'invité. Une seule invitation par espace et par compte. |
| vue `space_access` | user\_id, space\_id | Le propriétaire et les membres de chaque espace. Toutes les requêtes de contenu filtrent sur elle. |
| `notes` | + `revision` | Augmente de 1 à chaque enregistrement |

La colonne `user_id` des notes, fichiers, dossiers et tâches ne décide plus de l'accès : elle garde l'**auteur**. Les tags gardent le compte qui les a créés, mais s'affichent et se filtrent par nom. Les agendas, Temporaire, les envois en cours et les conversions restent filtrés par compte.

## Sécurité

- Toute requête de contenu passe de « c'est à moi » (`user_id = moi`) à « j'ai accès à son espace » (`space_id` dans `space_access`). Un élément d'un espace où l'on n'est pas répond 404, comme avant.
- La destination d'un rangement, d'un déplacement ou d'une conversion doit aussi être un espace accessible.
- Les actions du propriétaire sont vérifiées par le serveur. Accepter ou refuser une invitation n'est permis qu'à l'invité.
- Retirer un membre lui coupe l'accès à la requête suivante, puisque la liste des espaces est relue à chaque requête. Ses liens de partage vers l'espace sont révoqués.
- Les noms et adresses affichés sur la page Membres sont échappés, et l'adresse saisie n'entre dans aucune requête autrement que par un paramètre.
- Retirer un compte de `ALLOWED_EMAILS` lui coupe tout accès, espaces partagés compris. Ce qu'il a créé reste dans les espaces.

## Critères d'acceptation

- [ ] Le propriétaire invite un compte existant ; une adresse sans compte, soi-même, un membre, un doublon, Perso ou un espace archivé sont refusés
- [ ] L'invité voit l'invitation dans Réglages ; accepter ajoute l'espace à son île et à ses réglages ; refuser la fait disparaître ; une invitation d'un autre compte ne s'accepte pas
- [ ] Accepter au-delà de 8 espaces est refusé
- [ ] Un membre voit, crée, modifie, range et supprime les notes, fichiers, dossiers et tâches de l'espace ; un compte qui n'en est pas membre reçoit 404
- [ ] Les tags d'un espace partagé sont communs ; deux tags de même nom ne s'affichent qu'une fois
- [ ] Les agendas et Temporaire d'un membre restent invisibles aux autres
- [ ] Un membre ne renomme, n'archive, ne supprime l'espace ni n'invite (refus du serveur)
- [ ] Quitter ou être retiré coupe l'accès, laisse le contenu, ramène ses agendas dans son Perso et révoque ses liens de partage
- [ ] Deux membres dont l'un a déjà un espace du même identifiant ouvrent chacun le partagé à sa propre adresse
- [ ] Le Perso d'un membre réunit le contenu de l'espace partagé ; l'API et `/open` le trouvent aussi
- [ ] Un enregistrement de note sur une version dépassée est refusé sans rien écraser
- [ ] Chaque point a ses tests d'intégration ; STATUS et README à jour

## Décisions

Prises avec Rémi le 16 novembre 2026 :

- **Contenu commun** : notes, fichiers, dossiers et tâches. Les agendas restent à chacun.
- **On partage un espace existant** (bouton « Membres »), sauf Perso.
- **Membres** : tout sur le contenu. Le propriétaire seul gère l'espace et ses membres.
- **Invitation à accepter**, pas d'ajout direct.

Par défaut, pour avancer :

- **Seulement des comptes existants** : inviter une adresse autorisée qui ne s'est jamais connectée ne crée pas son compte d'avance.
- **Invitations visibles dans Réglages** seulement, sans notification ni e-mail.
- **Garde-fou de version** sur les notes plutôt que l'édition à plusieurs en direct, qui demanderait un serveur temps réel (WebSocket) et un éditeur collaboratif.
- **Pas de rôle « lecture seule »**, et pas d'affichage de l'auteur dans les listes : à voir à l'usage.
- **Le tableau de bord d'un espace partagé** (disposition des widgets) est commun, et tout membre peut le modifier.

# dashu — Notes en temps réel

Nov 16, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 16 novembre 2026, juste après les espaces partagés : voir les modifications des autres dans une note partagée pendant qu'ils écrivent, avec le curseur de chacun, comme dans un Google Doc. Ça remplace, quand la connexion le permet, le garde-fou de version (« Modifiée ailleurs entre-temps »), qui protégeait sans permettre d'écrire à plusieurs.

## Fonctionnement

**Pour l'utilisateur.**

- Une note ouverte se connecte au serveur en direct. Ce que tape quelqu'un d'autre apparaît dans la seconde, au bon endroit, sans recharger la page et sans perdre ce qu'on est en train d'écrire.
- Le curseur de chaque autre personne s'affiche dans le texte : un trait de sa couleur (celle de son avatar), avec son prénom au-dessus. Sa sélection est surlignée de la même couleur.
- En haut de la note, à côté de l'indicateur d'état, les avatars des personnes présentes sur la note.
- Le titre suit aussi en direct.
- L'enregistrement reste automatique, et l'indicateur dit toujours la vérité : « Enregistré à 14:05 » n'apparaît qu'une fois le texte écrit dans la base.
- Annuler (Ctrl+Z) n'annule que ses propres modifications, pas celles des autres.
- Ça marche dans toutes les notes, pas seulement les partagées : la même note ouverte sur l'ordinateur et sur l'iPhone reste à jour des deux côtés.

**Coupures.**

- Si la connexion tombe, on continue d'écrire ; l'indicateur affiche « Reconnexion… ». Au retour, les modifications faites entre-temps partent et se fusionnent avec celles des autres.
- Le serveur garde la session d'une note une minute après le départ de la dernière personne. Une coupure plus longue, ou un redémarrage de dashu, oblige à repartir de la version enregistrée. Ce qui n'était pas encore parti est alors enregistré comme avant, avec le garde-fou de version : ça passe si personne d'autre n'a modifié la note, sinon le message habituel le dit.
- Sans connexion en direct (navigateur trop ancien, réseau qui bloque), l'éditeur marche comme avant : enregistrement toutes les deux secondes et garde-fou de version.
- Une note modifiée hors de l'éditeur pendant qu'on l'édite à plusieurs (par Hermès ou l'API) : rien n'est écrasé. Les personnes présentes voient « Modifiée ailleurs entre-temps : copie tes changements, puis recharge la note ».

**Comment ça marche** (ProseMirror, module officiel `prosemirror-collab`) :

- Le serveur sert d'**arbitre** : il numérote les modifications de chaque note (les « étapes » de ProseMirror) et les renvoie, dans le même ordre, à tous ceux qui ont la note ouverte. Une étape envoyée sur une version dépassée est ignorée. Son auteur reçoit d'abord les étapes qui lui manquaient, recale les siennes par-dessus, puis les renvoie.
- Le serveur ne lit pas le contenu des étapes. Le Markdown reste le format stocké : celui qui vient d'écrire envoie, une seconde après, le Markdown de la note à sa version confirmée, et le serveur l'enregistre si cette version est toujours la dernière.
- La connexion est une **WebSocket**, un canal ouvert en permanence entre la page et le serveur, sur `/notes/{id}/live`. Le tunnel Cloudflare et Tailscale la laissent passer. Le serveur envoie un signe de vie toutes les 30 secondes, pour que la connexion ne soit pas coupée faute d'activité.

## Sécurité

- La connexion exige une session ouverte et l'accès à la note (son espace est dans `space_access`). Sinon : refus, sans ouvrir de canal.
- L'**origine** de la connexion doit être dashu lui-même (en-tête `Origin` comparé à l'hôte de la requête). Une page d'un autre site, même sous `dashu.dev`, ne peut pas ouvrir de canal avec le cookie de l'utilisateur.
- L'accès est revérifié à chaque envoi (étapes, titre, enregistrement) et toutes les 30 secondes. Un membre retiré de l'espace, une note déplacée hors de ses espaces ou une adresse retirée de `ALLOWED_EMAILS` ferment le canal.
- Le nom et la couleur affichés à côté d'un curseur viennent du serveur (le profil du compte), jamais de ce qu'envoie la page.
- Limites : 1 Mo par message, 1000 étapes par envoi, 20 personnes par note. Au-delà de 100 000 étapes dans une même session, tout le monde repart de la version enregistrée.
- Les étapes reçues ne sont jamais rendues comme du HTML brut : elles passent par le schéma de l'éditeur. Un lien venu d'un autre est nettoyé comme un lien collé (`safeHref`). L'enregistrement passe par le même Markdown que d'habitude, et l'affichage par ammonia.

## Critères d'acceptation

- [ ] Deux personnes sur la même note : ce que l'une tape apparaît chez l'autre en moins d'une seconde, y compris en tapant au même moment au même endroit, sans rien perdre
- [ ] Le curseur et la sélection de l'autre s'affichent avec son prénom et sa couleur ; les avatars des présents sont en haut de la note
- [ ] Le titre suit en direct
- [ ] Le Markdown enregistré est celui de la dernière version ; l'API et la recherche le voient
- [ ] Une coupure courte (moins d'une minute) ne perd rien ; une coupure longue retombe sur le garde-fou de version
- [ ] Sans WebSocket, l'éditeur marche comme avant
- [ ] Une connexion sans session, d'une autre origine ou sur une note inaccessible est refusée ; un membre retiré perd le canal
- [ ] Une note modifiée par l'API pendant l'édition n'est pas écrasée
- [ ] Ctrl+Z n'annule que ses propres modifications
- [ ] Chaque point a ses tests d'intégration (serveur) et a été essayé dans le navigateur à deux ; STATUS et README à jour

## Décisions

Prise avec Rémi le 16 novembre 2026 :

- **Collab ProseMirror et WebSocket**, plutôt que Yjs (plus lourd, avec deux vérités à garder d'accord) ou une simple présence sans frappe en direct.

Par défaut, pour avancer :

- **Toutes les notes** passent en direct, pas seulement celles des espaces partagés : un seul chemin à entretenir, et la même note sur deux appareils en profite.
- **Pas d'édition hors ligne** fusionnée plus tard : au-delà d'une minute de coupure, on retombe sur le garde-fou de version.
- **Le serveur ne garde pas les étapes sur le disque** : seulement en mémoire, le temps de la session. Le Markdown enregistré reste la seule vérité.
- **Les tags et le dossier** ne suivent pas en direct (ils se voient au rechargement).

# dashu — Événements et Google Agenda

Nov 17, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 17 novembre 2026 : ajouter des événements depuis l'Agenda de dashu, et qu'ils arrivent aussi dans Google Agenda quand le compte Google est connecté. Jusqu'ici, l'agenda ne faisait que lire des flux iCal (Pronote), et la connexion Google ne demandait que l'identité. Le projet Google de dashu est passé en production le même jour, ce qui est le préalable : en mode Test, un accès à l'agenda expire au bout de 7 jours.

## Fonctionnement

**Les événements de dashu.**

- Un bouton **Nouvel événement** dans l'Agenda ouvre un formulaire : titre, jour, heure de début et de fin (ou « journée entière »), lieu, espace. L'espace proposé est l'espace actif.
- L'événement apparaît dans l'Agenda, dans les widgets Calendrier et Prochains cours, dans la barre de commandes et dans l'API, comme un cours de Pronote. Perso le réunit avec ceux des autres espaces.
- Un clic sur un événement de dashu ouvre le même formulaire, pour le modifier ou le supprimer. Les événements des flux iCal restent en lecture seule.
- Comme les agendas, un événement est **à son auteur**, même dans un espace partagé : les autres membres ne le voient pas. Un membre qui quitte un espace partagé retrouve ses événements dans son Perso.

**Google Agenda.**

- Dans Réglages, une carte **Google Agenda** : « Connecter Google Agenda » envoie chez Google, qui demande la permission de gérer les événements de l'agenda. Au retour, la carte dit à quelle adresse Google l'agenda est relié. « Déconnecter » retire la permission chez Google et oublie le jeton.
- Une fois connecté, le formulaire d'un événement propose **« Aussi dans Google Agenda »**, coché par défaut. L'événement est alors créé dans l'agenda principal du compte Google, puis modifié ou supprimé là-bas quand on le modifie ou le supprime dans dashu.
- L'envoi part tout de suite après l'enregistrement. Si Google ne répond pas, l'événement est marqué « pas encore dans Google Agenda » et dashu réessaie toutes les cinq minutes. Le formulaire montre l'état : dans Google Agenda, en attente, ou l'erreur.
- **Un seul sens** : ce qu'on modifie directement dans Google ne revient pas dans dashu. Si l'agenda Google est aussi importé dans dashu par son lien iCal, un événement envoyé par dashu n'y apparaît pas en double.
- Sans connexion Google, ou si Google n'est pas configuré (`GOOGLE_CLIENT_ID` vide), tout le reste marche : les événements restent dans dashu.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `events` | id, user\_id, space\_id, title, location, starts\_at, ends\_at, all\_day, google, google\_event\_id, google\_dirty, google\_error, created\_at, updated\_at | `google` : à envoyer à Google ; `google_dirty` : changement pas encore envoyé |
| `google_accounts` | user\_id, email, refresh\_token, connected\_at, last\_error | `refresh_token` chiffré |
| `google_deletions` | user\_id, google\_event\_id | Suppressions à faire chez Google, gardées si Google ne répond pas |
| vue `agenda_items` | user\_id, space\_id, title, location, starts\_at, ends\_at, all\_day, event\_id | Les cours des flux iCal et les événements de dashu, sans les doublons |

## Sécurité

- **Permission minimale** : `calendar.events` (créer, modifier, supprimer des événements), plus l'adresse e-mail pour l'afficher. Pas de lecture des autres agendas.
- **Le jeton Google est chiffré** en base (ChaCha20-Poly1305), avec une clé dérivée de `SESSION_SECRET`. Une copie de la base ou d'une sauvegarde ne suffit donc pas à écrire dans l'agenda. Changer `SESSION_SECRET` oblige à reconnecter Google Agenda.
- La connexion à Google suit le même chemin que la connexion par Google : `state` vérifié, PKCE, adresse de retour déjà déclarée (`/auth/google/callback`). Seul un compte connecté à dashu peut relier un agenda, et seulement le sien.
- Un événement ne s'ouvre, ne se modifie et ne se supprime que par son auteur (404 sinon). L'espace choisi doit être accessible.
- Le titre et le lieu sont échappés à l'affichage, et envoyés à Google comme données JSON.

## Critères d'acceptation

- [ ] Créer, modifier et supprimer un événement depuis l'Agenda, en heures ou sur la journée entière ; titre vide, fin avant le début, date invalide sont refusés
- [ ] L'événement apparaît dans l'Agenda, les widgets, Perso et l'API ; un autre compte, même membre de l'espace, ne le voit pas et reçoit 404
- [ ] Connecter Google Agenda : retour de Google vérifié, jeton chiffré en base, adresse affichée ; déconnecter retire la permission
- [ ] Un événement coché arrive dans Google Agenda à sa création, suit ses modifications et sa suppression
- [ ] Google indisponible : rien n'est perdu, l'événement est marqué en attente et part au passage suivant
- [ ] Un événement envoyé par dashu n'apparaît pas en double quand l'agenda Google est aussi importé en iCal
- [ ] Chaque point a ses tests d'intégration (Google simulé par un faux serveur local) ; essai réel sur le serveur avec le vrai Google ; STATUS et README à jour

## Décisions

Prises avec Rémi le 17 novembre 2026 :

- **Écriture directe dans Google Agenda (option B)**, plutôt qu'un flux iCal abonné dans Google (lent et en lecture seule).
- **Projet Google publié** (en production) : pas d'expiration toutes les semaines. Tant que Google n'a pas vérifié l'application, sa page de consentement affiche « application non vérifiée » (passer par « Paramètres avancés »).

Par défaut, pour avancer :

- **Agenda principal** du compte Google, pas un agenda « dashu » à part.
- **Un seul sens**, de dashu vers Google.
- **Événements à leur auteur**, comme les agendas iCal, même dans un espace partagé.
- **Clé de chiffrement dérivée de `SESSION_SECRET`** : rien de nouveau à configurer sur le serveur.
- **Pas d'action API** pour créer un événement : Hermès a déjà son propre accès à Google Agenda.

# dashu — Connecteur Claude (MCP)

Nov 20, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 20 novembre 2026 : piloter dashu depuis Claude (claude.ai, l'app iPhone, Claude Desktop, Claude Code). Pas seulement Rémi : chaque compte de dashu, celui d'un collègue par exemple, doit pouvoir brancher son propre Claude sur son propre compte.

**MCP** (Model Context Protocol) est le protocole par lequel Claude découvre les outils d'une application et les appelle. Un **connecteur** est un serveur MCP ajouté à Claude.

Aujourd'hui, seule l'API `/api/v1` ouvre dashu à un programme. Elle agit au nom d'un seul compte (`API_USER_EMAIL`, celui de Rémi, pour Hermès), avec un jeton fixe. Un collègue ne peut donc pas s'en servir pour son compte. Et claude.ai n'accepte pas de jeton fixe : pour un connecteur perso, il demande **OAuth**, le protocole standard où l'on autorise une application depuis la page de connexion du service, sans jamais lui donner son mot de passe.

Le travail se fait en trois étapes, une PR chacune. Chaque étape sert dès qu'elle est fusionnée. En parallèle, l'API s'étend à tout ce que fait le site (section suivante, « API complète et corbeille ») : le connecteur et Hermès en profitent à chaque lot.

## Fonctionnement

### Étape 1 : un jeton par compte

- Réglages a une nouvelle carte **Connexions**. Elle liste les accès du compte : les jetons créés à la main et les autorisations données à Claude (étape 3), avec leur nom, leur date de création, leur dernière utilisation et leur date de fin.
- **Nouveau jeton** : on lui donne un nom (« Claude Code du Mac ») et une durée (30 jours, 90 jours ou 1 an ; 90 jours par défaut). Le jeton s'affiche **une seule fois**, avec un bouton Copier et la commande prête à coller pour Claude Code. dashu n'en garde qu'une empreinte.
- **Révoquer** coupe un accès à la requête suivante. **Tout révoquer** coupe d'un coup tous les accès du compte.
- 20 jetons créés à la main au plus par compte. Un jeton expiré est refusé ; il reste affiché comme « expiré » jusqu'à ce qu'on le supprime.
- Un jeton agit au nom de son compte, avec exactement ses droits sur le site : ses espaces, et les espaces partagés qu'il a rejoints.
- L'API `/api/v1` accepte ces jetons. `API_TOKEN` et `API_USER_EMAIL` continuent de marcher pour Hermès, sans changement.

### Étape 2 : la route `/mcp`

dashu parle MCP lui-même, à l'adresse `https://app.dashu.dev/mcp` (ou par l'adresse Tailscale). Le transport est celui que MCP appelle « Streamable HTTP » : Claude envoie un message JSON-RPC (un appel de fonction écrit en JSON) par `POST /mcp`, et dashu répond en JSON. dashu n'envoie jamais de message de lui-même, donc pas de flux ouvert : `GET /mcp` répond 405. Pas de session MCP non plus : chaque requête porte son jeton.

Dès cette étape, Claude Code se branche avec un jeton personnel :

```
claude mcp add --transport http dashu https://app.dashu.dev/mcp --header "Authorization: Bearer dashu_…"
```

Dans Claude, le connecteur porte le nom affiché de l'application, « Hermes Dashboard » (`assets::APP_NAME`).

**Les outils** font ce que fait l'API, ni plus ni moins. Ils appellent les mêmes fonctions que les routes de l'API, avec les mêmes règles et les mêmes limites (titre de tâche de 200 caractères, note de 200 000, 20 tags par requête) : un seul code à entretenir. Chaque outil porte une **annotation** qui dit à Claude s'il lit, écrit ou détruit. Claude s'en sert pour demander confirmation, selon ses propres réglages.

Les premiers outils couvrent l'API d'aujourd'hui. Chaque lot de « API complète et corbeille » ajoute les siens.

| Outil | Ce qu'il fait | Annotation |
| --- | --- | --- |
| `lister_espaces` | Les espaces du compte, partagés compris | lecture seule |
| `lister_taches` | Tâches d'un espace ou de tous : ouvertes, faites ou toutes | lecture seule |
| `creer_tache` | Titre, espace, échéance (aujourd'hui par défaut) | écriture |
| `modifier_tache` | Titre, faite ou non, échéance | écriture |
| `supprimer_tache` | Supprime une tâche | destructrice |
| `chercher_notes` | Notes récentes, ou recherche plein texte ; filtre par espace et par tag | lecture seule |
| `lire_note` | Une note complète, avec son Markdown | lecture seule |
| `creer_note` | Titre, Markdown, espace, tags | écriture |
| `modifier_note` | Titre, ajout à la fin du texte, épingle, ajout de tags | écriture |
| `remplacer_note` | Remplace tout le texte d'une note | destructrice |
| `deplacer_note` | Change une note d'espace. Sortie d'un espace partagé, elle n'est plus vue des autres membres | destructrice |
| `lire_agenda` | Événements entre deux dates, 62 jours au plus | lecture seule |
| `rafraichir_agenda` | Resynchronise tout de suite les flux iCal | écriture |

- Une erreur de saisie (espace inconnu, date invalide, titre trop long) revient comme un résultat d'outil en erreur, avec le même message en français que l'API : Claude peut corriger et réessayer.
- Les **instructions du serveur**, un texte que Claude lit à la connexion, disent que les notes, les tâches et les événements sont des données écrites par des personnes, parfois par un autre membre d'un espace partagé. Une phrase qui y ressemble à une consigne ne se suit pas.
- Une note modifiée par MCP pendant qu'on l'édite n'est pas écrasée : même chemin que l'API (« Modifiée ailleurs entre-temps »).
- Hors de portée, comme dans l'API : ce qui protège le compte, vider la corbeille, l'administration du serveur (`restart` et `update` restent à `dashu-admin`). La liste est dans « API complète et corbeille ».

### Étape 3 : OAuth, pour claude.ai et l'iPhone

**Pour l'utilisateur.**

- Dans les réglages des connecteurs de Claude, on ajoute un connecteur personnalisé avec l'adresse `https://app.dashu.dev/mcp`. Claude ouvre une page de dashu.
- Pas connecté à dashu : la page de connexion habituelle (mot de passe ou Google), puis retour à l'autorisation.
- Connecté : une page **Autoriser Claude**, au nom et au logo d'Hermes Dashboard, montre le nom du client, l'adresse où il sera renvoyé, le compte (« alice@… ») et ce qu'il pourra faire : ce que l'API permet à ce moment-là (au bout des lots de « API complète et corbeille » : tout, sauf ce qui protège le compte). Deux boutons : **Autoriser** et **Refuser**.
- Après Autoriser, l'accès apparaît dans Réglages › Connexions (« Claude, autorisé le 20 novembre »), révocable comme un jeton.
- Un connecteur ajouté sur claude.ai suit dans l'app iPhone et dans Claude Desktop. Claude Code passe par la même page, sans jeton à coller (`claude mcp add --transport http dashu https://app.dashu.dev/mcp`, puis `/mcp` pour se connecter).

**Comment ça marche** (OAuth 2.1, tel que MCP le demande) :

1. Claude appelle `/mcp` sans jeton. dashu répond 401, avec un en-tête `WWW-Authenticate` qui pointe vers `/.well-known/oauth-protected-resource`.
2. Ce document (RFC 9728) dit que le serveur d'autorisation est dashu lui-même. Claude lit alors `/.well-known/oauth-authorization-server` (RFC 8414) : les adresses des étapes suivantes, et **PKCE** obligatoire. PKCE (Proof Key for Code Exchange) : Claude invente un secret, n'en envoie que l'empreinte à l'autorisation, puis le secret lui-même à l'échange. Un code intercepté ne sert donc à rien.
3. **Enregistrement** (`POST /oauth/register`, RFC 7591) : Claude déclare son nom et ses adresses de retour ; dashu lui attribue un identifiant de client. C'est un client public, sans secret.
4. **Autorisation** (`GET /oauth/authorize`) : la page décrite plus haut. Le consentement est un formulaire avec jeton CSRF. Après Autoriser, dashu renvoie le navigateur vers l'adresse de retour avec un **code d'autorisation** et le `state` reçu.
5. **Échange** (`POST /oauth/token`) : Claude échange le code et le secret PKCE contre un **jeton d'accès** (1 heure) et un **jeton de renouvellement** (90 jours), qui sert à obtenir un nouveau jeton d'accès sans repasser par la page.

- La connexion, par mot de passe ou par Google, sait revenir à l'autorisation en cours. Aujourd'hui, elle mène toujours à `/`. L'autorisation en attente est gardée dans la session, pas dans l'adresse.
- `POST /oauth/revoke` (RFC 7009) révoque un jeton, si Claude l'appelle en se déconnectant.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `access_tokens` | id, user\_id, grant\_id, name, token\_hash, created\_at, expires\_at, last\_used\_at | Jetons créés à la main (`grant_id` vide) et jetons d'accès OAuth. `last_used_at` écrit au plus une fois par minute. Un jeton d'accès OAuth expiré est effacé |
| `oauth_clients` | client\_id, name, redirect\_uris, created\_at | Clients enregistrés. Un client jamais autorisé est effacé au bout de 24 heures |
| `oauth_codes` | code\_hash, client\_id, user\_id, redirect\_uri, code\_challenge, expires\_at | Codes d'autorisation : 60 secondes, effacés à l'usage |
| `oauth_grants` | id, user\_id, client\_id, refresh\_hash, previous\_refresh\_hash, created\_at, last\_used\_at, expires\_at | Une autorisation donnée à Claude. La supprimer supprime ses jetons d'accès |

Jetons, codes et jetons de renouvellement font 32 octets aléatoires, préfixés `dashu_` pour qu'un jeton oublié dans un fichier se repère d'un coup d'œil ou d'un `grep`. La base n'en garde que l'empreinte SHA-256 : un jeton aléatoire de 256 bits n'a pas besoin d'Argon2, qui sert à ralentir la recherche d'un mot de passe facile à deviner.

## Sécurité

- **Un accès, un compte.** Chaque jeton agit au nom de son compte, avec ses droits sur le site et jamais plus : l'accès passe par `space_access`, et un élément hors de ses espaces répond 404. Le compte et ses espaces sont relus à chaque requête.
- **Retirer un compte coupe ses accès.** Un compte retiré de `ALLOWED_EMAILS` reçoit 401 à la requête suivante, comme sa session.
- **Les jetons ne se lisent nulle part** : ni en base (empreinte seulement), ni dans les logs, ni dans une adresse. Les logs gardent le compte et le nom de l'outil, pas le contenu.
- **Le contenu est de la donnée.** Les instructions du serveur et les annotations le disent à Claude, mais ce ne sont que des indications : c'est Claude qui décide de demander confirmation. Les vrais garde-fous sont côté dashu : une suppression part dans la corbeille, et ni ce qui protège le compte ni l'administration ne sont accessibles.
- **Origine.** `/mcp` accepte une requête sans en-tête `Origin`, ou venant de dashu ou de Claude ; il refuse les autres (403). Une page web ouverte dans le navigateur ne peut donc pas l'appeler. MCP l'exige.
- **Adresses de retour : Claude seulement.** L'enregistrement est ouvert à tous, c'est ce qu'attend Claude. Mais il n'accepte que les adresses de retour de Claude : `https://claude.ai/api/mcp/auth_callback` et `https://claude.com/api/mcp/auth_callback` (à vérifier au moment de la PR), et l'adresse locale de Claude Code (`http://localhost` ou `http://127.0.0.1`, n'importe quel port). Sans cette règle, n'importe quel site pourrait s'enregistrer, montrer la page Autoriser sous un faux nom et repartir avec un accès.
- **Autorisation stricte.** PKCE en S256 obligatoire. Adresse de retour comparée en entier à celle enregistrée. Le paramètre `resource` envoyé par Claude doit désigner `/mcp` sous `BASE_URL`. Code à usage unique, valable 60 secondes, lié au client, à l'adresse de retour et au compte. Consentement demandé à chaque nouvelle autorisation. La page ne s'affiche pas dans un cadre (`frame-ancestors 'none'`, comme tout le site) : pas de clic piégé.
- **Jeton de renouvellement à usage unique.** Chaque renouvellement en donne un nouveau. Si un ancien jeton revient, c'est qu'il a été copié : toute l'autorisation est révoquée.
- **Chaque jeton à sa place.** Les jetons OAuth ne valent que pour `/mcp`. Les jetons créés à la main valent pour `/mcp` et `/api/v1`. `API_TOKEN` (Hermès) ne vaut que pour `/api/v1`.
- **Limites.** `/oauth/register` et `/oauth/token` ont leur propre limiteur par adresse IP, large comme celui des liens de partage (20 d'un coup, puis une requête toutes les 3 secondes), parce que tous les appels de claude.ai partent des mêmes adresses. `/mcp` : 60 appels d'un coup par jeton, puis un par seconde. 15 Mo par requête, de quoi envoyer un fichier de 10 Mo en base64. Pas de requêtes par lots : un tableau de messages JSON-RPC est refusé.
- **Aucun cookie** sur `/mcp` ni sur `/oauth/token` : pas de CSRF à prévoir. À l'inverse, une session ouverte n'ouvre pas `/mcp`.

## Prérequis

- Rien de nouveau à configurer : l'adresse publique est `BASE_URL`.
- claude.ai appelle dashu depuis les serveurs d'Anthropic. L'étape 3 ne marche donc que par `https://app.dashu.dev`, pas par l'adresse Tailscale. Vérifier que Cloudflare ne pose pas de défi anti-robot (Bot Fight Mode) sur `/mcp`, `/.well-known/` et `/oauth/`.

## Critères d'acceptation

Étape 1 :

- [ ] Réglages › Connexions crée un jeton, l'affiche une fois et n'en garde que l'empreinte ; un nom vide ou un 21e jeton sont refusés
- [ ] Un jeton agit sur `/api/v1` au nom de son compte : il voit ses espaces et les espaces partagés rejoints, et reçoit 404 ailleurs
- [ ] Jeton révoqué, expiré, « Tout révoquer » ou compte retiré de `ALLOWED_EMAILS` : 401 à la requête suivante
- [ ] `API_TOKEN` d'Hermès marche comme avant

Étape 2 :

- [ ] Claude Code se branche avec un jeton personnel, liste les outils et les appelle
- [ ] Chaque outil suit les règles et les limites de l'API, testé sur les mêmes cas ; une erreur de saisie revient en résultat d'erreur lisible
- [ ] Sans jeton : 401 et `WWW-Authenticate` ; `Origin` étranger : 403 ; lot JSON-RPC ou méthode inconnue : erreur JSON-RPC propre ; `GET /mcp` : 405
- [ ] Les annotations et les instructions du serveur sont présentes
- [ ] Une note modifiée par MCP pendant qu'on l'édite n'est pas écrasée

Étape 3 :

- [ ] Les deux documents `/.well-known/` sont servis et pointent vers `BASE_URL`
- [ ] Depuis claude.ai : ajout du connecteur, connexion, autorisation, puis outils utilisables dans claude.ai et dans l'app iPhone ; Claude Code se connecte sans jeton collé
- [ ] Un second compte branche son propre Claude et ne voit que ses espaces et les espaces partagés rejoints
- [ ] Refusés : adresse de retour hors de Claude ou différente de celle enregistrée, PKCE absent ou faux, code réutilisé ou expiré, jeton OAuth présenté à `/api/v1`
- [ ] Un jeton de renouvellement réutilisé révoque toute l'autorisation
- [ ] Révoquer dans Réglages coupe Claude à la requête suivante
- [ ] Chaque point a ses tests d'intégration (flux OAuth complet joué dans `tests/`) ; essai réel depuis claude.ai et l'iPhone, avec deux comptes ; STATUS et README à jour

## Décisions

Prises avec Rémi le 20 novembre 2026 :

- **Chaque compte branche son propre Claude**, sur son propre compte : pas d'accès partagé au nom de Rémi.
- **OAuth, pour que claude.ai et l'iPhone marchent**, en trois étapes : jetons par compte, route `/mcp`, OAuth. Un serveur MCP local sur le Mac a été écarté : il ne servirait qu'à Rémi, et ni à claude.ai ni à l'iPhone.

Par défaut, pour avancer :

- **Protocole écrit à la main** (JSON-RPC avec serde), sans SDK : la partie utile (`initialize`, `tools/list`, `tools/call`, `ping`) est petite. Le SDK Rust officiel (`rmcp`) reste possible si le protocole grossit.
- **Durées** : 90 jours par défaut pour un jeton créé à la main ; 1 heure pour un jeton d'accès OAuth ; 90 jours pour un jeton de renouvellement.
- **Un seul niveau d'accès** (lecture et écriture). Un accès en lecture seule viendra si le besoin se présente.
- **Adresses de retour de Claude seulement**, en liste fixe dans le code. Un autre client demanderait une variable.
- **Hermès garde `API_TOKEN`** ; il pourra passer à un jeton personnel plus tard.
- **Les outils suivent l'API**, lot par lot : un outil par action. Si Claude s'y perd quand ils seront nombreux (une cinquantaine), on les regroupera par domaine.
- **Noms d'outils en français, sans accent** : un nom d'outil n'accepte ni accent ni espace.

# dashu — API complète et corbeille

Nov 20, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 20 novembre 2026, avec le connecteur Claude : l'API doit permettre de tout faire dans dashu.

Elle date de la PR #1 (11 novembre) et ne couvre que les espaces en lecture, les tâches, les notes sans suppression et l'agenda en lecture. Depuis, le site a gagné les fichiers, les dossiers, les conversions, les événements, les espaces libres et partagés, les membres et de nombreux réglages. Hermès, avec sa commande `dashu` (`deploy/hermes/`), et le connecteur Claude passent tous deux par l'API : ils ne font que ce qu'elle couvre.

dashu n'a pas de corbeille : une suppression est définitive, et seule la restauration de toute la base (sauvegarde de la nuit) la rattrape. Ouvrir les suppressions à un assistant demande donc d'abord une corbeille. Une erreur de l'assistant, ou une consigne glissée dans une note partagée ou dans un mail lu par Hermès, doit pouvoir se rattraper.

## Corbeille

- Supprimer une note, un fichier, un dossier ou un espace, depuis le site comme depuis l'API, le met dans la **corbeille** au lieu de l'effacer. Un dossier y part avec son contenu, un espace avec tout le sien.
- Ce qui est dans la corbeille disparaît partout : listes, recherche, barre de commandes, Perso, widgets, API. Les liens de partage d'un fichier supprimé sont révoqués. Une note ouverte à plusieurs se ferme chez tout le monde (« Cette note a été supprimée »).
- Une page **Corbeille**, dans Réglages, liste ce qui a été supprimé, du plus récent au plus ancien : le nom, l'espace d'origine, qui l'a supprimé, quand, et par où (le site, ou le nom du jeton ou de l'autorisation : « Hermès », « Claude Code du Mac »…).
- **Restaurer** remet l'élément à sa place, dans son espace et son dossier. Si le dossier n'existe plus, il revient à la racine de l'espace ; si l'espace n'est plus accessible, à la racine de Perso de celui qui restaure.
- **Supprimer définitivement** un élément, ou **Vider la corbeille** : sur le site seulement, avec une confirmation. Jamais par l'API.
- Au bout de **30 jours**, dashu efface l'élément pour de bon, tout seul, fichiers sur le disque compris.
- Dans un espace partagé, la corbeille de l'espace est commune : tout membre voit ce qui y a été supprimé et peut le restaurer, comme il pouvait le supprimer. Un espace supprimé ne se restaure que par son propriétaire.
- Les tâches et les événements ne passent pas par la corbeille : leur suppression reste définitive, comme aujourd'hui. Ils sont petits et se recréent vite.
- Ce qui est dans la corbeille compte toujours dans la place occupée sur le disque.

## L'API couvre tout le site

Chaque action du site a son équivalent dans l'API, sauf la courte liste « Hors de l'API » plus bas. Le site, l'API et le connecteur appellent les mêmes fonctions : une règle (droits, limites, actions réservées au propriétaire) s'écrit une fois et vaut partout.

| Domaine | Ce que l'API ajoute |
| --- | --- |
| Tâches | Changer une tâche d'espace |
| Notes | Ranger dans un dossier, retirer un tag, supprimer (corbeille), télécharger en Markdown ou en PDF |
| Fichiers | Lister et chercher (espace, tag, dossier), lire les informations, télécharger, envoyer, renommer, changer d'espace ou de dossier, garder un fichier de Temporaire, poser et retirer des tags, supprimer (corbeille) |
| Liens de partage | Créer (1 heure, 1 jour ou 7 jours), lister, révoquer |
| Dossiers | Lister l'arborescence, créer, renommer, déplacer, supprimer (corbeille, avec le contenu) |
| Outils | Formats possibles pour un fichier, convertir, compresser, suivre une conversion, remplacer l'original par le résultat (l'original part dans la corbeille) |
| Agenda | Créer, modifier et supprimer un événement, et sa copie dans Google Agenda si l'agenda est relié ; lister, ajouter et retirer une source iCal |
| Espaces | Créer, renommer, changer la couleur, l'icône et les règles automatiques, archiver, désarchiver, supprimer (corbeille, ou en transférant le contenu), choisir l'espace « moment » |
| Membres | Voir les membres et les invitations d'un espace, inviter, annuler une invitation, retirer un membre, quitter ; voir, accepter et refuser les invitations reçues |
| Corbeille | Lister, restaurer |
| Réglages | Profil (prénom, nom, avatar), thème, taille de l'interface, barre latérale, onglets du téléphone, disposition du tableau de bord de chaque espace |

- Un envoi de fichier par l'API suit le même protocole par morceaux que le site, avec la même taille maximale. Par MCP, un fichier se lit et s'envoie jusqu'à 10 Mo (en base64 à l'envoi) ; au-delà, on passe par l'API ou par le site.
- Un fichier téléchargé par l'API est servi comme sur le site : en pièce jointe, jamais exécuté.

## Hors de l'API

Ces actions restent sur le site ou sur le serveur, exprès :

- **Ce qui protège le compte** : mot de passe, liaison avec Google, connexion de Google Agenda (elle passe de toute façon par le navigateur), jetons et autorisations (Réglages › Connexions). Sinon, un jeton volé deviendrait le compte entier, pour toujours : il pourrait changer le mot de passe ou se créer d'autres jetons.
- **L'effacement définitif** : supprimer définitivement, vider la corbeille.
- **Les comptes et le serveur** : les adresses autorisées (`dashu-compte`) et l'administration (`dashu-admin`) se font sur le serveur, pas dans dashu. Hermès garde son accès `dashu-admin` à six actions.
- **L'interface elle-même** : l'espace actif, la barre de commandes, la navigation. Un programme n'en a pas besoin : il désigne l'espace dans chaque requête.
- **Le téléchargeur de liens** (YouTube, Instagram, TikTok), quand il existera : réservé au site et au compte de Rémi, comme le prévoit la v2.

## Hermès et Claude suivent

- Chaque lot ajoute, dans la même PR : les routes de l'API, les outils MCP (dès que la route `/mcp` existe), les commandes de `deploy/hermes/dashu` et les lignes du skill d'Hermès, les tests et le README.
- Le skill d'Hermès change de description : il ne parle plus seulement de tâches, de notes et d'emploi du temps. Sa règle « pas de commande pour supprimer une note » devient « une suppression part dans la corbeille ».
- **Confirmation avant d'agir**, par une règle du skill pour Hermès et par l'annotation « destructrice » pour Claude : toute suppression, remplacer le texte d'une note ou l'original d'un fichier, créer un lien de partage, inviter ou retirer un membre, quitter, archiver ou supprimer un espace.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `notes`, `files`, `folders`, `spaces` | + `deleted_at`, `deleted_by`, `deleted_via`, `trash_op` | Remplies quand l'élément part dans la corbeille. `deleted_via` : le site, ou le nom du jeton ou de l'autorisation |
| vue `space_access` | | Ne donne plus les espaces qui sont dans la corbeille |

Un dossier supprimé marque tout son contenu avec le même numéro d'opération (`trash_op`) : le restaurer ramène l'ensemble, sans ramener ce qui avait été supprimé à part avant. Un espace supprimé n'a pas besoin de marquer son contenu : `space_access` le cache déjà.

## Sécurité

- **Toute requête de contenu filtre la corbeille.** C'est le risque principal de ce chantier : une liste oubliée montrerait un élément supprimé. Un test par endroit : listes, recherche, barre de commandes, Perso, widgets, API, liens de partage, `/open`.
- **Mêmes droits partout.** L'API passe par les mêmes fonctions que le site : `space_access`, actions du propriétaire vérifiées par le serveur, mêmes limites (taille d'envoi, 8 espaces, longueur des noms).
- **Sources iCal** : même validation que le site, adresses publiques seulement. Sinon, un assistant trompé pourrait faire lire à dashu une adresse du réseau local.
- **Liens de partage** : en créer un rend un fichier public. Hermès et Claude demandent confirmation, et chaque lien apparaît sur la page du fichier, révocable.
- **Trace.** Chaque suppression note par où elle est passée, et les logs gardent le compte, le jeton et l'action, sans le contenu. On voit ce qu'un assistant a fait.
- **Restaurer** vérifie l'accès de celui qui restaure, comme toute action.

## Critères d'acceptation

Corbeille :

- [ ] Supprimer une note, un fichier, un dossier (avec son contenu) ou un espace, depuis le site ou l'API, le met dans la corbeille ; il disparaît de partout (un test par endroit)
- [ ] La page Corbeille montre qui a supprimé, quand et par où ; restaurer remet l'élément à sa place, ou à la racine si sa place n'existe plus
- [ ] Supprimer définitivement et vider la corbeille : sur le site seulement, avec confirmation ; l'API n'a pas de route pour ça
- [ ] Après 30 jours, l'élément est effacé, fichier sur le disque compris
- [ ] Dans un espace partagé, tout membre voit et restaure ce qui y a été supprimé ; un autre compte reçoit 404
- [ ] Les liens de partage d'un fichier supprimé ne marchent plus ; une note supprimée ferme son canal en direct

API :

- [ ] Chaque ligne du tableau « L'API couvre tout le site » a sa route, avec ses tests : le cas normal, un élément d'un autre compte (404), une action de propriétaire tentée par un membre (refus), une saisie invalide
- [ ] Les actions « Hors de l'API » n'ont pas de route
- [ ] Un fichier envoyé par l'API arrive identique ; un fichier téléchargé est servi en pièce jointe
- [ ] Une source iCal vers une adresse privée est refusée
- [ ] La commande `dashu` d'Hermès et son skill couvrent chaque nouvelle route, comme les outils MCP quand ils existent
- [ ] STATUS et README à jour à chaque lot

## Ordre de livraison

Une PR par ligne, avec le connecteur Claude :

1. Jetons par compte (connecteur, étape 1)
2. Route `/mcp` sur le périmètre actuel (connecteur, étape 2)
3. OAuth, pour claude.ai et l'iPhone (connecteur, étape 3)
4. Corbeille, sur le site
5. API : notes complètes, fichiers, dossiers, liens de partage, corbeille
6. API : outils de conversion
7. API : événements et sources iCal
8. API : espaces, membres et réglages

Les lots 5 à 8 mettent à jour l'API, les outils MCP et la commande d'Hermès en même temps.

## Décisions

Prises avec Rémi le 20 novembre 2026 :

- **L'API fait tout ce que fait le site**, pour Hermès comme pour Claude. Ça remplace « pas de suppression de note par l'API » (PR #1), « pas de suppression ni de partage de fichier par l'API » (v2) et « pas d'action API pour créer un événement » (Événements et Google Agenda).
- **Corbeille d'abord** : une suppression par l'API passe par une corbeille de 30 jours, restaurable depuis le site.

Par défaut, pour avancer :

- **La corbeille vaut aussi pour le site** : un seul comportement, et un clic malheureux se rattrape aussi.
- **Tâches et événements hors de la corbeille** : leur suppression reste définitive.
- **30 jours** avant l'effacement définitif, qui ne se déclenche à la main que depuis le site.
- **Ce qui protège le compte reste hors de l'API**, comme l'administration du serveur.
- **Fichiers par MCP jusqu'à 10 Mo** ; au-delà, l'API (envoi par morceaux) ou le site.
- **Ordre** : le connecteur d'abord, pour que Rémi et son collègue s'en servent vite, puis la corbeille, puis l'API par lots. La corbeille arrive avant la première suppression par l'API.

Pendant la réalisation (23 novembre 2026) :

- **Comment la corbeille se cache** : une note, un fichier ou un dossier supprimé passe dans l'espace « corbeille » caché de son espace (`spaces.trash_of`), que la vue `space_access` ne donne à personne ; un espace supprimé est marqué `deleted_at`, et la vue le cache avec son contenu. Toutes les requêtes de contenu passaient déjà par `space_access` (espaces partagés) : rien n'a eu à être filtré requête par requête, ce qui était le risque principal noté plus haut. Les colonnes prévues (`deleted_at`, `deleted_by`, `deleted_via`, `trash_op`) sont là ; s'y ajoutent `trash_root` (la ligne que la personne a supprimée), `trash_folder_id` (son dossier d'origine) et, pour un dossier, `trash_name` (les noms de dossiers sont uniques par emplacement).
- **Un espace supprimé perd ses membres**, comme avant la corbeille : ils sont retirés (leurs agendas rentrent chez eux) et ne reviennent pas s'il est restauré. Son slug est libéré pour un nouvel espace, et repris à la restauration s'il est encore libre.
- **Supprimer un dossier en transférant son contenu** : le contenu monte d'un niveau, puis le dossier, vide, part dans la corbeille.

---

# dashu — Installation simple

Nov 23, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 23 novembre 2026 : installer dashu sur n'importe quelle machine en tapant une commande, en répondant aux questions au fur et à mesure, et que tout marche. Sans nom de domaine obligatoire.

Aujourd'hui, c'est laborieux :

- `deploy/lxc/install.sh` prend l'email et l'adresse en arguments, puis **compile** dashu sur place : 10 à 20 minutes, Rust à installer, environ 1,2 Go de mémoire. `dashu-update` recompile aussi à chaque mise à jour.
- Il ne marche qu'en x86_64 (processeurs Intel et AMD) : Typst y est téléchargé dans cette seule version.
- Le compte se crée avec une commande à recopier. Tailscale, le tunnel Cloudflare et Google se font à la main, en suivant le README.
- La page d'accueil (dashu.dev) propose d'installer dashu avec Docker, mais aucune image n'est publiée.

L'installation s'adresse aussi à d'autres personnes que Rémi. Le travail se fait en six étapes, une PR chacune. Chaque étape sert dès qu'elle est fusionnée : après la deuxième, on installe dashu en une commande ; les étapes 4 et 5 permettent de créer le compte et de régler Google depuis le navigateur, sans terminal.

## Fonctionnement

### Étape 1 : des binaires tout faits

- **Licence AGPL** (version 3 ou ultérieure) : fichier `LICENSE`, champ `license` de `Cargo.toml`, section Licence du README.
- `dashu version` affiche la version (celle de `Cargo.toml`) et le commit dont le binaire est issu.
- La CI compile dashu pour Linux, en x86_64 et en aarch64 (ARM 64 bits : Raspberry Pi 4 et 5, serveurs ARM). Les binaires sont liés à **musl**, une bibliothèque C embarquée dans le binaire : il ne dépend pas de celle de la machine et tourne sur toute distribution Linux. L'image Docker l'est déjà.
- Chaque architecture est compilée sur son propre runner GitHub (la machine qui exécute la CI), x86_64 ou ARM : pas de compilation croisée, que ring et SQLite, écrits en partie en C, compliquent.
- Une **version publiée** (release GitHub) contient, par architecture, une archive `dashu-<arch>-linux.tar.gz` : le binaire, `deploy/` (services et scripts) et `LICENSE`. Le fichier `SHA256SUMS` donne l'empreinte SHA-256 de chaque archive, ce qui permet de vérifier qu'elle est arrivée intacte.
- Deux **canaux**, c'est-à-dire deux suites de versions qu'une installation peut suivre :
  - `v1` : à chaque push sur la branche `v1`, une fois la CI verte, la pré-version `canal-v1` est remplacée. C'est le canal du serveur de Rémi.
  - `stable` : chaque tag `vX.Y.Z` crée une version. C'est le canal par défaut des autres installations.
- **Image Docker** `ghcr.io/moreauremi/dashu`, pour amd64 et arm64, construite à partir des mêmes binaires. Tags : `v1`, `X.Y.Z` et `latest` (la dernière version stable). Elle fait moins de 30 Mo, comme aujourd'hui.
- Les outils de conversion (ffmpeg, pandoc, Typst, Ghostscript, heif-dec) restent hors du binaire et de l'image : l'installateur les propose.

### Étape 2 : l'installateur pose les questions

```
curl -fsSL https://raw.githubusercontent.com/moreauremi/dashu/v1/deploy/lxc/install.sh | sudo bash
```

Le script vérifie d'abord la machine : Linux avec systemd, x86_64 ou aarch64, droits root. Sinon, il s'arrête en expliquant pourquoi, et renvoie vers Docker pour macOS, Windows ou un NAS. Puis il télécharge l'archive du canal `stable` pour l'architecture de la machine, vérifie son empreinte, installe le binaire, crée l'utilisateur `dashu`, ses dossiers et ses services, comme aujourd'hui.

Ensuite, il pose les questions, dans cet ordre.

**1. Comment accéder à dashu ?** Un ou plusieurs choix :

| Mode | Ce que fait le script | Adresse | Limites |
| --- | --- | --- | --- |
| Local | dashu écoute sur le réseau de la maison | `http://<ip>:3000` | Pas de HTTPS : le navigateur signale une connexion non sécurisée et le mot de passe passe en clair sur le réseau local. Rien depuis l'extérieur, pas de Google, pas de connecteur Claude sur claude.ai |
| Tailscale | Installe Tailscale, affiche le lien de connexion au compte Tailscale, puis publie dashu sur le réseau Tailscale (`tailscale serve`) | `https://<machine>.<réseau>.ts.net` | Seulement les appareils du réseau Tailscale : pas de connecteur Claude sur claude.ai, liens de partage inutiles pour les autres |
| Tailscale Funnel | Comme Tailscale, puis ouvre cette adresse à tout Internet (`tailscale funnel`) | La même, publique | Pas de nom personnalisé |
| Domaine + Cloudflare | Demande le domaine et le jeton du tunnel, installe `cloudflared` et dit quelle route publier dans Cloudflare | `https://app.mondomaine.fr` | Il faut un domaine et un compte Cloudflare |

- **Funnel** donne une adresse publique en HTTPS sans acheter de domaine ni passer par Cloudflare. Il s'active dans la console Tailscale : si ce n'est pas fait, le script dit quoi cocher.
- Pour Tailscale, le script vérifie que MagicDNS et HTTPS sont activés dans la console, et que `/dev/net/tun` existe. Dans un conteneur LXC qui ne l'a pas, il affiche les deux lignes à ajouter côté Proxmox.
- `BASE_URL`, l'adresse publique de dashu, est déduite du choix. Avec plusieurs modes, c'est la plus ouverte : Cloudflare, puis Funnel, puis Tailscale, puis local. Les autres adresses servent à se connecter par mot de passe, comme Tailscale chez Rémi aujourd'hui.

**2. Activer les fichiers ?** Oui ou non, puis le dossier (`/srv/dashu` par défaut). Si les sauvegardes tombent sur le même disque que ce qu'elles protègent, le script le dit.

**3. Activer les conversions ?** Le script annonce la place nécessaire (plusieurs centaines de Mo), installe les paquets, Typst pour la bonne architecture, et le service de conversion. Une version de Typst différente de celle attendue est remplacée.

**4. Le compte** : email, puis mot de passe tapé deux fois, sans s'afficher.

**5. Google, facultatif** : le script affiche l'adresse de retour exacte à déclarer, rappelle les étapes de la console Google, puis demande l'identifiant et le secret, sans afficher le secret. On peut passer et y revenir plus tard.

À la fin, le script attend que dashu réponde, vérifie l'adresse principale, puis affiche un résumé : l'adresse, et les commandes utiles (`dashu-update`, `dashu-compte`, sauvegarde et restauration).

- **Relancer le script** sur une machine déjà installée affiche un menu : mettre à jour, changer l'accès, activer les fichiers ou les conversions, ajouter Google, changer le mot de passe. La base n'est jamais touchée. `dashu.env` est copié avant chaque changement et remis en place si dashu ne redémarre pas, comme le fait déjà `dashu-compte`.
- **Les questions lisent le terminal** (`/dev/tty`) : avec `curl … | bash`, l'entrée du script est le téléchargement, pas le clavier. Sans terminal, le script s'arrête, sauf en mode `--non-interactif`.
- **Rien ne casse** : `install.sh <email> <adresse>` installe comme avant, sans question. `--non-interactif` prend les réponses en options, pour automatiser. `--binaire <fichier>` installe un binaire local, pour les tests. `--depuis-les-sources` compile comme aujourd'hui.
- Au passage, `.env.example` est remis d'accord avec le README (`BASE_URL`, l'API sans `API_TOKEN`). L'avertissement « `ALLOWED_EMAILS` est vide », émis aujourd'hui avant que les logs ne soient en place et donc perdu, s'affiche vraiment.

### Étape 3 : la mise à jour télécharge

- `dashu-update` lit le canal dans `/etc/dashu/branch` (`v1` chez Rémi, `stable` ailleurs). Il télécharge l'archive de son architecture, vérifie son empreinte et compare la version avec `dashu version`. Rien à faire : « déjà à jour ».
- La suite ne change pas : instantané de la base, ancien binaire gardé (`dashu.prev`), installation, redémarrage, attente de `/health`, retour arrière en cas d'échec.
- Les scripts d'exploitation viennent de l'archive, rangée dans `/opt/dashu/share`, et non plus du dépôt git `/opt/dashu/src`.
- Une mise à jour prend une à deux minutes au lieu de 10 à 20. `dashu-update --depuis-les-sources` compile encore, en secours.
- `dashu-admin` garde exactement ses six actions. `status` lit la version avec `dashu version` au lieu de git, et `update` ne parle plus de recompiler.
- Hermès : `deploy/hermes/install.sh` copie ses fichiers depuis `/opt/dashu/share`, et son skill décrit la mise à jour telle qu'elle devient.
- **Bascule du serveur de Rémi** : une commande décrite dans le README. Rust et `/opt/dashu/src` restent en place jusqu'à ce que Rémi valide ; leur retrait est une commande à part.

### Étape 4 : premier lancement dans le navigateur

L'installateur ne pose plus que les questions du système (accès, fichiers, conversions). Le compte et Google se règlent dans le navigateur. Avec Docker, sur n'importe quel système :

```
docker run -d -p 3000:3000 -v dashu:/data ghcr.io/moreauremi/dashu
```

- **`SESSION_SECRET` devient facultatif.** Absent, dashu le crée au premier démarrage dans un fichier `secret` à côté de la base, lisible par dashu seul, et le garde ensuite. `dashu-backup` le sauvegarde avec la base : sans lui, les jetons Google Agenda d'une base restaurée seraient illisibles.
- **Mode installation**, tant qu'aucun compte n'existe :
  - Au démarrage, dashu écrit dans son journal un **code d'installation** de 12 caractères (`K7QM-2XPA-9RTD`), nouveau à chaque démarrage. L'installateur l'affiche avec l'adresse ; sous Docker, `docker logs` le montre.
  - Toute page mène à `/setup`, sauf `/health` et les fichiers statiques. L'API et `/mcp` sont fermés.
  - `/setup` demande le code, puis trois étapes :
    1. **Le compte** : email et mot de passe (12 caractères au moins). Ce compte est l'administrateur de l'instance.
    2. **L'adresse publique** : dashu propose celle que le navigateur utilise en ce moment. Il vérifie qu'elle mène bien à lui quand c'est possible, et prévient sans bloquer sinon.
    3. **Google, facultatif** : l'adresse de retour s'affiche avec un bouton Copier, puis les étapes de la console Google, l'identifiant et le secret. On peut passer.
  - À la fin, dashu enregistre, ouvre la session du compte et se relance pour prendre la nouvelle configuration. La session survit à la relance : le secret ne change pas.
- **Les réglages de l'instance** (adresse publique, Google, adresses autorisées) sont rangés en base. **Les variables d'environnement restent prioritaires** : une valeur posée dans `dashu.env` ne se change pas depuis le site. Le serveur de Rémi, configuré par `dashu.env`, ne voit aucun changement.
- `ALLOWED_EMAILS` devient facultative : absente, la liste vient de la base. `dashu set-password` vérifie la même liste que la page de connexion.

### Étape 5 : la carte Instance dans Réglages

- Visible par l'administrateur seulement. On y change l'adresse publique, Google (le secret n'est jamais réaffiché : on le remplace ou on le retire) et les adresses autorisées.
- Mêmes garde-fous que `dashu-compte` : ni la dernière adresse, ni celle de l'API ne se retirent.
- Une valeur qui vient de l'environnement s'affiche verrouillée : « définie sur le serveur ».
- Un changement relance dashu : quelques secondes de coupure, les notes ouvertes en direct se reconnectent.
- `dashu-compte` continue de modifier `dashu.env`. Sur une installation sans `ALLOWED_EMAILS`, `ajouter` et `retirer` renvoient vers la carte Instance ; `mot-de-passe` marche partout.

### Étape 6 : la page d'accueil

La section Installer de dashu.dev donne la commande en une ligne et la variante Docker, à la place du `git clone` et du `docker compose up --build`. PR vers `main`.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `instance_settings` | key, value, updated\_at | Réglages de l'instance : `base_url`, `google_client_id`, `google_client_secret`. Le secret Google est chiffré avec la clé tirée du secret de session, comme les jetons Google Agenda |
| `allowed_emails` | email, added\_at, added\_by | Adresses autorisées, quand `ALLOWED_EMAILS` n'est pas posée |
| `users` | + `is_admin` | Le compte créé par l'assistant. Sur une base existante, le plus ancien compte |

## Sécurité

- **Sans le code, rien.** Le premier venu qui tombe sur `/setup` ne prend pas l'instance. Le code est aléatoire, gardé en mémoire seulement, nouveau à chaque démarrage et comparé à temps constant. Il se tape dans un formulaire avec jeton CSRF, jamais dans l'adresse : ni les logs d'accès ni l'historique du navigateur ne le gardent. Les essais sont limités comme sur `/login`. Il ne s'écrit que dans le journal du service, que seuls root et Docker lisent.
- **`/setup` se ferme pour de bon.** Dès qu'un compte existe, il répond 404, vérifié en base à chaque requête. Pas de remise à zéro par le site.
- **Téléchargements vérifiés.** Tout passe en HTTPS, et chaque archive est comparée à `SHA256SUMS` avant d'être installée. Limite assumée : l'empreinte protège d'un téléchargement abîmé, pas d'une version publiée par quelqu'un qui aurait pris la main sur le dépôt (voir « Reste à trancher »). Tailscale et `cloudflared` viennent de leurs dépôts officiels.
- **Les secrets ne s'affichent jamais.** Mot de passe, jeton du tunnel et secret Google se tapent sans écho. Ils ne passent jamais en argument d'une commande (visible de tous par `ps`) ni dans les logs. `dashu.env` reste en 640 (root et le groupe `dashu`), le fichier `secret` en 600.
- **Le service reste enfermé.** L'assistant web écrit en base, jamais dans le système ni dans `/etc`. Installer des paquets, Tailscale ou `cloudflared` reste le travail du script, en root. `hardening.conf` ne change pas.
- **L'environnement prime.** Sur un serveur configuré par fichier, comme celui de Rémi, un compte volé ne peut pas changer l'adresse ni Google depuis le site. La carte Instance est réservée à l'administrateur, avec jeton CSRF.
- **Un accès public, c'est Internet.** Avec Funnel ou Cloudflare, la page de connexion est ouverte à tous : le script le dit, et le limiteur de `/login` s'applique. En mode local, il prévient que le mot de passe passe en clair sur le réseau.
- **`dashu-admin` ne change pas** : six actions, sans paramètre libre. `dashu-update` n'installe qu'une archive vérifiée.

## Prérequis

- Le dépôt est déjà public. Sur GitHub, la release demande les droits `contents: write`, l'image `packages: write`. Une fois l'image publiée, Rémi rend le paquet public dans les réglages de GitHub (une seule fois).
- Les runners ARM de GitHub (`ubuntu-24.04-arm`) sont gratuits pour les dépôts publics.
- Pour les essais à la main : un conteneur LXC Debian 13 de test sur le Proxmox et, si possible, une machine ARM.
- À vérifier pendant l'étape 2 : que Google accepte une adresse de retour en `.ts.net` (Tailscale et Funnel).

## Critères d'acceptation

Étape 1 :

- [ ] `LICENSE` contient l'AGPL-3.0, `Cargo.toml` la déclare, le README le dit
- [ ] `dashu version` affiche la version et le commit
- [ ] Un push sur `v1` (CI verte) remplace la pré-version `v1` ; un tag `vX.Y.Z` crée une version ; chacune a les archives x86_64 et aarch64 et `SHA256SUMS`
- [ ] Le binaire aarch64 démarre sur un runner ARM et répond à `/health`
- [ ] L'image `ghcr.io/moreauremi/dashu` existe pour amd64 et arm64, fait moins de 30 Mo et répond à `/health`
- [ ] Le binaire musl est mesuré contre le binaire actuel (mesures de STATUS) avant la bascule du serveur

Étape 2 :

- [ ] Sur une Debian 13 neuve, `curl … | sudo bash` installe dashu en posant les questions, sans rien compiler
- [ ] Chaque mode d'accès marche de bout en bout : local, Tailscale, Funnel, Cloudflare ; l'adresse principale est vérifiée à la fin
- [ ] Mot de passe, jeton du tunnel et secret Google n'apparaissent ni à l'écran ni dans la liste des processus
- [ ] Une archive dont l'empreinte ne correspond pas est refusée, et rien n'est installé
- [ ] Relancé, le script propose le menu et n'écrase ni la base ni `dashu.env` ; un changement qui empêche dashu de démarrer est annulé
- [ ] `install.sh <email> <adresse>` marche comme avant ; `--non-interactif` installe sans question
- [ ] L'installation marche en x86_64 et en aarch64, Typst compris
- [ ] `tests/deploy/installation.sh` tourne dans la CI : installation non interactive, puis une passe interactive dans un terminal simulé, compte, connexion, relance

Étape 3 :

- [ ] `dashu-update` télécharge, vérifie et installe la version du canal en moins de 2 minutes ; « déjà à jour » si rien n'a changé
- [ ] Un binaire qui ne démarre pas est remplacé par l'ancien et la base restaurée, comme aujourd'hui
- [ ] `dashu-admin` garde exactement ses six actions ; `status` affiche la version sans git
- [ ] Hermès met à jour dashu comme avant ; son installation lit `/opt/dashu/share`
- [ ] Le serveur de Rémi est passé aux binaires ; STATUS relevé

Étape 4 :

- [ ] Sans compte, toute page mène à `/setup` ; l'API et `/mcp` sont fermés ; sans le bon code, rien ne se crée ; les essais sont limités
- [ ] Le code n'apparaît dans aucune adresse ni dans les logs d'accès ; il change à chaque démarrage
- [ ] L'assistant crée le compte administrateur, enregistre l'adresse et, si demandé, Google ; dashu se relance et la session reste ouverte
- [ ] Une fois un compte créé, `/setup` répond 404
- [ ] Sans `SESSION_SECRET`, le secret est créé une fois, gardé au redémarrage et sauvegardé avec la base
- [ ] Une variable d'environnement prime sur la base ; le serveur de Rémi démarre sans aucun changement
- [ ] `docker run` de l'image mène au même assistant
- [ ] L'installateur ne pose plus que les questions du système, puis affiche l'adresse et le code

Étape 5 :

- [ ] La carte Instance n'apparaît que pour l'administrateur ; ses routes répondent 404 aux autres comptes
- [ ] Adresse, Google et adresses autorisées s'y changent ; une valeur de l'environnement s'affiche verrouillée
- [ ] Ni la dernière adresse ni celle de l'API ne se retirent ; une adresse retirée perd l'accès à la requête suivante
- [ ] Le secret Google n'est jamais réaffiché

Étape 6 :

- [ ] La page d'accueil donne la commande d'installation et la variante Docker, qui marchent telles quelles

Toutes les étapes :

- [ ] Chaque point a ses tests (`tests/installation.rs`, `tests/deploy/installation.sh`) ; STATUS et README à jour à chaque étape

## Ordre de livraison

Une PR par ligne :

1. Licence, `dashu version`, binaires et image publiés (étape 1)
2. Installateur interactif (étape 2)
3. `dashu-update` par binaires, puis bascule du serveur de Rémi (étape 3)
4. Premier lancement dans le navigateur (étape 4), après la fusion de la branche `api-contenu`, qui touche les mêmes fichiers (`config.rs`, `lib.rs`, `main.rs`)
5. Carte Instance (étape 5)
6. Page d'accueil, PR vers `main` (étape 6)

## Décisions

Prises avec Rémi le 23 novembre 2026 :

- **L'installation s'adresse à d'autres personnes**, pas seulement aux machines de Rémi : versions numérotées, image Docker publiée, page d'accueil.
- **Le serveur de Rémi passe aux binaires** : `dashu-update` télécharge au lieu de compiler. Le binaire musl est mesuré avant la bascule.
- **Licence AGPL** : qui modifie dashu et le propose en ligne à d'autres doit publier ses modifications.
- **Le domaine n'est pas obligatoire** : quatre modes d'accès au choix, local, Tailscale, Tailscale Funnel, domaine et Cloudflare.
- **Assistant web au premier lancement** (étapes 4 et 5), après l'installateur en terminal.

Par défaut, pour avancer :

- **Linux avec systemd** pour l'installateur, en x86_64 et aarch64 ; Docker pour le reste (macOS, Windows, NAS).
- **musl** pour les binaires : un seul binaire pour toutes les distributions, déjà utilisé par l'image Docker.
- **Runners natifs** x86_64 et ARM, plutôt qu'une compilation croisée.
- **`/etc/dashu/branch` garde son nom** et contient le canal (`v1` ou `stable`).
- **Conversions en option** : ni dans le binaire ni dans l'image Docker ; l'installateur les propose avec leur taille.
- **Adresse principale** : avec plusieurs modes d'accès, `BASE_URL` prend la plus ouverte.
- **Réglages de l'instance en base**, l'environnement restant prioritaire.
- **Relance de dashu après un changement de configuration**, plutôt qu'une configuration modifiable à chaud : plus simple, quelques secondes de coupure.
- **Code d'installation de 12 caractères**, écrit dans le journal seulement.
- **Les unités systemd** ne sont installées que par le script, comme aujourd'hui : `dashu-update` n'y touche pas.

Pendant la réalisation (24 novembre 2026) :

- **Archives sans numéro de version** (`dashu-x86_64-linux.tar.gz`) : l'adresse d'un canal ne change jamais, et le canal stable se télécharge par l'adresse « dernière version » de GitHub (`releases/latest/download/…`), qui ignore les pré-versions. La version se lit avec `dashu version`.
- **La pré-version du canal v1 s'appelle `canal-v1`** : un tag `v1` porterait le même nom que la branche, et git ne saurait plus lequel des deux on désigne.
- **Le binaire est celui de l'image Docker**, extrait de l'image : un seul build par architecture, testé (taille, `/health`, `dashu version`, binaire statique) avant d'être publié. Le job « image Docker » de la CI devient « binaire et image », une fois par architecture.
- **Étapes 2 et 3 dans la même PR** : une machine installée par le nouveau script n'a plus le code dans `/opt/dashu/src`, l'ancien `dashu-update` (qui compile) ne pourrait pas la mettre à jour. Le mode de mise à jour est noté dans `/etc/dashu/source` (`binaire` ou `sources`) ; sans ce fichier, un serveur qui a le code reste en `sources` : celui de Rémi ne change qu'au moment de la bascule.
- **Fichiers toujours dans `/srv/dashu`**, sans question sur le dossier : c'est le chemin que connaît le bac à sable du service (`hardening.conf`, `dashu-worker.service`). Pour un autre disque, on le monte sur `/srv/dashu`. Le service de conversion est installé dès les fichiers (les images se convertissent sans paquet de plus) ; la question « conversions » installe les paquets (ffmpeg, pandoc, Typst…).
- **Jeton du tunnel Cloudflare dans `/etc/dashu/tunnel.env`** (root seulement), lu par l'unité `dashu-tunnel.service` : `cloudflared service install <jeton>`, la commande proposée par Cloudflare, le mettrait dans la ligne de commande et dans l'unité.
- **Sans question, deux accès seulement** : `aucun` (dashu n'écoute que sur la machine, comme l'ancien script) et `local`. Tailscale et Cloudflare demandent une connexion à un compte, donc les questions.
- **Tout le script est dans des fonctions**, appelées par sa dernière ligne : avec `curl … | bash`, bash lit tout le script avant d'exécuter quoi que ce soit, et aucune commande ne peut avaler la suite du téléchargement.
- **L'assistant tient sur une page en trois parties** (compte, adresse, Google) plutôt que trois pages : moins d'allers-retours, et rien n'est enregistré tant que tout n'est pas valable. Le code se donne avant, sur sa propre page.
- **L'adresse est vérifiée depuis le serveur** (`<adresse>/health`) : si elle ne répond pas, la page de fin le dit, sans bloquer (le réseau du serveur n'est pas celui du navigateur).
- **La session survit à la relance** tant que l'adresse ne passe pas de http à https : le nom du cookie change alors (`__Host-dashu`), et on se reconnecte une fois.
- **`dashu-compte`** garde la main sur `ALLOWED_EMAILS` quand la ligne existe ; sans elle, `liste` lit la base, et `ajouter` et `retirer` renvoient vers Réglages › Instance.
- **Le secret de session** reste dans `dashu.env` pour une installation par le script (il le génère, comme avant) ; le fichier `secret` sert à l'image Docker et à toute installation sans `SESSION_SECRET`.
- **Carte Instance** : sa propre adresse ne se retire pas non plus (l'administrateur perdrait l'accès). Un changement range la valeur en base et relance dashu ; la carte lit la base, pas la configuration du démarrage, et montre donc le changement avant même la relance. Les routes de la carte répondent 404 à un compte qui n'est pas administrateur, comme le prévoit la sécurité.

Reste à trancher :

- **Signature des archives** (avec minisign, par exemple) en plus de l'empreinte : elle protégerait d'une version publiée par quelqu'un d'autre que Rémi.
- **Inviter quelqu'un par lien**, plutôt qu'en ajoutant son adresse.
- **Une adresse courte** pour la commande (`https://dashu.dev/install.sh`, servie par la page d'accueil).

---

# dashu — Outils par espace

Nov 25, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 25 novembre 2026, en deux PR :

1. **Choisir les outils affichés dans chaque espace.** Notes et Agenda dans École, mais pas Fichiers, par exemple. Aujourd'hui, la barre latérale est la même partout, écrite à la main dans `layout.html`, et la barre de commandes répète la liste de son côté.
2. **Un outil « 1Life » réservé au compte de Rémi**, pour son travail. Pour l'instant un squelette : trois modules vides.

Un **outil** est une entrée de la barre latérale : Dashboard, Notes, Fichiers, Agenda, Outils, puis 1Life.

## Fonctionnement : outils par espace (PR 1)

- **Un registre unique** (`src/registry.rs`) déclare chaque outil : identifiant, nom, icône, adresse, raccourci clavier, mot de la barre de commandes, affiché par défaut ou non, masquable ou non, réservé ou non. La barre latérale, la barre de commandes (`/go`), les réglages et l'API en sont tirés ; la barre de raccourcis du téléphone le suit. Ajouter un outil = ajouter une entrée, plus ses pages.
- **Réglages › Espaces** : dans chaque espace, la liste des outils, avec un interrupteur affiché / masqué pour chacun. Le changement s'enregistre tout de suite (htmx), sans bouton « Enregistrer », et la barre latérale suit.
- **Le Dashboard (l'accueil de l'espace) et les Réglages ne se masquent pas.**
- **Un outil masqué** disparaît, dans cet espace seulement, de la barre latérale, de `/go` dans la barre de commandes et de la barre de raccourcis du téléphone (« Nouvelle note » suit Notes).
- **Ouvrir l'adresse d'un outil masqué** (`/e/ecole/files`) ramène à l'accueil de l'espace, avec le message « Cet outil est masqué dans cet espace » et un lien vers les réglages.
- **Masquer ne supprime rien.** Notes, fichiers et événements restent en base, et réapparaissent quand on réaffiche l'outil.
- **C'est un réglage d'affichage, pas une permission.** Une note ou un fichier s'ouvre toujours par son adresse, par `/open` ou depuis Perso ; l'API, le connecteur Claude et Hermès travaillent sur tous les outils, même masqués. Les widgets du dashboard ne changent pas : ils se retirent dans la disposition du dashboard.
- **Le réglage est propre à chaque compte.** Dans un espace partagé, chaque membre choisit ses outils, sans changer ceux des autres.
- **API** : `GET /api/v1/spaces/{slug}/tools` lit le réglage, `PUT` le change (`{"tools": {"files": false}}`), avec l'outil MCP et la commande d'Hermès qui vont avec, comme pour la disposition du dashboard.

## Fonctionnement : 1Life, outil réservé (PR 2)

- **Un outil réservé** n'existe que pour les comptes autorisés : ailleurs, il n'apparaît ni dans la barre latérale, ni dans la barre de commandes, ni dans les réglages, ni dans l'API.
- **L'autorisation se donne seulement sur le serveur**, par le binaire :
  - `dashu tools list` : les outils réservés, et les comptes autorisés pour chacun ;
  - `dashu tools grant <email> 1life` ;
  - `dashu tools revoke <email> 1life`.
  Aucune route HTTP, aucun écran, aucune action de l'API ni du connecteur ne donne ou ne retire cet accès.
- **Contrôle côté serveur sur toutes les routes `/1life/*`** : un compte non autorisé reçoit une 404, la même que pour une adresse qui n'existe pas (pas une 403, qui dirait que l'outil existe). Sans session aussi : 404, et non la page de connexion.
- **Une fois autorisé**, 1Life se règle par espace comme les autres outils, **masqué par défaut**. Masqué dans l'espace actif, ses pages ramènent à l'accueil de l'espace, comme les autres outils. Retirer l'autorisation le fait disparaître partout ; ses réglages par espace restent, pour une autorisation future.
- **Contenu, squelette seulement** : une page d'accueil avec trois cartes, « Compte rendu », « Audit » et « Clients », chacune marquée « Bientôt ». Chaque module a son adresse (`/1life/cr`, `/1life/audit`, `/1life/clients`) et une page « Module à venir ». Un petit registre des modules, dans le module 1Life, les déclare : en ajouter un = une entrée.
- **Aucune logique métier pour l'instant** : pas de table pour les modules, pas d'appel à une IA, pas d'action dans l'API ni pour Hermès.

## Modèle de données

| Table | Colonnes | Remarques |
| --- | --- | --- |
| `space_tools` | `user_id`, `space_id`, `tool`, `shown` | Clé (`user_id`, `space_id`, `tool`). Pas de ligne : la valeur par défaut du registre. Rien n'est écrit par la migration : tous les outils actuels restent affichés |
| `tool_grants` | `user_id`, `tool`, `granted_at` | Clé (`user_id`, `tool`). Écrite seulement par `dashu tools grant` et `revoke` (PR 2) |

Un identifiant d'outil inconnu en base (outil retiré du registre) est ignoré.

## Sécurité

- **Masquer n'est pas une permission** : aucune vérification d'accès ne dépend de `space_tools`. Les droits restent ceux de `space_access`.
- **Un outil réservé se vérifie sur le serveur**, à chaque requête de ses pages, à partir de `tool_grants`. Masquer le lien ne suffit jamais.
- **Personne ne s'autorise lui-même** : aucune route n'écrit dans `tool_grants` ; l'API et le connecteur répondent sur un outil réservé non autorisé exactement comme sur un outil inconnu.
- **Dépôt public** : aucun nom de client, aucun document ni aucune information interne de 1Life dans le code, les tests ou les fixtures.

## Critères d'acceptation

PR 1 :

- [ ] Masquer Fichiers dans École le retire de la barre latérale, de `/go` et de la barre du téléphone dans École seulement ; le réafficher le ramène, et les fichiers d'École sont tous là
- [ ] `/e/ecole/files`, Fichiers masqué dans École, ramène à `/e/ecole/` avec « Cet outil est masqué dans cet espace » et un lien vers les réglages
- [ ] Les réglages d'un espace ne changent pas ceux d'un autre, ni ceux d'un autre membre d'un espace partagé
- [ ] Le Dashboard et les Réglages ne se masquent pas ; un identifiant d'outil inconnu est refusé
- [ ] Chaque entrée du registre apparaît dans la barre latérale, la barre de commandes et les réglages sans autre modification (test qui parcourt le registre)
- [ ] Sur une base existante, la migration ne masque rien
- [ ] L'API travaille sur un outil masqué (créer et lister des notes, Notes masqué) ; `GET` et `PUT /api/v1/spaces/{slug}/tools` lisent et changent le réglage
- [ ] STATUS et README à jour

PR 2 :

- [ ] Un compte non autorisé, ou sans session, reçoit 404 sur `/1life`, `/1life/cr`, `/1life/audit` et `/1life/clients` (et avec l'espace dans l'adresse) ; 1Life n'est ni dans ses réglages, ni dans sa barre de commandes, ni dans l'API
- [ ] `dashu tools grant <email> 1life` puis `dashu tools revoke` donnent puis retirent l'accès ; `dashu tools list` les montre ; un compte ou un outil inconnu est refusé
- [ ] Un compte autorisé voit 1Life dans ses réglages, masqué par défaut ; affiché dans un espace, il apparaît dans la barre latérale de cet espace seulement, et ses quatre pages s'ouvrent
- [ ] Aucune route ne donne l'accès ; l'API refuse de régler 1Life pour un compte non autorisé comme un outil inconnu
- [ ] STATUS et README à jour

## Décisions

Prises le 25 novembre 2026, par défaut, pour avancer :

- **La palette `Ctrl+K` n'existe plus** depuis le 13 novembre 2026 : « absent de Ctrl+K » veut dire absent de la barre de commandes `/` (`/go`).
- **Réglage par compte et par espace**, plutôt que par espace seulement : c'est un réglage d'affichage, et dans un espace partagé chacun garde le sien.
- **Seule l'adresse de l'outil est gardée** (`/notes`, `/files`, `/calendar`, `/tools`) : une note ou un fichier ouvert par lien, par `/open` ou depuis Perso s'ouvre toujours. Pour 1Life, toutes ses adresses.
- **Les widgets du dashboard ne suivent pas** le réglage : ils ont déjà leur propre disposition par espace.
- **L'onglet Outils garde son adresse sans espace** (`/tools`) : il est masqué selon l'espace actif (celui de l'onglet ou le dernier utilisé).
- **Le réglage passe par l'API** (route, outil MCP, commande d'Hermès), comme la disposition du dashboard : « L'API couvre tout le site ». L'autorisation des outils réservés, elle, reste hors de l'API, comme l'administration du serveur.
- **La PR 2 part de la branche de la PR 1** : elle a besoin du registre. Elle sera rebasée sur `v1` une fois la PR 1 fusionnée.

---

# dashu — Parcours et espaces

Nov 26, 2026 · @Rémi Moreau

## Contexte

Demandé par Rémi le 26 novembre 2026 : analyser le fonctionnement des espaces et la façon de voir les notes et les fichiers, puis corriger les erreurs de logique, les endroits où l'on se perd et les redirections qui n'ont pas de sens. Depuis l'audit du 13 novembre (« Espaces et outils »), beaucoup de choses se sont ajoutées : l'espace dans l'adresse, les dossiers, les espaces partagés, la corbeille et les outils par espace. Chacune a apporté ses redirections, et elles se contredisent.

Le problème central : **l'en-tête `X-Espace` de l'onglet passe avant la session**. Il n'est cherché que parmi les espaces en service, et un échec retombe en silence sur Perso. À côté de ça, plusieurs actions redirigent sans dire vers quel espace aller. Résultat : on change d'espace et rien ne bouge, on écrit dans Perso sans le savoir, on revient au mauvais endroit.

## Constats et corrections

**Lot 1 : l'espace ne se perd plus.**

- Changer d'espace sur une page sans préfixe (`/tools`, un événement, 1Life) ne fait rien : la redirection garde l'en-tête de l'ancien espace. `POST /space` répond désormais, pour htmx, par un `HX-Location` qui porte le nouvel espace, comme la commande `/space`.
- Changer d'espace dans un dossier de Fichiers ramène à l'ancien espace, parce que le dossier lui appartient. Un numéro de dossier est retiré de la page de retour : on arrive à la racine du nouvel espace. `racine`, `temporaire` et la recherche restent.
- Passer sur un espace où l'outil ouvert est masqué affichait « Cet outil est masqué » à chaque balayage. On arrive maintenant sur l'accueil de l'espace, sans message. Un fichier de Temporaire ramène à l'onglet Outils.
- Créer un espace devait en faire l'espace actif, mais l'onglet restait sur l'ancien. Il passe sur le nouveau, comme après un archivage, une suppression ou un départ, où l'onglet passe sur un espace qui reste.
- Dans un espace archivé ouvert par son adresse, les fragments et les écritures tombaient sur Perso : une tâche ajoutée, la disposition du dashboard, une nouvelle note. L'en-tête de l'onglet et la session acceptent maintenant un espace archivé.
- Un onglet resté sur un espace disparu (supprimé, quitté, membre retiré) écrivait en silence dans Perso. Une écriture est refusée (409, retour à l'accueil, « Cet espace n'est plus accessible : rien n'a été enregistré. »). Une page se rabat sur le dernier espace utilisé. Ses redirections ne prennent plus le préfixe de l'espace disparu.
- Le bandeau d'un espace archivé proposait « Restaurer » aux membres, puis répondait 403. Il n'est plus proposé qu'au propriétaire, et il ramène sur la page de l'espace au lieu des Réglages.
- Un espace archivé ne se supprimait pas quand Perso était le seul espace en service (« Il faut garder au moins un espace. »). Supprimer un archivé ne retire rien au sélecteur : c'est permis.

**Lot 2 : on revient d'où l'on vient.**

- Le retour de l'éditeur mène à la page d'où l'on a ouvert la note : un dossier de Fichiers, le dashboard, une recherche ou Perso. C'est le paramètre `depuis`, qui existe déjà pour les fichiers. Une note supprimée ramène au même endroit.
- **Depuis Perso, une note, un fichier ou un événement d'un autre espace s'ouvre sur place**, avec la pastille de son espace. Le retour ramène à Perso. Hors de Perso, un élément s'ouvre toujours dans son espace.
- Une action faite pendant une recherche de Fichiers garde la recherche et le tag. Compresser revient à la page d'origine. La reconnexion revient à la page demandée. Refuser une invitation ramène à la carte des espaces.

**Lot 3 : la corbeille et les messages.**

- **La corbeille devient un dossier « Corbeille » à la racine de Fichiers**, à côté de Temporaire, filtré sur l'espace ouvert (tous les espaces depuis Perso ou la racine). `/settings/trash` y mène, et Réglages garde un lien. Elle montre l'origine de chaque élément (« École / Math ») et se filtre par type.
- Les confirmations disent « Mettre à la corbeille » et non « Supprimer définitivement ». Les messages de suppression portent un lien vers la corbeille. Un élément restauré dit où il est revenu (« à la racine d'École, son dossier n'existe plus »), avec un lien, et prévient si ses liens de partage ont été révoqués.
- **« Vider la corbeille » n'efface que ce que l'on a supprimé soi-même.** Dans un espace partagé, ce que les autres membres ont supprimé reste 30 jours. La confirmation le dit.
- Un titre n'est plus échappé deux fois (« R&amp;amp;D »).

**Lot 4 : Perso, Notes et Fichiers parlent la même langue.**

- Dans Perso, les widgets et les listes ne disent plus « dans cet espace » quand ils montrent tous les espaces. Les notes épinglées du widget portent leur pastille d'espace et tiennent dans le widget.
- Dans l'onglet Notes, le titre d'une section de dossier s'ouvre sur ce dossier (`/notes?dossier=N`, avec un fil d'Ariane et un lien vers Fichiers). Une ligne de note a un menu : Ranger, Épingler, Mettre à la corbeille.
- La recherche de Fichiers trouve aussi les notes du dossier. Le filtre par tag des notes se normalise comme celui des fichiers.
- Un widget ne mène plus à un outil masqué. Masquer un outil met aussi à jour la barre du téléphone. L'indicateur de conversions mène à l'onglet Outils, qui les montre toutes.

**Lot 5 : ranger ne casse rien.**

- **Les images d'une note la suivent** quand on la déplace vers un autre espace (site, API, commande, dossier déplacé), sauf celles qu'une autre note de l'ancien espace affiche aussi. Le message le dit.
- Une conversion en cours suit son fichier déplacé. Un lien de partage devenu inutilisable après un déplacement n'est plus affiché comme valable, et le déplacement le dit.
- La fenêtre « Ranger » signale les espaces partagés et prévient quand on sort un élément d'un espace partagé.

## Décisions

Prises avec Rémi le 26 novembre 2026 :

- **Perso ouvre sur place** les éléments des autres espaces, au lieu de faire passer l'onglet dans leur espace.
- **La corbeille va dans Fichiers**, comme dossier de la racine.
- **Les images d'une note la suivent** quand on la déplace.
- **Vider la corbeille n'efface que ses propres suppressions.**

Par défaut, pour avancer :

- **Une écriture depuis un onglet sur un espace disparu est refusée** (409 et retour à l'accueil). Une page, elle, se rabat sur le dernier espace utilisé.
- **Un formulaire classique** (sans en-tête d'onglet) garde ses adresses sans préfixe, comme avant.
- **Un lot par PR**, dans l'ordre. Chaque lot a ses tests dans `tests/parcours.rs` ou dans le fichier de son module.

## Critères d'acceptation

Lot 1 :

- [ ] Depuis l'île, passer de École à Taff sur `/tools` ouvre `/tools` sur Taff ; sur `/notes?q=…`, ouvre `/e/taff/notes?q=…`
- [ ] Dans un dossier d'École, passer sur Taff mène à la racine des fichiers de Taff ; `racine`, `temporaire` et la recherche restent
- [ ] Vers un espace où l'outil est masqué : l'accueil de cet espace, sans message d'erreur ; un fichier de Temporaire ramène à l'onglet Outils
- [ ] Créer un espace y fait passer l'onglet ; un refus laisse l'onglet où il est
- [ ] Dans un espace archivé, une tâche ajoutée y reste et son widget la montre
- [ ] Une écriture depuis un onglet sur un espace disparu est refusée, sans rien écrire ; le message attend sur l'accueil
- [ ] Le bandeau d'un espace archivé ne propose « Restaurer » qu'au propriétaire, et ramène sur la page de l'espace
- [ ] Un espace archivé se supprime même quand Perso est le seul espace en service

Lots 2 à 5 : un critère par point ci-dessus, écrit avec son lot.

- [ ] Chaque point a ses tests d'intégration ; STATUS et README à jour
