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
| Notes Markdown | Créer, éditer, supprimer, tags, recherche plein texte, aperçu en direct | Partage public, collaboration temps réel |
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
| Éditeur de notes | CodeMirror 6 (JS, mode Markdown) | Seule brique JS significative, chargée uniquement sur la page éditeur |
| Styles | CSS écrit à la main, variables CSS | Design sur mesure, pas de framework CSS |

Règle : aucune dépendance front en dehors de htmx et CodeMirror sans justification écrite dans le README.

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
│   ├── search/             palette Ctrl+K, FTS5
│   └── web/                erreurs, CSRF, en-têtes de sécurité
├── templates/              askama : layout, pages, fragments htmx
├── static/                 css, js (htmx, CodeMirror), icônes, polices
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
- **Palette de commandes** `Ctrl+K` / `Cmd+K` : recherche dans les notes et accès rapide aux pages et actions (« nouvelle note », « changer d'espace »).
- Raccourcis clavier : `N` nouvelle note, `G` puis `D` / `N` / `C` pour aller au Dashboard / Notes / Calendrier.

### Dashboard

- En-tête : date du jour, heure, salutation.
- Widget **Prochains cours** : les 3 prochains événements de l'espace actif, avec salle et heure.
- Widget **Tâches du jour** : ajouter, cocher, supprimer, en place.
- Widget **Notes récentes** : 5 dernières notes modifiées, plus les notes épinglées.
- Chaque widget est un fragment HTMX chargé indépendamment : un widget lent ne bloque pas les autres.

### Notes

- Éditeur CodeMirror en Markdown, aperçu rendu côté serveur à côté (rafraîchi 300 ms après la dernière frappe).
- Sauvegarde automatique toutes les 2 secondes après une modification, indicateur « Enregistré ».
- Tags ajoutables depuis l'éditeur, filtre par tag dans la liste.
- Recherche plein texte avec extrait surligné du passage trouvé.
- Épingler, supprimer (avec confirmation), déplacer vers un autre espace.
- Support : titres, listes, cases à cocher, code avec coloration, tableaux, liens.

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
| Poids de la page éditeur (avec CodeMirror) | < 250 Ko |
| Temps de réponse serveur d'une page (hors réseau) | < 20 ms |
| Score Lighthouse Performance (mobile) | ≥ 95 |
| Mémoire du binaire au repos | < 50 Mo |
| Taille de l'image Docker | < 30 Mo |

Moyens imposés :

- Compilation en mode `release` avec LTO (optimisation à l'édition de liens).
- Fichiers statiques versionnés par empreinte dans le nom, cache navigateur d'un an.
- Compression brotli/gzip sur toutes les réponses texte.
- CodeMirror chargé uniquement sur la page éditeur.
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
| v2 — Fichiers | Cloud de fichiers par espace, partage par lien temporaire | Stockage disque + métadonnées SQLite, upload en streaming axum |
| v3 — Outils fichiers | Convertisseur d'images, outils PDF (fusion, découpe, compression), OCR | crate `image`, `lopdf` ; pandoc / LibreOffice / Tesseract appelés en sous-processus pour ce que Rust ne couvre pas |
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
