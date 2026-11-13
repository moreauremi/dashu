# Consignes pour Claude Code

dashu est le tableau de bord perso de Rémi (tâches, notes Markdown, emploi du temps Pronote, trois espaces Perso, École et Taff). Un seul binaire Rust, en service sur son homelab. `SPEC.md` est le cahier des charges, `STATUS.md` l'état vérifié de chaque critère, `README.md` l'installation et l'exploitation. Les lire avant tout changement de fond.

**Nom affiché : Hermes Dashboard** (depuis le 12 novembre 2026, même identité visuelle que les autres projets de Rémi). dashu reste le nom de code : on ne le renomme pas dans le code, les commandes, les services ni les variables, seulement là où l'utilisateur le voit. Le nom passe par `assets::APP_NAME`, le logo par `assets::logo()` (`static/logo.svg`).

Le contexte privé (infra, adresses, historique, suite prévue) est dans `PASSATION.md`, qui n'est jamais versionné. S'il est présent à la racine, le lire en début de session.

## Façon de travailler

- **Langue** : tout en français (code commenté, docs, messages de commit, PR). Même ton que le README : phrases courtes, un terme technique expliqué la première fois.
- **Branches** : `v1` porte l'application et reçoit toutes les PR ; `main` ne contient que la page d'accueil `website/` (GitHub Pages, dashu.dev). Une évolution = une branche partie de `origin/v1` = une PR vers `v1`.
- **Avant de pousser** : `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. La CI (`.github/workflows/ci.yml`) refait tout cela, puis construit l'image Docker (moins de 30 Mo exigés). On ne fusionne qu'avec une CI verte.
- **Après la fusion** : Rémi lance `dashu-update` dans le conteneur (ou demande à Hermès « mets à jour dashu »). Rien ne se déploie tout seul.
- **Chaque comportement nouveau a son test d'intégration** dans `tests/` (aide : `tests/common/mod.rs`, `TestApp`, `logged_in`, `between`).
- **Tenir les docs à jour** : `STATUS.md` quand un critère, une mesure ou un écart change ; `README.md` quand l'installation, l'exploitation ou une variable change.
- **Secrets** : ne jamais demander ni afficher un secret (jeton du tunnel Cloudflare, secret Google, `SESSION_SECRET`, `API_TOKEN`). Rémi les tape lui-même sur le serveur. Ne jamais versionner `.env` ni `PASSATION.md`.
- **Choix** : quand il y a une vraie décision, proposer 2 ou 3 options, dire ce que chacune change, signaler la moins chère. Être franc si une demande a un défaut.

## Pièges connus (ils ont déjà coûté du temps)

- **CSP stricte** (`src/web/headers.rs`) : aucun script ni style en ligne, pas d'attribut `style=` dans les templates. Un style calculé passe par une feuille construite en JavaScript (`new CSSStyleSheet()` + `document.adoptedStyleSheets`, voir la barre de progression du balayage dans `static/js/app.js`).
- **htmx** (`templates/base.html`) : `attributesToSettle` exclut `style`, sinon htmx recopie des styles en ligne et la CSP les bloque. La navigation remplace le contenu de `body` avec `transition:true` ; la règle `.htmx-added` est volontairement limitée, sinon la page clignote en noir.
- **Écouteurs JS** : la page est remplacée à chaque navigation. Les événements comme `wheel` restent accrochés à l'ancien élément au trackpad : on rattache l'écouteur à chaque nouveau `#app` (`bindWheel` dans `onPage`). Tout ce qui doit survivre à une navigation se rebranche dans `onPage`.
- **Fichiers statiques** : `build.rs` les embarque avec une empreinte dans le nom. Dans un template, toujours `{{ crate::assets::url("chemin") }}`. Les chemins `/static/…` écrits dans le CSS et le manifeste sont réécrits automatiquement. Un nouveau type de fichier demande son entrée dans `fn mime` de `build.rs`.
- **Requêtes SQL** : `sqlx::query!` est vérifié à la compilation, hors ligne, via `.sqlx/`. Après une requête ou une migration modifiée, régénérer avec la commande du README (« Modifier une requête SQL ») et versionner `.sqlx/`. Une migration ne se modifie jamais une fois fusionnée : on en ajoute une.
- **Dates** : stockées en secondes Unix UTC, affichées en Europe/Paris par `src/time.rs` (heure d'été comprise). Ne pas calculer de fuseau ailleurs.
- **Notes** : Markdown → HTML par pulldown-cmark, puis nettoyé par ammonia. Ne jamais afficher du contenu utilisateur sans ce nettoyage ou l'échappement d'askama.
- **Formulaires** : tout `POST` porte le jeton CSRF ; l'API `/api/v1` passe par le jeton `Bearer` (comparaison à temps constant).
- **Logs** : couleurs seulement dans un terminal. Sous systemd, journald classerait sinon les lignes en « blob data ».
- **Bac à sable systemd** (`deploy/lxc/hardening.conf`), absent en local et dans `cargo test` : `RestrictSUIDSGID=yes` refuse tout `chmod` qui pose le bit setgid (EPERM), et chaque `ReadWritePaths=` est un point de montage à part, donc `rename` de `/srv/dashu/tmp` vers `/srv/dashu/fichiers` échoue (EXDEV). La compression en est tombée en panne le 13 novembre 2026. Pour passer d'un dossier à l'autre : `move_file` (`src/tools/mod.rs`). La CI rejoue ce bac à sable (étape « Compression dans le bac à sable du serveur »).
- **Dépendances front** : rien en dehors de htmx et de l'éditeur (ProseMirror, markdown-it) sans justification écrite dans le README (règle de `SPEC.md`). Elles sont produites par `npm run build` et versionnées dans `static/`. Après une modification de `frontend/`, relancer `npm run build` et versionner `static/js/editor.js`.
- **Éditeur de notes** (`frontend/editor/`) : le Markdown reste le format stocké. Un bloc nouveau a besoin des deux sens, lecture (`parser` de `format.js`) et écriture (`nodes`), sinon il se perd à l'enregistrement. ProseMirror retient Échap : l'éditeur rend lui-même le focus (voir `editorKeys`).
- **Téléphone** : l'application s'installe sur l'écran d'accueil de l'iPhone (`static/manifest.webmanifest`). En mode app, le zoom est bloqué (`touch-action` dans le CSS, viewport et gestes dans `app.js`) ; dans Safari il reste permis. Les champs de saisie font 16 px au moins sur écran tactile, sinon iOS zoome au focus.

## Le serveur, vu du code

- Conteneur LXC Debian sur Proxmox, binaire natif sous systemd. Pas de Docker en production (Docker reste une variante construite par la CI).
- `deploy/lxc/` contient tout ce qui tourne sur le serveur. **`dashu-update` réinstalle ces scripts à chaque mise à jour** : une modification ici part en production à la mise à jour suivante.
- `deploy/lxc/dashu-admin` est la commande forcée de la clé SSH d'Hermès, exécutée en root. Toute modification est sensible : garder exactement six actions (`status`, `logs [N]`, `restart`, `update`, `update-status`, `backup`), sans paramètre libre. Pas de restauration par Hermès.
- Le service est limité à 256 Mo de mémoire, en lecture seule sauf `/var/lib/dashu`, sans capacité Linux (`deploy/lxc/hardening.conf`). Un outil lourd (ffmpeg, LibreOffice…) doit tourner dans un service à part, pas dans dashu.
- `BASE_URL=https://app.dashu.dev` : la connexion Google ne marche que depuis cette adresse ; l'adresse Tailscale sert au mot de passe.
- Sauvegarde chaque nuit à 3 h 30 (heure du conteneur, UTC) ; `dashu-restore` pour restaurer. Les sauvegardes sont encore sur le même disque que la base (écart noté dans `STATUS.md`).

## La suite : v2

Décidée avec Rémi le 11 novembre 2026, détaillée dans `PASSATION.md`. À écrire d'abord dans `SPEC.md` (périmètre, sécurité, critères d'acceptation), puis à faire par PR.

1. Monter le second disque du serveur (465 Go) dans le conteneur, pour les fichiers et les sauvegardes ; sauvegarder aussi les fichiers ; refaire un exercice de restauration.
2. **Fichiers, version courte** : fichiers par espace sans dossiers (tags et recherche comme les notes), aperçu, images collées et pièces jointes dans les notes, partage par lien temporaire, impossible à deviner et révocable. Fichiers envoyés servis en téléchargement ou isolés, jamais exécutés dans la page.
3. **Onglet Outils, conversions** : une zone de dépôt unique qui propose les formats possibles (images dont HEIC, PDF, Markdown et texte, audio et vidéo). Chaque conversion tourne isolée, avec limites de temps, de mémoire et de taille, sans réseau.
4. **Onglet Outils, liens YouTube, Instagram, TikTok vers MP4 ou MP3** (yt-dlp), en dernier : seuls ces trois sites sont acceptés, l'outil reste réservé au compte de Rémi, aucun partage par lien du résultat, et il n'apparaît pas dans le portfolio.
