# État de la v1

Mis à jour le 10 novembre 2026, branche `v1`.

**En bref.** Tout le périmètre v1 de [SPEC.md](SPEC.md) est codé et vérifié en local : 73 tests automatiques verts, clippy sans avertissement, parcours refaits à la main dans un navigateur. **Rien n'est encore déployé.** Les critères qui exigent le serveur réel, un vrai compte Google ou Docker restent ouverts : ils sont listés plus bas, avec la marche à suivre.

Une case cochée veut dire « vérifié ici, en local, de la façon indiquée ». La spec demande que tout soit coché *sur le serveur réel* : aucune case ne l'est à ce sens-là tant que dashu n'est pas en ligne.

## Critères d'acceptation

### Fonctionnel

- [ ] **Connexion par identifiants et par Google sur le même compte**
  - Identifiants : vérifié (tests `tests/auth.rs`, et à la main dans le navigateur).
  - Google : codé (OpenID Connect, PKCE, `state`, `nonce`, rattachement par email vérifié) mais **jamais exécuté pour de vrai**, faute d'identifiants Google en local. À tester dès que `GOOGLE_CLIENT_ID` et `GOOGLE_CLIENT_SECRET` existent.
- [ ] **Un email absent de `ALLOWED_EMAILS` est refusé, avec les deux méthodes**
  - Identifiants : vérifié (test `email_hors_liste_refuse_meme_avec_le_bon_mot_de_passe`). La commande `dashu set-password` refuse aussi un email hors liste.
  - Google : le refus est codé dans le retour de Google, non exécuté pour de vrai.
- [x] **6 tentatives de login ratées en une minute déclenchent un blocage temporaire** : test `sixieme_tentative_en_une_minute_bloquee` (5 refus, puis 429 ; le blocage est par adresse IP).
- [x] **Les 3 espaces existent à la première connexion et le changement d'espace filtre dashboard, notes et agenda** : tests `trois_espaces_des_la_premiere_connexion`, `le_changement_d_espace_filtre_le_contenu`, `vue_semaine` ; vu dans le navigateur.
- [x] **Une note créée, modifiée, taguée, retrouvée par la recherche, déplacée, puis supprimée** : test `cycle_de_vie_d_une_note` ; création, édition, tags, épinglage et recherche refaits dans le navigateur.
- [x] **Le flux iCal s'affiche correctement en vue semaine, heure d'été comprise** : vérifié avec `tests/fixtures/pronote.ics`, un fichier au format des exports Pronote (tests `heure_d_ete_et_heure_d_hiver`, `vue_semaine`, `vue_semaine_en_heure_d_ete`), importé par URL dans le navigateur. **Pas encore essayé avec le vrai flux Pronote Campus** : son URL est personnelle.
- [x] **La palette `Ctrl+K` trouve une note et ouvre une page** : vu dans le navigateur (recherche, sélection au clavier, ouverture avec Entrée, bascule sur l'espace de la note).
- [ ] **Tout fonctionne sur téléphone** : dashboard, menu, agenda et éditeur vérifiés en émulation 375 px (pas de débordement horizontal, menu en tiroir, onglets Écrire/Aperçu). **Pas essayé sur un vrai téléphone.**

### Technique

- [x] **`cargo clippy` sans avertissement, `cargo test` vert** : en local (`cargo clippy --all-targets -- -D warnings`, 73 tests). La CI (`.github/workflows/ci.yml`) est écrite mais **n'a jamais tourné** : rien n'est poussé sur GitHub.
- [x] **Tests d'intégration sur : login, isolation des données entre deux utilisateurs, CRUD des notes, parsing iCal avec un `.ics` d'exemple** : `tests/auth.rs` (16), `tests/isolation.rs` (4), `tests/notes.rs` (5), `tests/calendar.rs` (11).
- [ ] **Cibles de performance atteintes** : les mesures locales passent toutes (tableau ci-dessous). Les cibles de la spec se mesurent sur le homelab, via Cloudflare, en 4G : à refaire là-bas. La taille de l'image Docker n'a pas pu être mesurée (pas de Docker sur la machine de développement).
- [x] **Aucune injection de script possible via le contenu d'une note** : tests unitaires du nettoyage (`src/notes/markdown.rs`), test d'intégration `une_note_ne_peut_pas_injecter_de_script` sur toutes les pages où une note apparaît, et CSP sans `unsafe-inline` en seconde barrière. Essai à la main : une balise `<script>` tapée dans l'éditeur disparaît de l'aperçu.
- [ ] **Sauvegarde nocturne vérifiée par une restauration réelle** : `dashu backup` testé en local sur une base en service, puis un second serveur démarré sur la copie (connexion et notes retrouvées). Le script `scripts/backup.sh`, la tâche cron et la restauration dans le volume Docker restent à dérouler sur le serveur.
- [x] **README complet** : installation locale, variables d'environnement, déploiement, création des identifiants Google, sauvegardes.

## Ce qu'il te reste à faire

Dans l'ordre :

1. **Relire et pousser la branche `v1`.** La CI se lance au premier push : elle construit l'image Docker, vérifie qu'elle fait moins de 30 Mo et qu'elle démarre.
2. **Décider du sort de la page d'accueil.** `dashu.dev` pointe aujourd'hui sur GitHub Pages (`website/`). La faire pointer sur le tunnel la remplace par l'application.
3. **Créer les identifiants Google** (README, section dédiée) et tester la connexion Google avec un email autorisé, puis avec un email qui ne l'est pas.
4. **Déployer** en suivant le README : tunnel Cloudflare, LXC, `docker compose up -d --build`.
5. **Brancher le vrai flux Pronote** dans Réglages et comparer une semaine avec Pronote.
6. **Tester sur ton téléphone.**
7. **Planifier `scripts/backup.sh`**, attendre une nuit, restaurer la sauvegarde une fois.
8. **Refaire les mesures de performance** sur le serveur (Lighthouse mobile, `docker stats`, `docker images`).

## Mesures de performance (en local)

Binaire `release` (LTO) sous Windows 11, base de test, 200 requêtes par page. Ce ne sont pas les conditions de la spec (homelab, Cloudflare, 4G) : elles donnent un ordre de grandeur.

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

- **Le dépôt n'était pas vide.** Il contenait la page d'accueil (`website/`) et son workflow GitHub Pages : ils sont laissés tels quels. Le README, qui décrivait un autre projet (équipes, domotique, connecteurs), a été remplacé.
- **Pas de page d'inscription.** Un compte naît à la première connexion Google ou avec `dashu set-password`. Raison dans le README : sans vérification d'email, un formulaire public permettrait de créer le compte d'une adresse autorisée à la place de son propriétaire.
- **Un commit pour toute l'application**, puis un par lot (tests, déploiement et documentation), au lieu d'un commit par étape : les sept étapes ont été écrites d'un seul tenant avant la première compilation.
- **Versions** : sqlx 0.8 et reqwest 0.12 plutôt que les dernières, pour rester compatibles avec `tower-sessions-sqlx-store` et `openidconnect`. Une crate hors liste, `syntect`, colore le code des notes côté serveur.
- **Deux colonnes ajoutées au modèle** pour des comportements demandés : `tasks.done_at` (une tâche cochée reste visible jusqu'à minuit), `calendar_sources.last_error` et `last_attempt_at` (afficher l'échec de synchro), plus `users.theme` et `calendar_events.all_day`.
- **Récurrences iCal (`RRULE`) non développées** : Pronote n'en produit pas.

Les autres limites connues sont dans le README.

## Pour la grille d'évaluation

- **Interventions** : une seule question posée (comment compiler, la machine n'ayant ni Rust, ni compilateur C, ni Docker). Rust et GCC ont été installés avec winget.
- **Bugs trouvés pendant le développement** par les tests et les essais, tous corrigés : un `<input type="password">` survivait au nettoyage d'une note ; une adresse comme `/notes/abc` répondait 400 au lieu d'une page 404 ; les couleurs des matières se répartissaient mal (quatre matières sur neuf de la même teinte) ; un retour arrière sur `/notes/new` aurait recréé une note.
- **Prix** : le nombre de jetons consommés n'est pas lisible depuis la session elle-même. Il se lit dans l'écran d'usage de l'application.
