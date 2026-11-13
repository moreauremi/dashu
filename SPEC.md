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
4. **Paquets** : ffmpeg, pandoc, poppler-utils, qpdf, ghostscript, libheif-examples ; Typst et yt-dlp en binaires officiels (trop anciens ou absents dans Debian). Pas de LibreOffice. `deploy/lxc/install.sh` et le README sont mis à jour.

## Fichiers

### Modèle de données

| Table | Colonnes principales | Remarques |
| --- | --- | --- |
| `files` | id, user\_id, space\_id, name, mime, size, sha256, storage\_key, created\_at, updated\_at | `storage_key` : 32 octets aléatoires, seul nom sur le disque |
| `files_fts` | table virtuelle FTS5 sur name | Tenue à jour par des triggers, comme les notes |
| `file_tags` | file\_id, tag\_id | Mêmes tags que les notes |
| `share_links` | id, file\_id, token\_hash, expires\_at, revoked\_at, created\_at | Le lien en clair n'est jamais stocké |

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
| PDF | Lecteur du navigateur. Si l'en-tête `sandbox` l'empêche (cas connu de Chrome), le PDF est servi depuis un sous-domaine séparé ou proposé en téléchargement : à trancher dans la PR de l'aperçu |
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
- Instagram et TikTok demandent souvent un cookie de session. C'est un secret : fichier dans `/etc/dashu/`, lisible par ce service seul, jamais affiché.
- **Cadre** : les conditions d'utilisation des trois plateformes interdisent le téléchargement. L'outil est donc réservé au compte de Rémi (`DOWNLOADER_EMAIL`), un résultat ne se partage jamais par lien, et l'outil ne figure pas dans le portfolio.

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

Reste à trancher, dans la PR concernée :

- **Taille du volume de sauvegarde des fichiers** sur le premier disque (étape 6) : 50 Go au plus.
- *(tranché le 11 novembre)* **Aperçu des PDF** : la première page, rendue en image par le service de conversion, et le bouton Télécharger. Le PDF n'est jamais servi pour être affiché par le navigateur.

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
- **G avec les outils déjà installés** (ffmpeg, Ghostscript, libheif, crate `image`). Markdown et texte vers PDF, HTML ou DOCX demandent pandoc et Typst : ils restent à l'étape 8 de la v2.
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

**Un seul sélecteur pour ranger.** La même fenêtre sert à déplacer un fichier (liste et aperçu), à garder un fichier de Temporaire ou de l'onglet Outils, et à déplacer une note (bouton de l'en-tête de l'éditeur). La barre de commandes garde `/move <espace>` et `/keep <espace>`, qui rangent à la racine de l'espace.

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
- **Hors de ce lot** : glisser-déposer sur un dossier, déplacer un dossier entier, créer une note depuis Fichiers.
