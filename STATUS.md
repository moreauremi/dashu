# État de la v1

Mis à jour le 24 novembre 2026, branche `v1`. Version en service sur le serveur : 93d540a (PR #79), relevée le 24 novembre 2026 par `dashu-admin status` : binaire téléchargé (mode `binaire`, canal `v1`). Tout ce qui est fusionné dans `v1` est donc en service ; « pas encore essayé » veut dire qu'il reste à s'en servir en vrai.

**En bref.** La v1 est terminée : **tous les critères d'acceptation sont vérifiés**, la plupart sur le serveur réel. dashu tourne sur le homelab, dans un conteneur LXC Debian du serveur Proxmox, joignable sur `https://app.dashu.dev` (tunnel Cloudflare) et sur le réseau Tailscale. Il sert au quotidien, sur ordinateur et sur iPhone (en app sur l'écran d'accueil). La CI GitHub tourne à chaque PR.

Une case cochée veut dire « vérifié », et la ligne dit où : sur le serveur réel, par les tests, ou les deux.

## Critères d'acceptation

### Fonctionnel

- [x] **Connexion par identifiants et par Google sur le même compte**
  - Identifiants : en service sur le serveur, et tests `tests/auth.rs`.
  - Google : vérifié sur le serveur le 11 novembre 2026. « Continuer avec Google » avec l'adresse du compte ouvre le même compte (mêmes notes). Client OAuth « Application Web » d'un projet Google dédié, en mode Test jusqu'au 17 novembre 2026, publié (en production) depuis.
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
  - `dashu-backup.timer` la lance chaque nuit à 3 h 30 (heure du conteneur, UTC). Copie vérifiée par `PRAGMA integrity_check`. Elle tourne seule : dernière sauvegarde `dashu-2026-11-23_033014.db`, relevée le 23 novembre 2026.
  - Cas d'échec testés hors du serveur avec le vrai binaire : fichier corrompu, base qui n'est pas dashu, base avec laquelle dashu ne démarre pas (retour automatique).
- [x] **README complet** : installation locale, variables d'environnement, déploiement tel qu'il tourne (LXC, systemd, Tailscale, tunnel Cloudflare), mise à jour, sauvegarde et restauration, identifiants Google. Les scripts du serveur sont versionnés dans `deploy/lxc/`.

## Ce qu'il reste

Rien pour la v1. À garder en tête :

- **Si `/usr/local/sbin` ou `/usr/local/bin` manque dans le `PATH`** du shell ouvert par `pct enter` : `echo 'export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/sbin:$PATH"' >> ~/.bashrc`. Sans `/usr/local/bin`, `command -v typst` répond « absent » alors que Typst est bien là (relevé le 23 novembre 2026) ; les services, eux, le trouvent.

Fait le 11 novembre : script de mesure injecté par Cloudflare désactivé (Web Analytics), client OAuth Google créé, `BASE_URL` passé à `https://app.dashu.dev`. Le soir, les sauvegardes sont passées sur le second disque, comme le demandait la spec (voir « v2 : avancement »).

## v2 : avancement

Le cahier des charges et ses critères sont dans `SPEC.md`, partie v2. Faits : l'étape 1 (prérequis), l'étape 2 (fichiers), l'envoi par morceaux, la compression et l'aperçu (début de l'étape 3), avancés à la demande de Rémi. Une case reste vide tant que le critère n'est pas vérifié sur le serveur réel.

- [ ] **Le second disque est monté dans le conteneur et le reste après un redémarrage de l'hôte**
  - Fait le 11 novembre 2026 : l'ancien contenu du disque de 465 Go est effacé, et le disque est déclaré comme stockage LVM-thin `ssd2`. Deux volumes y sont pris : `mp0` de 45 Go sur `/srv/dashu`, `mp1` de 5 Go sur `/var/backups/dashu`.
  - Vérifié sur le serveur après un redémarrage du conteneur : les deux volumes sont montés, et dashu, cloudflared et Tailscale répondent.
  - Relevé le 23 novembre 2026 (`findmnt -T`) : `/srv/dashu` sur `ssd2` (44 Go, 41,7 Go libres), `/var/backups/dashu` sur `ssd2` (4,8 Go, 4,6 Go libres).
  - Reste à vérifier au prochain redémarrage de l'hôte.
- [x] **La base est sauvegardée sur le second disque, les fichiers sur le premier**
  - La base : vérifié sur le serveur. Les 12 sauvegardes existantes ont été recopiées sur le volume, avec les mêmes empreintes SHA-256. Une sauvegarde lancée par `dashu-backup` y a été écrite.
  - Les fichiers (étape 6, écrite le 15 novembre 2026) : chaque nuit après la base, `dashu-backup-files` fait un instantané incrémental (liens durs, 14 jours) sur un volume du premier disque, `mp2` sur `/var/backups/dashu-fichiers` ; `dashu-restore` remet les fichiers absents avec la base de la même nuit. Vérifié par un test du script dans la CI (`tests/deploy/sauvegarde-fichiers.sh` : lien dur pour un fichier inchangé, envois en cours exclus, purge, restauration d'un fichier supprimé).
  - En service, relevé sur le serveur le 23 novembre 2026 : `mp2` est sur le premier disque (`pve`, 48,9 Go, 46,3 Go libres), `rsync` est installé, et l'instantané de la nuit (`fichiers-2026-11-23_033014`) a été fait avec la base de la même heure.
  - Reste l'exercice de restauration d'un fichier (README), critère « Restauration réelle » de la spec.
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

**Suite de la v2, demandée le 15 novembre 2026** (« fait tout » : la suite annoncée sur le portfolio). Écrite, vérifiée par les tests et en service sur le serveur (version 61c8738) ; sauf mention, pas encore essayée en vrai.

- [ ] **Tags et recherche dans les fichiers** (étape 3) : les mêmes tags que les notes, posés depuis l'aperçu d'un fichier, montrés dans la liste, avec un filtre par tag (nombre de fichiers par tag) ; recherche sur le nom (FTS5, accents ignorés, début de mot) dans l'endroit montré et tout ce qu'il contient (à la racine : tous les espaces), chaque résultat avec son dossier. Le nettoyage des tags inutilisés tient compte des fichiers (il ne regardait que les notes). La barre de commandes trouvait déjà un fichier par son nom (`/open`). Migration 0014, tests `tests/fichiers_tags.rs` (4). Vu dans Chrome (instance locale) : tag posé depuis l'aperçu, recherche pendant la frappe, résultat avec son dossier.
- [ ] **Partage par lien temporaire** (étape 4) : depuis l'aperçu d'un fichier rangé, un lien de 1 heure, 1 jour ou 7 jours (32 octets aléatoires, seule l'empreinte SHA-256 en base, lien entier affiché une seule fois, bouton « Copier »), révocable ; la liste marque les fichiers partagés. Page publique `/s/<jeton>` sans connexion : nom, taille, téléchargement (`attachment`, CSP `sandbox`, `nosniff`, `noindex`) ; expiré, révoqué ou inconnu : la même page 404 ; limitée à 20 requêtes d'un coup puis une toutes les 3 s par adresse, avec un compteur à part de celui de la connexion. Les liens finis depuis 30 jours sont effacés. Migration 0015, tests `tests/partage.rs` (4). Vu dans Chrome : lien créé, ouvert dans une session vierge, téléchargé.
- [ ] **Conversion des documents** (étape 8, en partie) : Markdown et texte vers PDF, HTML ou DOCX, dans le menu « Convertir » ; pandoc en `--sandbox` dans le service de conversion isolé, ses fichiers de travail (et ceux de Typst) dans le dossier de la tâche ; le résultat arrive à côté de l'original. Tests : arguments de pandoc (`arguments_de_pandoc`), menu et circuit complet avec le faux service (`tests/documents.rs`). Le service de conversion ne voit pas non plus les sauvegardes des fichiers.
  - Premier essai sur le serveur (15 novembre 2026) : un `.md` en PDF échoue (« fichier illisible ou abîmé »), le HTML et le DOCX non. Cause : pour un PDF, pandoc 3.1 (celui de Debian 13) écrit le source Typst dans le dossier courant, sans regarder `TMPDIR`, et sous systemd ce dossier est `/`, en lecture seule. Corrigé : pandoc tourne dans le dossier de la tâche (`pandoc_tourne_dans_le_dossier_de_la_tache`). Au passage, une image du Markdown faisait aussi échouer le PDF (Typst ne lit que des fichiers présents) : elle devient son texte de remplacement.
  - La CI fait maintenant la vraie conversion, PDF, HTML et DOCX, avec pandoc 3.1.11.1 et Typst 0.11.1, dans le bac à sable du service (étape « Documents dans le bac à sable du service de conversion », test `documents_dans_le_bac_a_sable`). Vérifié sur le serveur le 15 novembre 2026 : le `.md` sort en PDF.
  - Mise en page du PDF façon Notion (demandée le 15 novembre 2026, celle de pandoc faisait daté) : notre gabarit Typst (`src/tools/pdf/gabarit.typst`), A4, police Inter (`fonts-inter`), texte en français aligné à gauche, titres en demi-gras, code et tableaux sur fond gris clair, vraies cases à cocher (`src/tools/pdf/images.lua`). La CI garde le PDF de l'exemple `tests/fixtures/document.md` (artefact `document-pdf`), regardé page par page. Sur le serveur (relevé le 23 novembre 2026) : `fonts-inter` installé, Typst dans `/usr/local/bin` depuis le 15 novembre. Reste : un essai.
- [ ] **Renommer un fichier** (demandé le 15 novembre 2026) : depuis son aperçu ou sa ligne dans la liste ; on change la partie avant l'extension, l'extension (et donc le type, tiré d'elle) ne change jamais, une extension retapée n'est pas doublée ; le nom est nettoyé comme à l'envoi. La recherche et le téléchargement suivent le nouveau nom. Tests : `renommer_garde_l_extension` (unitaire), `tests/renommer.rs`.
- [ ] **Menu « … » sur chaque ligne de l'onglet Fichiers** (demandé le 15 novembre 2026) : à la place des icônes à droite d'un dossier, d'une note ou d'un fichier, un seul bouton « … » ; son menu présente chaque action en icône et texte (Télécharger, Renommer, Ranger ailleurs, Convertir…, Supprimer en rouge après un trait). Un choix qui demande une saisie ouvre sa fenêtre à la place du menu, sous le même bouton ; le nom à renommer est déjà sélectionné. Test `tests/menu_actions.rs` ; vu dans Chrome sur ordinateur et téléphone (instance locale).
- [ ] **Retour depuis la page d'un fichier** (demandé le 15 novembre 2026) : le lien de retour ramenait toujours au dossier du fichier. Il ramène maintenant à la page d'où on l'a ouvert (app.js lui passe l'adresse quittée, espace compris, paramètre `depuis`), avec son nom : « École / Réseaux · « vlan » » pour une recherche, « Fichiers » pour la racine de Perso, « Dashboard », le titre d'une note. L'origine survit à renommer, convertir, ranger ; supprimer y ramène. Une adresse d'un autre site, trop longue ou d'un dossier disparu est ignorée (retour au dossier, comme avant). Par ce lien comme par le retour du navigateur, la page retrouve son défilement (noté par onglet, sessionStorage). Tests `tests/retour.rs` ; vu dans Chrome sur l'instance locale (dossier qui défile, recherche, racine de Perso, dashboard, retour du navigateur).
- [ ] **Ajouter un compte sans ouvrir la configuration** (demandé le 16 novembre 2026) : `dashu-compte ajouter | retirer | liste | mot-de-passe` (`deploy/lxc/dashu-compte`), en root dans le conteneur. Il réécrit la ligne `ALLOWED_EMAILS` de `/etc/dashu/dashu.env` (reste du fichier, droits et propriétaire inchangés), redémarre dashu et remet l'ancien fichier s'il ne répond plus. Refuse une adresse mal formée, le retrait de `API_USER_EMAIL` (dashu ne démarrerait plus) et celui de la dernière adresse. Hors de `dashu-admin` : Hermès n'y a pas accès. Test `tests/deploy/comptes.sh`, rejoué par la CI. Installé sur le serveur (`/usr/local/sbin/dashu-compte`, relevé le 23 novembre 2026). Reste : l'essayer.
- [ ] **Télécharger une note en Markdown ou en PDF** (demandé le 17 novembre 2026) : bouton « Télécharger » dans l'éditeur, à côté de « Ranger ». Markdown servi par dashu (le titre en `# Titre`, puis le texte enregistré) ; PDF par le service de conversion, même gabarit que « Convertir › En PDF », titre et date en tête, envoyé dès qu'il est prêt sans rien ranger dans l'espace (tâche sans ligne dans `jobs`, numéro de dossier au-dessus de `EXPORT_BASE`, prise en premier par le service). Le nom du fichier est le titre de la note. Les modifications pas encore enregistrées partent avant le téléchargement (app.js). Service arrêté : page 503 au bout de 30 s. Tests `tests/notes_telechargement.rs` (6, avec un faux service), unitaires `attente_d_un_pdf`, `pdf_d_une_note_en_premier` et ceux de `src/notes/export.rs` (titre lu tel quel par pandoc). Vu dans le navigateur sur une instance locale, avec un faux service (pandoc et Typst absents du Mac) : menu, message « PDF en préparation », modification envoyée avant le téléchargement, page d'erreur sans service. Reste : un vrai PDF sur le serveur, et l'essai sur l'iPhone en app.
  - Options du PDF (demandées le 17 novembre 2026) : taille du texte (80 à 150 %, toute autre valeur refusée) et « Garder les titres avec leur texte », cochée d'office : un titre qui serait coupé de son paragraphe en bas d'une page passe en haut de la suivante, avec lui (bloc plus long qu'une page : le titre ne part que s'il n'a pas trois lignes sous lui). Fait dans le gabarit Typst (`titre-garde`) et le filtre `images.lua` ; une première version qui lisait la position sur la page ne se stabilisait pas (« layout did not converge »), d'où des mesures seules. Vérifié avec pandoc 3.1.11.1 et Typst 0.11.1 (ceux du serveur) en local : positions des titres relevées avec et sans l'option (identiques quand le titre a la place, en haut de page sinon), pages regardées, zoom de 80 à 150 % sans avertissement, liste de cases à cocher et tableau sous un titre, zoom farfelu venu de la note écarté. Puis vu dans le navigateur avec le vrai service de conversion : menu « PDF… », focus sur la taille, largeur téléphone, PDF à 125 % en 2,4 s. Tests `options_du_pdf`, `le_filtre_connait_chaque_zoom`, `une_taille_de_texte_inconnue_est_refusee`, et la CI convertit la fiche d'exemple avec les deux options (`note.pdf` dans l'artefact `document-pdf`).
  - Au passage (signalé par Rémi le 17 novembre 2026) : la flèche des listes déroulantes, dessinée par le navigateur, collait au bord droit. Toutes les listes de l'app ont maintenant le chevron des icônes, à 12 px du bord, gris dans les deux thèmes (thème forcé compris). Vu dans le navigateur, clair et sombre.
- [ ] **Lire un PDF en entier dans dashu** (demandé le 18 novembre 2026 ; seule la première page s'affichait, en image) : la page d'un PDF montre toutes ses pages l'une sous l'autre, dessinées par pdf.js (Mozilla, version « legacy », choix noté dans `SPEC.md` et justifié dans le README). Barre collée sous l'île (sous la barre du haut sur téléphone) : page en cours, zoom de 50 à 400 % et « ajuster à la largeur » (zoom d'ouverture, 125 % au plus), qui suit la largeur de la fenêtre. Texte sélectionnable. Seules les pages à moins d'un écran et demi sont dessinées, les autres rendent leur mémoire. Le PDF arrive par `/files/{id}/raw` comme une donnée (`application/octet-stream`, `attachment`, CSP `sandbox`, demandes partielles) : le navigateur ne l'affiche jamais lui-même. pdf.js (1,8 Mo, environ 0,55 Mo compressé) n'est chargé qu'à l'ouverture d'un PDF, une fois, et le lecteur se ferme quand on quitte la page (navigation htmx comprise). Tests `un_pdf_se_lit_dans_la_page` et `une_photo_heic_attend_son_apercu` (`tests/preview.rs`). Vu dans le navigateur intégré (Chromium) sur une instance locale : un PDF de 16 pages (texte sélectionné exactement sur le dessin, compteur, pages lointaines effacées), un PDF aux pages de tailles différentes (paysage, page tournée de 90°, A5), zoom et « ajuster » en largeur ordinateur et téléphone, aller-retour par la navigation de dashu, thèmes clair et sombre, aucune erreur ni violation de CSP en console. Reste : l'essai sur l'iPhone en app (Safari pas essayé ici) et avec un vrai PDF scanné.
- [x] **Téléchargeur de liens** (étape 9, demandé le 25 novembre 2026) : un lien YouTube, Instagram ou TikTok devient un MP4 ou un MP3, en haut de l'onglet Outils, pour le seul compte `DOWNLOADER_EMAIL` (404 pour les autres ; ni API ni connecteur). Lien lu et réécrit (hôte exact, `https`, sans identifiant ni port), revérifié par le service ; résultat dans Temporaire, nommé d'après le titre, marqué `files.from_link` : ni lui ni ses conversions ne se partagent par lien (site et API). Service `dashu-download` : Internet mais pas la machine, le réseau de la maison ni Tailscale (`IPAddressDeny`), DNS publics, un téléchargement à la fois, 30 minutes, trois en attente au plus. Il vérifie lui-même son pare-feu (un envoi UDP vers `127.0.0.1`, `192.168.0.1` et `100.100.100.100` doit être refusé sur-le-champ ; en TCP, le filtre jette le paquet sans refuser la connexion, ce que la CI a montré au premier essai) et ne télécharge rien sans lui ; l'onglet Outils dit s'il est arrêté ou sans pare-feu. yt-dlp et Deno (moteur JavaScript que YouTube impose à yt-dlp) en binaires officiels vérifiés par SHA-256, yt-dlp remis à jour chaque semaine (`dashu-telechargeur-maj.timer`). Activé par `dashu-telechargeur activer <email>`, pas par l'installateur. Migration 0022. Tests `tests/telechargeur.rs` (5, dont un lien symbolique posé à la place du résultat, jamais rangé : Linux seulement) et tests unitaires de `src/tools/telechargeur.rs` (liens acceptés et refusés, arguments de yt-dlp, progression, messages d'erreur, noms). La CI rejoue la vérification du pare-feu avec et sans le filtre de l'unité. Vu dans le navigateur, sur une instance locale avec un service simulé : avertissement quand le service ne tourne pas, lien d'un autre site refusé, lien YouTube accepté et suivi (« Son depuis un lien », en attente), puis fichier rangé dans Temporaire sous le titre de la vidéo (« / » remplacé) ; largeur ordinateur et téléphone (champ en 16 px, pas de débordement), thèmes clair et sombre. Essayé sur le serveur le 25 novembre 2026 : `dashu-telechargeur activer` installe yt-dlp 2026.11.23 et Deno 2.9.8, le service démarre, mais le conteneur LXC non privilégié n'applique pas `IPAddressDeny`, et le service l'a détecté (« sans pare-feu ») : il refuse tout. D'où une seconde couche, des règles nftables pour le seul compte `dashu-download` (`deploy/lxc/dashu-telechargeur.nft`, chargées par `dashu-download-parefeu.service`, sans lesquelles le téléchargeur ne démarre pas), que la CI charge pour de vrai : la vérification passe pour ce compte, pas pour un autre. L'avertissement « pas dans ALLOWED_EMAILS » du script était faux ; il lit maintenant la liste par `dashu-compte liste`. Sur le serveur, les règles nftables se chargent, et `etat` dit « Pare-feu : actif ». Premier vrai téléchargement : le MP3 arrive ; le MP4 (159 Mo) aussi, mais dashu a été tué par le noyau (« oom-kill », limite de 256 Mo) en le rangeant, d'où une erreur 502 de Cloudflare pendant 3 secondes. Cause : deux copies d'un coup (vers les fichiers, puis vers le dossier de sa vignette), dont les pages en attente d'écriture comptent dans la mémoire de dashu. Corrigé : les copies se font par morceaux de 1 Mo, avec une écriture forcée sur le disque tous les 32 Mo (`copy_file`, test `copie_par_morceaux`), comme les envois, qui forçaient déjà l'écriture après chaque morceau ; l'unité gagne une limite douce, `MemoryHigh=192M`, que `dashu-update` ne pose pas (il ne réinstalle pas les `.service`) : sur le serveur, `systemctl set-property dashu MemoryHigh=192M`, une fois. Vérifié sur le serveur le 25 novembre 2026, après la mise à jour (`27273c2`) et cette commande : un MP4 de 151 Mo est rangé dans Temporaire avec sa vignette, et dashu n'a pas été arrêté (`memory.events` : `max 0`, `oom_kill 0` ; rien dans le journal). Mémoire au plus haut : 193 Mo, la limite douce. Ce sont les pages du fichier copié, déjà écrites sur le disque (`memory.stat` : `anon` 4,9 Mo, `file` 181 Mo, dont 76 Ko en attente d'écriture), que le noyau reprend en arrivant à 192 Mo (`high` 377 fois). Le service `dashu-download` est monté à 496 Mo pour cette vidéo, sous sa limite de 1 Go : à surveiller avec une vidéo plus grosse. Écart connu : le MP4 rangé pendant l'incident n'a pas de vignette. Sa tâche, coupée avant d'être prête, échoue au bout de 24 heures sans être relancée, et le rattrapage du démarrage ne reprend que les fichiers sans aucune tâche de vignette.
- [ ] **Uptime Kuma** : hors de dashu, dans son propre conteneur ; installation décrite dans le README (« Supervision et accès d'un assistant »), à faire sur l'hôte.

## Espaces et outils : avancement

Cahier des charges dans `SPEC.md` (« Espaces et outils »), accepté en entier le 13 novembre 2026. Tout est écrit, vérifié par les tests d'intégration et en service sur le serveur (version 61c8738) ; **pas encore essayé point par point** depuis le téléphone et l'ordinateur.

- [x] **B. Ranger après coup** : espace au choix pour une tâche et un envoi, « Déplacer vers… » pour les tâches et les fichiers (la vignette suit). Tests `tests/ranger.rs`.
- [x] **D. Savoir où on est** : titre de l'onglet, rappel au-dessus du titre des pages, liseré de la couleur de l'espace. Test `on_sait_toujours_dans_quel_espace_on_est`.
- [x] **G. Menu « Convertir » et onglet Outils** : sorties possibles selon le type, refus de toute autre ; fichiers de l'onglet Outils hors des listes, gardés ou effacés au bout de 24 heures. Tests `tests/conversions.rs`, tests unitaires des formats d'image et des arguments ffmpeg et Ghostscript.
  - Les vraies conversions (ffmpeg, Ghostscript, libheif) ne sont vérifiées que par les tests unitaires de leurs arguments : à essayer sur le serveur avec une photo HEIC, une vidéo `.mov` et un PDF.
- [x] **H. Suivi partout** : « 1 conversion en cours » dans la barre latérale de toutes les pages, puis « Prêt : … ». Test `une_conversion_va_jusqu_au_resultat`.
- [x] **A. Perso montre tout** : dashboard, notes, fichiers et agenda de tous les espaces, filtre « Seulement Perso ». Test `perso_montre_tous_les_espaces`. Depuis les dossiers (13 novembre 2026), l'onglet Fichiers de Perso s'ouvre sur la racine, un dossier par espace ; le filtre « Seulement Perso » des fichiers est devenu le dossier Perso.
- [x] **C. L'espace dans l'adresse** : `/e/<slug>/…`, en-tête `X-Espace` par onglet, redirections et historique qui gardent l'espace. Tests `tests/adresse.rs`.
- [x] **E. Archiver un espace** ; **I. règles par espace** ; **J. remplacer l'original** ; **F. espace du moment**. Tests `tests/espaces_suite.rs`.
- [x] **K. Déposer partout, coller une image dans une note** ; **L. la palette connaît la page** (aujourd'hui la barre de commandes). Tests `deposer_partout_et_coller_une_image`, `la_barre_connait_la_page`. Le glisser-déposer et le collage eux-mêmes (JavaScript) se vérifient dans le navigateur.
- Migrations 0011 (`files.expires_at`, `uploads.for_tools`, `jobs.target`) et 0012 (`spaces.archived_at`, `auto_mp4`, `auto_heic_jpeg`, `users.moment_space`).
- **Barre latérale détachée** (13 novembre 2026) : le glissement était saccadé ; elle a maintenant sa propre couche (`will-change`) et une courbe par sens. Mesuré dans le navigateur de la démo : une image toutes les 16 ms à l'arrivée comme au départ.

## Barre de commandes : avancement

Cahier des charges dans `SPEC.md` (« Barre de commandes »), accepté le 13 novembre 2026. En service sur le serveur (version 61c8738), pas encore essayé point par point.

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

## Éditeur de notes en une seule vue : avancement

Demandé par Rémi le 13 novembre 2026 : plus d'éditeur et d'aperçu côte à côte, mais une seule vue mise en forme, comme dans Notion, avec le menu `/`. Fait sur ProseMirror (à la place de CodeMirror) ; le Markdown reste le format enregistré. En service sur le serveur (version 61c8738) ; reste à l'essayer sur l'ordinateur et sur l'iPhone.

- [x] Une seule vue : titre, tags et texte défilent ensemble ; l'ancien aperçu (`POST /notes/preview`) n'existe plus. Le serveur met le texte en forme en attendant le script. Test `l_editeur_est_une_seule_vue` (page, script sans `eval`, icônes du menu dans le sprite).
- [x] Menu `/` : texte, titres 1 à 3, listes à puces, numérotées et à cocher, tableau, citation, séparateur, bloc de code, lien, image, fichier, date du jour ; filtre à la frappe, `↑` `↓` `Entrée`, `Échap` le ferme. Vu dans le navigateur (instance locale), au clavier, en largeur ordinateur et téléphone, thèmes clair et sombre.
- [x] Raccourcis Markdown pendant la frappe (`##`, `-`, `[]`, `**gras**`…), `Entrée` qui continue une liste et en sort sur une ligne vide, retour arrière qui rend un titre en texte, annuler. Vu dans le navigateur.
- [x] Tableau : `Tab` d'une cellule à l'autre, ligne ajoutée au bout, barre pour ajouter ou retirer ligne et colonne ; un « | » tapé dans une cellule est échappé dans le Markdown. Vu dans le navigateur.
- [x] Lien : barre de mise en forme sur une sélection, `Ctrl+K`, recherche des notes et fichiers (`/cmd/suggest`), fiche Ouvrir / Modifier / Retirer. Case à cocher cliquée : `- [x]` dans le Markdown. Vu dans le navigateur.
- [x] Collage : du Markdown collé devient des blocs ; une image collée part dans les fichiers de l'espace et s'affiche dans la note. Vu dans le navigateur.
- [x] `Échap` sort de l'éditeur, puis `/` ouvre la barre de commandes (ProseMirror retenait `Échap` : corrigé). Vu dans le navigateur.
- [x] Aucune erreur dans la console, donc rien de bloqué par la CSP (menus placés par une feuille construite, pas de style en ligne).
- [x] Aller-retour Markdown -> éditeur -> Markdown identique sur titres, listes, cases, tableaux alignés, code, liens, images (script de vérification lancé en local, pas versionné). Différences connues, sans effet sur le rendu : des échappements en plus (`\[`), un retour à la ligne simple devient une espace (l'aperçu l'affichait déjà ainsi), `_italique_` devient `*italique*`. Elles n'apparaissent qu'à la première modification d'une ancienne note.
- [ ] Essayé sur l'iPhone (clavier, menu `/` au toucher, sélection et barre de mise en forme sous le menu d'iOS).
- [ ] Lighthouse de la page éditeur sur le serveur réel. En local : `editor.js` pèse 100,7 Ko en brotli (CodeMirror : 96,9 Ko), page éditeur environ 155 Ko de HTML + CSS + JS, sous la cible de 250 Ko.
- Limites : le code n'est pas coloré pendant l'édition (il l'est dans le rendu du serveur) ; pas de bloc dépliable ni d'encadré (Notion en a, le Markdown non) ; une entité HTML rare (`&hearts;`) reste écrite telle quelle dans l'éditeur.

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

## Dossiers : avancement

Demandé par Rémi le 13 novembre 2026, cahier des charges dans `SPEC.md` (« Dossiers »). Il revient sur la décision du 11 novembre (fichiers sans dossiers). Tout est vérifié par les tests d'intégration (`tests/dossiers.rs`, 14 tests), dans le navigateur sur une instance locale, puis **sur le serveur** : en service depuis les PR #30 et #31, Rémi l'a essayé sur `app.dashu.dev` le 15 novembre 2026, depuis son iPhone et son ordinateur, et tout marche.

- [x] La racine de Fichiers montre un dossier par espace (pas les archivés) et Temporaire ; renommer un espace renomme son dossier. Test `la_racine_montre_les_espaces_et_temporaire`.
- [x] Un dossier se crée dans un espace et dans un dossier ; un troisième niveau est refusé par le serveur ; deux voisins ne portent pas le même nom (« Math » et « math », « Économie » et « économie »). Test `des_dossiers_sur_trois_niveaux_au_plus`.
- [x] Un fichier envoyé depuis un dossier y arrive ; sa copie convertie aussi, et « Remplacer l'original » lui donne le dossier de l'original. Test `un_fichier_envoye_dans_un_dossier_y_arrive_avec_sa_copie`.
- [x] Temporaire montre un fichier déposé dans Outils ; la fenêtre « Ranger » le garde dans le dossier choisi. Test `temporaire_puis_range_dans_un_dossier`.
- [x] Le sélecteur de l'éditeur, à côté des tags, propose la racine et les dossiers du premier niveau de l'espace de la note ; une note sans choix est à la racine. Test `le_selecteur_de_l_editeur_range_la_note`. Vu dans le navigateur : le choix met à jour le sélecteur sans recharger l'éditeur.
- [x] « Voir plus… » ouvre la fenêtre « Ranger » (tous les espaces, dossiers et sous-dossiers) ; un dossier d'un autre espace y déplace la note, qui s'ouvre dans cet espace. Test `voir_plus_range_dans_un_autre_espace`. Vu dans le navigateur.
- [x] Une note rangée apparaît dans son dossier de Fichiers ; l'onglet Notes la montre dans la section de son dossier du premier niveau. Test `une_note_rangee_se_voit_dans_fichiers_et_dans_notes`.
- [x] Supprimer un dossier non vide : avertissement, puis « Transférer » (tout monte d'un niveau, un nom pris devient « Exercices (2) ») ou « Tout supprimer » (notes, fichiers, disque compris, sous-dossiers) ; sans choix, refus. Test `supprimer_un_dossier_vide_ou_non`. Vu dans le navigateur.
- [x] Un second compte ne voit, ne remplit, ne renomme ni ne supprime les dossiers du premier ; ranger dans un dossier d'un autre espace que celui demandé est refusé. Test `un_autre_compte_ne_touche_pas_aux_dossiers`.
- [x] Supprimer un espace en gardant son contenu emmène ses dossiers (« MATH (2) » si le nom est pris). Test `supprimer_un_espace_emmene_ses_dossiers`. L'API range une note qui change d'espace à la racine du nouveau. Test `l_api_range_a_la_racine_du_nouvel_espace`.
- [x] Déplacer un dossier entier (ajouté le même jour) : avec ses notes, ses fichiers, ses sous-dossiers et ses conversions en cours, vers un autre dossier ou un autre espace ; un dossier qui a des sous-dossiers ne va que directement dans un espace (la fenêtre ne propose que les espaces) ; pas dans lui-même ni à côté d'un homonyme. Test `deplacer_un_dossier_entier`. Vu dans le navigateur.
- [x] Glisser-déposer dans l'onglet Fichiers (ajouté le même jour) : un fichier, une note ou un dossier lâché sur un dossier, le dossier d'un espace ou un pas du fil d'Ariane y est rangé ; des fichiers de l'ordinateur lâchés sur un dossier y arrivent. Test `glisser_deposer_dans_l_onglet_fichiers` (adresses et cibles dans la page, rangements). Vu dans le navigateur à la souris (note, dossier, fichier sur le fil d'Ariane) ; le dépôt de fichiers de l'ordinateur sur un dossier, par un événement simulé (l'automatisation ne glisse pas un vrai fichier du bureau).
- [x] « Nouvelle note » dans le dossier d'un espace et dans un dossier (ajouté le même jour) : la note naît dans le dossier ouvert. Test `creer_une_note_depuis_fichiers`. Vu dans le navigateur.
- [x] Aucune erreur dans la console (rien de bloqué par la CSP) ; largeur téléphone sans défilement horizontal.
- [x] Essayé sur le serveur, depuis l'iPhone et l'ordinateur (Rémi, 15 novembre 2026). Au doigt, le glisser-déposer de Safari n'est pas visé : la fenêtre « Ranger » fait la même chose.
- Migration 0013 (`folders`, `folder_id` sur `notes`, `files` et `uploads`). Les tests existants ont été adaptés là où Perso montrait la liste des fichiers (`/files?dossier=espace`).

## Espaces partagés : avancement

Demandé par Rémi le 16 novembre 2026, cahier des charges dans `SPEC.md` (« Espaces partagés »). Tout est vérifié par les tests d'intégration (29 tests dans `tests/espaces_partages_membres.rs`, `_notes.rs`, `_taches.rs` et `_fichiers.rs`). La partie visible a été essayée dans le navigateur, sur une instance locale avec deux comptes : invitation, acceptation, page Membres, note commune et garde-fou de version, en taille ordinateur et téléphone, sans erreur bloquée par la CSP. En service sur le serveur (version 61c8738), **pas encore essayé avec un second compte réel**.

- [x] Le propriétaire invite un compte existant ; une adresse sans compte ou non autorisée, soi-même, un membre, un doublon, Perso et un espace archivé sont refusés, sans rien créer. Tests `inviter_puis_accepter`, `refus_d_invitation_sans_rien_creer`.
- [x] L'invité voit l'invitation en haut de Réglages ; accepter ajoute l'espace à son île (« Partagé par … »), refuser la fait disparaître, le propriétaire peut l'annuler ; l'invitation d'un autre compte répond 404. Tests `inviter_puis_accepter`, `refuser_et_annuler_une_invitation`, `invitation_d_un_autre_compte`. Vu dans le navigateur.
- [x] Accepter au-delà de 8 espaces est refusé. Test `limite_de_huit_espaces`.
- [x] Un membre voit, crée, modifie, range et supprime les notes, fichiers, dossiers et tâches de l'espace ; un espace non partagé répond 404. Tests `le_membre_lit_modifie_epingle_et_deplace`, `les_membres_partagent_les_taches`, `un_membre_voit_telecharge_et_previsualise`, `un_membre_renomme_range_et_supprime`, `un_membre_envoie_dans_l_espace_partage`, `les_dossiers_sont_communs`, `un_espace_non_partage_repond_404`, `un_dossier_non_partage_est_refuse`.
- [x] Les tags sont communs ; deux tags de même nom ne s'affichent qu'une fois, sur une note comme sur un fichier. Tests `tags_de_meme_nom_communs`, `deux_tags_de_meme_nom_n_en_font_qu_un`.
- [x] Les agendas et Temporaire d'un membre restent invisibles aux autres. Tests `un_agenda_de_bob_reste_invisible_a_alice`, `le_temporaire_reste_a_son_auteur`.
- [x] Un membre ne renomme, n'archive, ne supprime l'espace ni n'invite (403). Test `un_membre_ne_gere_pas_l_espace`. La suppression ne transfère le contenu que vers un espace du propriétaire. Test `le_transfert_n_accepte_que_les_espaces_du_proprietaire`.
- [x] Quitter ou être retiré coupe l'accès à la requête suivante, laisse le contenu, ramène ses agendas dans son Perso, révoque ses liens de partage et remet son espace « moment » au défaut. Tests `quitter_un_espace`, `retirer_un_membre`, et pour la suppression `supprimer_un_espace_partage_en_gardant_le_contenu`, `supprimer_un_espace_partage_avec_son_contenu`.
- [x] Un membre qui a déjà un espace du même identifiant ouvre le partagé à sa propre adresse (`/e/ecole-2/`). Test `inviter_puis_accepter`.
- [x] Le Perso d'un membre réunit le contenu de l'espace partagé ; l'API et `/open` le trouvent aussi ; le tableau de bord d'un espace partagé est commun. Tests `le_membre_lit_modifie_epingle_et_deplace`, `open_trouve_les_notes_des_espaces_partages`, `l_api_voit_les_espaces_partages`, `un_membre_modifie_le_tableau_de_bord_partage`.
- [x] Un enregistrement de note sur une version dépassée est refusé (409) sans rien écraser ; l'indicateur d'état le dit. Test `numero_de_version_contre_les_ecrasements`. Vu dans le navigateur : la base garde le texte de l'autre, l'indicateur affiche « Modifiée ailleurs entre-temps ». Il a fallu corriger l'éditeur, qui remplaçait ce message par « Échec de l'enregistrement ».
- [x] Liens de partage : tout membre voit et révoque ceux de l'espace ; un lien ne sert plus un fichier dont l'espace n'est plus accessible à son créateur (fichier rangé dans l'espace privé d'un autre membre). Tests `un_membre_voit_et_revoque_un_lien_d_alice`, `un_lien_ne_sert_plus_un_fichier_sorti_de_l_espace_partage`.
- [ ] Essayé sur le serveur, avec un second compte réel.
- Migration 0016 (`space_members`, `space_invitations`, vue `space_access`, `notes.revision`). La colonne `user_id` des contenus garde l'auteur ; l'accès passe par l'espace. Une centaine de requêtes ont changé de filtre ; restent par compte les agendas, Temporaire, les envois en cours et les conversions.

## Notes en temps réel : avancement

Demandé par Rémi le 16 novembre 2026, cahier des charges dans `SPEC.md` (« Notes en temps réel »). Le serveur est vérifié par les tests d'intégration (`tests/notes_direct.rs`, 8 tests sur un vrai serveur local avec un vrai client WebSocket, et un test de l'origine dans `src/notes/live.rs`). L'éditeur a été essayé dans le navigateur, sur une instance locale, avec la même note ouverte dans deux onglets. En service sur le serveur (version 61c8738), **pas encore essayé** à travers le tunnel Cloudflare et Tailscale.

- [x] Ce qu'un onglet tape apparaît dans l'autre dans la seconde. Vu dans le navigateur. Deux ajouts au même endroit au même moment (l'envoi du premier retardé exprès de 1,5 s) : les deux onglets finissent avec le même texte, les deux ajouts gardés, et la base l'enregistre. Tests `les_etapes_sont_numerotees_et_diffusees` (numérotation, version dépassée ignorée, page arrivée en retard).
- [x] Le curseur de l'autre s'affiche avec son prénom et sa couleur, et suit le texte ; ses avatars sont en haut de la note. Vu dans le navigateur. Le nom et la couleur viennent du profil, jamais de la page. Test `curseurs_titre_et_presents_viennent_du_serveur`.
- [x] Le titre suit en direct. Test `curseurs_titre_et_presents_viennent_du_serveur`.
- [x] Seul le Markdown de la dernière version est enregistré ; l'API le lit ; l'indicateur affiche « Enregistré à » dans les deux onglets. Test `seule_la_derniere_version_est_enregistree`. Vu dans le navigateur.
- [x] Coupure courte : la session est retrouvée (test `une_reconnexion_retrouve_la_session`). Coupure longue (serveur arrêté puis relancé) : le texte tapé pendant la coupure est enregistré par le repli classique, avec le garde-fou de version. Vu dans le navigateur. Cet essai a montré un trou, corrigé : quitter la page pendant une coupure perdait ce qui n'était pas parti, et le repli (`sendBeacon`) part maintenant.
- [x] Sans WebSocket (pas de réponse en 4 secondes, navigateur trop ancien, note trop fréquentée), l'éditeur enregistre comme avant. C'est le même chemin que le repli ci-dessus, et les tests de l'enregistrement classique passent toujours.
- [x] Refus sans session, d'une autre origine (dont `dashu.dev`), sans en-tête `Origin`, ou sur une note inaccessible ; un membre retiré perd le canal à son envoi suivant, ou au signe de vie suivant (30 s au plus). Tests `refus_sans_session_d_une_autre_origine_ou_sans_acces`, `un_membre_retire_perd_le_canal`, `une_note_supprimee_ferme_la_session`, `origine_de_dashu_seulement`.
- [x] Une note modifiée par l'API pendant l'édition n'est pas écrasée : les présents voient « Modifiée ailleurs entre-temps ». Test `une_modification_exterieure_n_est_pas_ecrasee`.
- [x] Ctrl+Z n'annule que ses propres modifications. Vu dans le navigateur (une saisie annulée, celle de l'autre onglet gardée).
- [x] Aucune erreur bloquée par la CSP : `connect-src 'self'` laisse passer la WebSocket de la même adresse.
- [ ] Essayé sur le serveur, à travers le tunnel Cloudflare et Tailscale, avec deux comptes réels et sur l'iPhone. À vérifier en particulier : que Tailscale transmet l'hôte d'origine (sinon l'origine est refusée et l'éditeur reste en enregistrement classique, sans rien perdre).
- Dépendances : `prosemirror-collab` 1.3.1 côté éditeur (justifié dans le README), fonction `ws` d'axum côté serveur ; `tokio-tungstenite` et `futures-util` pour les tests seulement. Aucune migration : les étapes ne vivent qu'en mémoire.

## Événements et Google Agenda : avancement

Demandé par Rémi le 17 novembre 2026, cahier des charges dans `SPEC.md` (« Événements et Google Agenda »). Vérifié par les tests (`tests/agenda_evenements.rs`, 7 tests, dont 4 contre un faux Google local qui imite l'échange du code, l'adresse du compte et l'API Agenda ; 2 tests unitaires dans `src/calendar/google.rs`) et dans le navigateur sur une instance locale, sans le vrai Google. En service sur le serveur (version 61c8738), essayé avec le vrai Google le 23 novembre 2026.

- [x] Créer, modifier, supprimer un événement depuis l'Agenda (« Nouvel événement »), en heures ou sur la journée entière ; titre vide, date invalide, fin avant le début, heure manquante, espace d'un autre compte refusés (422, ce qui a été tapé reste). Tests `creer_modifier_supprimer_un_evenement`, `un_formulaire_invalide_est_refuse`. Vu dans le navigateur.
- [x] L'événement apparaît dans l'Agenda (cliquable), les widgets Calendrier et Prochains cours, Perso, la barre de commandes et l'API ; il reste à son auteur, même dans un espace partagé (404 pour les autres) ; il revient dans le Perso d'un membre qui quitte l'espace. Test `un_evenement_reste_a_son_auteur`. Tout l'agenda lit une vue `agenda_items` (flux iCal et événements de dashu).
- [x] Relier Google Agenda : `state` vérifié, PKCE, permission « agenda » exigée (décochée chez Google : refusé), jeton chiffré en base, adresse affichée ; déconnecter retire la permission chez Google. Test `relier_google_agenda`.
- [x] Un événement coché arrive dans Google Agenda (heures avec le fuseau de Paris), suit ses modifications et sa suppression. Test `un_evenement_suit_dans_google_agenda`.
- [x] Google indisponible : l'événement est marqué en attente (formulaire et Réglages le disent), et part au passage suivant (toutes les cinq minutes). Une modification faite pendant un envoi n'est pas perdue (compteur de changements). Test `google_indisponible_puis_reprise`.
- [x] Pas de doublon quand l'agenda Google est aussi importé en iCal (UID `<id>@google.com`). Test `un_evenement_suit_dans_google_agenda`.
- [x] Essai réel sur le serveur avec le vrai Google, après avoir activé **Google Calendar API** dans le projet Google de dashu (README, « Créer les identifiants Google », étape 5). Fait par Rémi le 23 novembre 2026 : API activée, agenda relié depuis Réglages, l'événement créé dans dashu arrive dans Google Agenda.
- Migration 0017 (`events`, `google_accounts`, `google_deletions`, vue `agenda_items`). Dépendances : `ring` et `base64` en direct (déjà tirées par rustls et axum), fonction `json` de reqwest. Le champ de date et d'heure suivent maintenant le style des autres champs (16 px sur écran tactile, sinon iOS zoome).

## Connecteur Claude et API complète : avancement

Demandés par Rémi le 20 novembre 2026, cahiers des charges dans `SPEC.md` (« Connecteur Claude (MCP) » et « API complète et corbeille »), livrés une PR par étape.

**Étape 1, jetons par compte** (23 novembre 2026). Vérifiée par les tests (`tests/jetons.rs`, 5 tests ; `tests/api.rs` ; tests unitaires de `src/auth/tokens.rs`) et dans le navigateur, sur une instance locale : carte Connexions, jeton créé et copié, appel de l'API avec lui depuis la page. **Pas encore sur le serveur.**

- [x] Réglages › Connexions crée un jeton (nom, durée de 30 jours, 90 jours ou 1 an), l'affiche une fois avec un bouton Copier et n'en garde que l'empreinte SHA-256 ; un nom vide ou trop long, une durée inconnue ou un 21e jeton sont refusés. Tests `un_jeton_s_affiche_une_fois_et_seule_son_empreinte_est_gardee`, `nom_vide_duree_inconnue_et_vingt_et_unieme_jeton_refuses`.
- [x] Un jeton agit sur `/api/v1` au nom de son compte : ses espaces et les espaces partagés rejoints, 404 ailleurs. Test `un_jeton_agit_au_nom_de_son_compte`.
- [x] Jeton révoqué, expiré (toujours listé, « expiré », à supprimer), « Tout révoquer » ou compte retiré de `ALLOWED_EMAILS` : 401 à la requête suivante ; un autre compte ne révoque pas le jeton (404). Test `revoque_expire_tout_revoquer_ou_compte_retire_coupent_l_acces`.
- [x] `API_TOKEN` d'Hermès marche comme avant (tous les tests de `tests/api.rs`). Sans `API_TOKEN`, l'API n'est plus désactivée : elle n'accepte que les jetons personnels (test `sans_api_token_seuls_les_jetons_personnels_ouvrent_l_api`).
- Dernière utilisation notée au plus une fois par minute, affichée dans la carte. Les logs disent qu'un jeton a été créé ou révoqué (compte et nom), jamais le jeton.
- Migration 0018 (`access_tokens`). La colonne `grant_id` servira à l'étape 3 (OAuth).

**Étape 2, route `/mcp`** (23 novembre 2026). Vérifiée par les tests (`tests/mcp.rs`, 6 tests). **Pas encore essayée avec un vrai Claude Code** ni sur le serveur.

- [ ] Claude Code se branche avec un jeton personnel, liste les outils et les appelle. Le protocole est vérifié par les tests (`initialize`, notification `notifications/initialized` en 202, `ping`, `tools/list`, `tools/call`) ; reste l'essai avec Claude Code lui-même (commande donnée par la carte Connexions).
- [x] Chaque outil suit les règles et les limites de l'API : ils appellent les mêmes fonctions (`src/api/mod.rs`, séparées de leurs routes pour cela). Une erreur de saisie (espace inconnu, titre trop long, date invalide, argument manquant, période de plus de 62 jours) revient en résultat d'erreur lisible. Tests `les_outils_font_ce_que_fait_l_api`, `une_erreur_de_saisie_revient_en_resultat_lisible`, `un_jeton_n_agit_que_sur_son_compte`.
- [x] Sans jeton, avec `API_TOKEN` ou un cookie de session : 401 et `WWW-Authenticate` ; `Origin` étranger : 403 ; lot JSON-RPC, JSON illisible, méthode ou outil inconnus : erreur JSON-RPC propre ; `GET /mcp` : 405. Test `refus_du_protocole`.
- [x] Les annotations (lecture, écriture, destruction) et les instructions du serveur sont présentes. Test `initialisation_et_liste_des_outils`.
- [x] Une note modifiée par MCP pendant qu'on l'édite n'est pas écrasée (l'éditeur reçoit 409, « Modifiée ailleurs »). Test `une_note_modifiee_par_mcp_pendant_l_edition_n_est_pas_ecrasee`.
- La carte Connexions donne, à la création d'un jeton, la commande `claude mcp add` prête à coller. Limite de 60 appels d'un coup par jeton puis un par seconde (tower_governor, compteur par empreinte du jeton) ; 15 Mo par requête.
- Protocole écrit à la main (JSON-RPC avec serde), sans SDK, comme le prévoit la spec. Versions acceptées : 2025-06-18, 2025-03-26, 2024-11-05.

**Étape 3, OAuth pour claude.ai et l'iPhone** (23 novembre 2026). Vérifiée par les tests (`tests/oauth.rs`, 10 tests qui jouent le flux complet ; tests unitaires de `src/oauth/mod.rs`) et dans le navigateur sur une instance locale : page « Autoriser », puis retour vers une autre origine (`127.0.0.1`) avec le code et le `state`, sans blocage de la CSP. **Pas encore essayée avec le vrai claude.ai**, ni sur le serveur.

- [x] Les deux documents `/.well-known/` sont servis et pointent vers `BASE_URL` (et la variante `/.well-known/oauth-protected-resource/mcp`) ; `/mcp` sans jeton répond 401 avec leur adresse. Test `documents_de_decouverte`.
- [ ] Depuis claude.ai : ajout du connecteur, connexion, autorisation, puis outils dans claude.ai et l'app iPhone ; Claude Code sans jeton collé. Le parcours est joué de bout en bout par le test `de_l_enregistrement_aux_outils` (enregistrement, connexion qui revient à l'autorisation, page « Autoriser », code, jetons, `/mcp`) ; reste l'essai réel.
- [x] Un second compte branche son propre Claude et ne voit que ses espaces. Test `un_second_compte_branche_son_propre_claude`.
- [x] Refusés : adresse de retour hors de Claude à l'enregistrement, client ou adresse de retour inconnus (page d'erreur, jamais de redirection), PKCE absent, en `plain` ou faux, ressource étrangère, code réutilisé, expiré, d'un autre client ou d'une autre adresse de retour, jeton OAuth présenté à `/api/v1`, consentement sans jeton CSRF. Tests `enregistrement_limite_aux_adresses_de_claude`, `autorisation_stricte`, `refuser_renvoie_une_erreur_a_claude`, `de_l_enregistrement_aux_outils`.
- [x] Un jeton de renouvellement réutilisé révoque toute l'autorisation ; un autre client ne le renouvelle pas ; un compte retiré de `ALLOWED_EMAILS` ne renouvelle plus. Tests `un_jeton_de_renouvellement_reutilise_revoque_tout`, `un_compte_retire_ne_renouvelle_plus`.
- [x] Révoquer dans Réglages › Connexions (ou « Tout révoquer ») coupe Claude à la requête suivante ; un autre compte ne révoque pas (404) ; Claude peut se déconnecter par `/oauth/revoke`. Tests `revoquer_dans_reglages_coupe_claude`, `claude_se_deconnecte_par_la_revocation`.
- La page « Autoriser » a sa propre CSP : son formulaire ne peut partir que vers l'origine de l'adresse de retour de cette demande-là. La demande en attente est gardée dans la session (10 minutes), jamais dans l'adresse ; la connexion par mot de passe ou par Google y revient.
- Migration 0019 (`oauth_clients`, `oauth_codes`, `oauth_grants`, et un déclencheur qui efface les jetons d'accès d'une autorisation supprimée). Ménage à chaque échange de jeton (codes, jetons d'accès et autorisations expirés) et à chaque enregistrement (clients jamais autorisés depuis 24 heures).
- À vérifier à l'essai réel (étape 3) : que Cloudflare ne pose pas de défi anti-robot sur `/mcp`, `/.well-known/` et `/oauth/` ; et que la fenêtre ouverte par claude.ai revient bien (dashu envoie `Cross-Origin-Opener-Policy: same-origin` sur ses pages, ce qui coupe le lien avec la fenêtre qui l'a ouverte ; si claude.ai en dépendait, il faudrait l'assouplir sur les pages du parcours).

**Étape 4, corbeille** (23 novembre 2026). Vérifiée par les tests (`tests/corbeille.rs`, 8 tests ; les anciens tests de suppression adaptés : ils vérifient maintenant la disparition partout, puis l'effacement depuis la corbeille). **Pas encore sur le serveur.**

- [x] Supprimer une note, un fichier, un dossier (avec son contenu) ou un espace, depuis le site, le met dans la corbeille ; il disparaît de partout. Une note : sa page, les listes, la recherche, Perso, le dossier, le filtre par tag, le widget, la barre de commandes (`/open`, suggestions), l'API. Un fichier : sa page, son téléchargement, la liste, la recherche, le widget. Un espace : son adresse, chez le propriétaire et chez les membres, l'API, la recherche de Perso. Tests `une_note_supprimee_disparait_de_partout_puis_revient_a_sa_place`, `un_fichier_supprime_perd_ses_liens_de_partage`, `un_dossier_part_avec_son_contenu_et_revient_avec_lui`, `un_espace_supprime_ne_se_restaure_que_par_son_proprietaire`. La suppression par l'API arrive à l'étape 5.
- [x] La page Corbeille montre qui a supprimé, quand et par où, et dans combien de jours l'élément s'efface ; restaurer remet l'élément à sa place (dossier, tags), à la racine si son dossier n'existe plus, sous un nom libre si le sien a été repris (« Math (2) », « Taff (2) »). Tests ci-dessus et `sans_son_dossier_un_element_revient_a_la_racine`.
- [x] Supprimer définitivement et vider la corbeille : sur le site seulement, avec confirmation ; l'API n'a pas de route pour ça. Test `supprimer_definitivement_et_vider_sur_le_site_seulement`.
- [x] Après 30 jours, l'élément est effacé, fichier du disque compris (vérifié toutes les heures). Test `apres_trente_jours_tout_s_efface`.
- [x] Dans un espace partagé, tout membre voit et restaure ce qui y a été supprimé ; un autre compte reçoit 404 ; un espace supprimé ne se voit et ne se restaure que par son propriétaire. Tests `la_corbeille_d_un_espace_partage_est_commune`, `un_espace_supprime_ne_se_restaure_que_par_son_proprietaire`.
- [x] Les liens de partage d'un fichier supprimé ne marchent plus, même s'il est restauré. Test `un_fichier_supprime_perd_ses_liens_de_partage`. Une note supprimée ferme son canal en direct : le test existant `une_note_supprimee_ferme_la_session` passe toujours (l'accès est refusé dès que la note est dans la corbeille).
- « Remplacer l'original » met l'original dans la corbeille. Les tâches, les événements et les fichiers de Temporaire restent supprimés pour de bon, comme le prévoit la spec pour les deux premiers.
- Migration 0020 : colonnes de la corbeille, espace corbeille par espace (`spaces.trash_of`), vues `space_access` (sans les espaces corbeille ni supprimés), `trash_access` et `agenda_items` (sans l'agenda d'un espace supprimé). Choix technique noté dans `SPEC.md` (« API complète et corbeille », décisions du 23 novembre).

**Étape 5, API des notes, fichiers, dossiers, liens de partage et de la corbeille** (23 novembre 2026). Vérifiée par les tests (`tests/api_contenu.rs`, 7 tests). **Pas encore sur le serveur.**

- [x] Chaque ligne du tableau a sa route : notes (ranger dans un dossier, retirer un tag, supprimer, télécharger en Markdown), fichiers (lister et chercher, informations, téléchargement, envoi par morceaux, renommer, ranger, garder de Temporaire, tags, supprimer), liens de partage (créer, lister, révoquer), dossiers (arborescence, créer, renommer, déplacer, supprimer), corbeille (lister, restaurer). Tests : cas normal, élément d'un autre compte (404), saisie invalide (nom vide ou pris, espace inconnu, troisième niveau, durée inconnue, taille trop grande, ni espace ni dossier). Le téléchargement d'une note en PDF passe par la même fonction que le site, mais n'est pas testé ici (il attend le service de conversion).
- [x] Les routes appellent les mêmes fonctions que le site : les handlers des fichiers, dossiers, liens et notes ont été séparés de leur travail (`files::rename_to`, `move_into`, `keep_in`, `open_upload`, `receive`, `folders::create_in`, `rename_to`, `move_to`, `share::create_link`, `revoke_link`, `notes::file_in_folder`, `export::markdown_download`…), sans changement de comportement du site (tous ses tests passent).
- [x] Un fichier envoyé par l'API arrive identique, en morceaux (un morceau hors d'ordre répond 409 avec l'octet où reprendre) ; un fichier téléchargé est servi en pièce jointe, avec la CSP `sandbox`. Test `fichiers_envoi_lecture_rangement_et_suppression`.
- [x] Une suppression par l'API part dans la corbeille, avec le nom du jeton comme trace (« Hermès » pour `API_TOKEN`) ; un membre supprime dans un espace partagé, le propriétaire restaure. Tests `notes_ranger_detaguer_telecharger_supprimer`, `un_membre_supprime_dans_l_espace_partage_et_le_proprietaire_restaure`. Vider et effacer n'ont toujours pas de route (test de l'étape 4).
- [x] Les outils MCP suivent : 16 de plus (29 en tout), dont `envoyer_fichier` et `lire_fichier` jusqu'à 10 Mo en base64 ; suppressions et création de lien marquées destructrices. Test `les_outils_mcp_du_contenu`.
- [x] La commande `dashu` d'Hermès et son skill suivent (fichiers, dossiers, liens, corbeille ; envoi par morceaux) ; la règle « pas de commande pour supprimer une note » devient « une suppression part dans la corbeille », et la confirmation couvre toute suppression et la création d'un lien. Vérifié par une relecture et `python -m py_compile`, pas contre un vrai dashu.
- Chaque appel d'API est journalisé avec le compte, le jeton et l'action (méthode et chemin), jamais le contenu.

**Étape 6, API des conversions** (23 novembre 2026). Vérifiée par les tests (`tests/api_outils.rs`, 3 tests, avec le faux service de conversion des tests du site). **Pas encore sur le serveur.**

- [x] Formats possibles d'un fichier, convertir, compresser, suivre une conversion (en file, terminée avec le fichier produit), remplacer l'original par le résultat (l'original part dans la corbeille, avec le nom du jeton). Refus : sortie inconnue ou impossible pour ce type, pages manquantes ; un autre compte reçoit 404 ; sans service de conversion (`WORK_DIR`), les routes répondent 404. Test `formats_convertir_suivre_remplacer`, `sans_service_de_conversion_les_routes_n_existent_pas`.
- [x] Mêmes fonctions que le site : `tools::queue` rend maintenant le numéro de la tâche ; « Remplacer l'original » est devenu `tools::replace_with_result`, partagé (le test du site passe toujours).
- [x] Connecteur Claude : `formats_possibles`, `convertir`, `lister_conversions`, `suivre_conversion`, `remplacer_original` (destructeur). Test `les_outils_mcp_des_conversions`. Hermès : `formats`, `convert`, `jobs`, `job`, `replace` ; skill 1.2.0.

**Étape 7, API des événements et des sources iCal** (23 novembre 2026). Vérifiée par les tests (`tests/api_agenda.rs`, 3 tests ; ceux du site, `tests/agenda_evenements.rs`, passent toujours, Google compris). **Pas encore sur le serveur.**

- [x] Créer, lire, modifier (les champs absents restent), supprimer un événement ; il apparaît dans `GET /api/v1/calendar` avec son identifiant (`event`). Refus : titre vide, date invalide, fin avant le début, heure manquante, espace inconnu. Un autre compte reçoit 404. Mêmes fonctions que le site (`events::validate_in`, `insert`, `save`, `remove`), donc même copie dans Google Agenda. Test `evenements_creer_lire_modifier_supprimer`.
- [x] Une source iCal vers une adresse privée, la machine elle-même, un nom local ou en HTTP est refusée ; l'adresse complète n'est jamais redonnée (le domaine seul). Même vérification que Réglages (`settings::add_source`). La première synchronisation part en arrière-plan. Test `sources_ical_adresses_publiques_seulement`.
- [x] Connecteur Claude : `creer_evenement`, `modifier_evenement`, `supprimer_evenement` (destructeur), `lister_sources_agenda`, `ajouter_source_agenda`, `retirer_source_agenda` (destructeur). Test `les_outils_mcp_de_l_agenda`. Hermès : `event-add`, `event-edit`, `event-rm`, `sources`, `source-add`, `source-rm` ; skill 1.3.0. La décision du 17 novembre « pas d'action API pour créer un événement » est remplacée, comme le dit la spec du 20 novembre.

**Étape 8, API des espaces, des membres et des réglages** (23 novembre 2026). Vérifiée par les tests (`tests/api_espaces.rs`, 8 tests ; ceux du site passent toujours) et, pour la commande d'Hermès, sur un vrai dashu local avec deux comptes et une base jetable. **Pas encore sur le serveur.**

- [x] Espaces : créer, lire, renommer, couleur, icône, règles automatiques, archiver, désarchiver, supprimer (corbeille, ou en transférant le contenu : le choix est obligatoire). Refus : nom vide, pris ou trop long, couleur ou icône inconnue, 9e espace, Perso archivé ou supprimé. Test `espaces_creer_modifier_archiver_supprimer`, `huit_espaces_au_plus`.
- [x] Membres : voir, inviter, annuler, retirer, quitter ; invitations reçues, accepter, refuser. Un espace d'un autre compte répond 404, une action de propriétaire tentée par un membre 403, même avec une saisie invalide. Un membre ne voit pas les invitations en attente. Test `membres_inviter_accepter_retirer_quitter`.
- [x] Tableau de bord de chaque espace : lire, remplacer, vider, revenir à la disposition par défaut ; commun dans un espace partagé ; un corps sans `layout` est refusé au lieu de tout effacer. Test `tableau_de_bord_d_un_espace`.
- [x] Réglages : prénom, nom, avatar, thème, taille, barre latérale, raccourcis du téléphone, espace « moment ». Tout est vérifié avant la première écriture. Un champ inconnu (`password`, `email`) est refusé. Test `reglages_du_compte`, `ce_qui_protege_le_compte_n_a_pas_de_route`.
- [x] Tâches : changer d'espace (`space` dans `PATCH /tasks/{id}`). Cette ligne du tableau de la spec n'avait pas été faite aux lots précédents. Test `changer_une_tache_d_espace`.
- [x] Mêmes fonctions que le site : les handlers de `src/spaces/mod.rs`, `src/settings/` et `dashboard::move_task` sont séparés de leur travail, sans changement de comportement (tous les tests du site passent).
- [x] Connecteur Claude : 20 outils de plus (60 en tout). Archiver, supprimer, inviter, retirer, quitter, refuser et déplacer une tâche sont marqués destructeurs. Test `les_outils_mcp_des_espaces`.
- [x] Hermès : `space-add`, `space-edit`, `space-archive`, `space-unarchive`, `space-rm`, `members`, `invite`, `invite-cancel`, `member-rm`, `space-leave`, `invitations`, `invitation-accept`, `invitation-decline`, `dashboard`, `dashboard-set`, `settings`, `settings-set`, `avatar-set`, `avatar-rm`, `task-edit --move` ; skill 1.4.0. Chacune jouée contre un dashu local, refus compris. Les commandes des lots 5 à 7 restent vérifiées par une relecture seulement.

**Critères « API » de la spec :**

- [x] Chaque ligne du tableau a sa route et ses tests. Une exception : le téléchargement d'une note en PDF par l'API passe par la fonction du site, mais n'a pas de test d'API (il attend le service de conversion).
- [x] Les actions « Hors de l'API » n'ont pas de route (tests `supprimer_definitivement_et_vider_sur_le_site_seulement`, `ce_qui_protege_le_compte_n_a_pas_de_route`).
- [x] Un fichier envoyé par l'API arrive identique ; un fichier téléchargé est servi en pièce jointe (étape 5).
- [x] Une source iCal vers une adresse privée est refusée (étape 7).
- [x] La commande `dashu` d'Hermès, son skill et les outils MCP couvrent chaque nouvelle route.
- [x] STATUS et README à jour à chaque lot.

**Pour qui reprend :**

- **Où en sont les PR** : lots 1 à 7 fusionnés (#63 à #66, #68 à #70). Lot 8 : branche `api-espaces`, partie de `v1`.
- **Reste** : essayer les lots 4 à 8 sur le serveur après `dashu-update`, avec un vrai Claude et Hermès ; la page d'accueil si Rémi le veut.
- **Pièges rencontrés** : `cargo test` échoue sur `arguments_de_pandoc` sous Windows seulement (chemin `/j\gabarit.typst`), sans lien avec ce chantier ; sous Windows, un binaire `target/debug/dashu.exe` lancé bloque la compilation (lancer une copie) ; un long script Python passé en heredoc au shell Bash échoue (l'écrire dans un fichier) ; après une requête SQL nouvelle, `cargo sqlx prepare` sur une base jetable (`DATABASE_URL=sqlite://target/prepare.db`), puis versionner `.sqlx/`.

## Installation simple : avancement

Demandée par Rémi le 23 novembre 2026, cahier des charges dans `SPEC.md` (« Installation simple »), livrée une PR par étape.

**Étape 1, binaires publiés** (24 novembre 2026). Vérifiée par la CI de la branche (run 45354327899), par le test `tests/version.rs`, puis par la première publication après la fusion (run 45400215372 sur `v1`, commit 3316ca1) et par la bascule du serveur.

- [x] `LICENSE` contient l'AGPL-3.0 (texte officiel), `Cargo.toml` la déclare (`AGPL-3.0-or-later`), le README le dit.
- [x] `dashu version` affiche « dashu 1.0.0 <commit> » ; le commit vient de `DASHU_COMMIT` à la compilation, « inconnu » sinon. Test `version_affiche_la_version_et_le_commit` ; en CI, le binaire de l'archive affiche bien le commit du push.
- [x] Un push sur `v1` crée ou remplace la pré-version `canal-v1` : archives x86_64 (10,8 Mo) et aarch64 (10,1 Mo), `SHA256SUMS`. Archive x86_64 retéléchargée depuis l'adresse publique : l'empreinte correspond.
- [x] Un tag `vX.Y.Z` crée une version (canal stable) : `v1.0.0`, le 24 novembre (run 45405233082), marquée « dernière version » ; `releases/latest/download/…` sert ses archives (empreinte de l'archive aarch64 vérifiée), et les images `:1.0.0` et `:latest` se lisent sans identifiant.
- [x] Le binaire aarch64 est compilé sur un runner ARM, statique (`file` : « ARM aarch64, statically linked »), et son image répond à `/health` et `/login`.
- [x] L'image `ghcr.io/moreauremi/dashu:v1` existe pour amd64 et arm64 (une image par architecture, réunies sous un même nom). Construites et testées en CI : 10 Mo (x86_64) et 21 Mo (aarch64), sous les 30 Mo. Rendue publique le 24 novembre : son manifeste se lit sans identifiant (amd64 et arm64).
- [x] Le binaire musl mesuré contre le binaire compilé, le 24 novembre, sur le serveur, `/login` interrogé 20 fois depuis le conteneur (commande du README, « Mise à jour ») :

  | | Compilé sur place (glibc) | Téléchargé (musl) |
  | --- | --- | --- |
  | Médiane | 0,55 ms | 0,66 ms |
  | Moyenne | 0,56 ms | 0,93 ms |
  | Pire | 0,70 ms | 3,3 ms (la première requête après le démarrage : 2,7 ms) |
  | Mémoire | 10,4 Mo (après 11 min) | 5,8 Mo (juste après le démarrage) |

  Environ 0,1 ms de plus en médiane, attendu avec musl (allocateur de mémoire plus simple) ; plus de 20 fois sous la cible de 20 ms. Binaire téléchargé gardé. La mémoire est à remesurer après quelques heures de marche pour une comparaison juste.
- Un test unitaire, `tools::worker::tests::arguments_de_pandoc`, échoue sous Windows (chemins Linux assemblés avec `\`) ; il passe sur la CI Linux. Sans rapport avec cette étape.

**Étapes 2 et 3, installateur et mise à jour par binaires** (24 novembre 2026), dans la même PR. Vérifiées par la CI de la branche (run 45370369395), qui installe dashu pour de vrai sur une machine x86_64 et une machine ARM avec l'archive qu'elle vient de construire (`tests/deploy/installation.sh`) : questions dans un terminal simulé (`tests/deploy/repondre.py`), connexion, relance, mises à jour, retour arrière, menu. **Pas encore essayé sur une Debian neuve, ni sur le serveur.**

- [x] Sur une Debian 13 neuve, `curl … | sudo bash` installe dashu en posant les questions, sans rien compiler : essayé le 24 novembre sur un conteneur LXC tout neuf du Proxmox (`dashu-test`, version 1.0.0 du canal stable), en plus des machines Ubuntu de la CI. Trois défauts vus à cet essai, corrigés depuis : des avertissements de langue d'apt et de perl sur une machine neuve, Tailscale qui reliait la machine sous le nom `dashu` (pris par le vrai serveur), et le menu qui ne redonnait pas le code après un changement d'accès. Le test de la CI a aussi trouvé que « Changer l'accès » arrêtait le script en erreur sans Google configuré ; corrigé.
- [ ] Chaque mode d'accès marche de bout en bout. Le mode local est vérifié en CI et sur le conteneur de test (joignable seulement depuis le réseau de la maison, comme prévu) ; Tailscale essayé sur le conteneur de test, joint depuis un PC du réseau Tailscale ; restent Funnel et Cloudflare.
- [x] Le mot de passe ne s'affiche pas : la CI tape les réponses quand les questions sont posées et vérifie que le mot de passe n'apparaît pas dans ce qui s'affiche. Il passe à dashu par un tube (`printf`, commande interne de bash), jamais en argument ; le jeton du tunnel va dans `/etc/dashu/tunnel.env` (root seulement), lu par l'unité `dashu-tunnel.service`.
- [x] Une archive dont l'empreinte ne correspond pas est refusée, et rien ne change (test « archive abîmée » de la CI).
- [x] Relancé, le script ne touche ni à la base ni à `dashu.env` (empreinte du fichier comparée en CI) ; le menu change le mot de passe (vérifié en CI) ; un changement de configuration qui empêche dashu de démarrer est annulé (`changer_env`, même mécanisme que `dashu-compte`).
- [x] `install.sh <email> <adresse>` marche comme avant (relance sans question en CI) ; `--non-interactif` installe sans question.
- [x] x86_64 et aarch64 : toute la séquence passe sur les deux machines de la CI. Typst est choisi selon l'architecture (archive `aarch64-unknown-linux-musl` vérifiée sur GitHub), mais les conversions ne sont pas installées en CI (600 Mo).
- [x] `tests/deploy/installation.sh` tourne dans la CI, avec une limite de 20 minutes.
- [x] `dashu-update` télécharge, vérifie et installe la version du canal ; « déjà à jour » si rien n'a changé ; une version qui ne démarre pas est remplacée par l'ancienne, base restaurée, et il attend que l'ancienne réponde (vérifié en CI avec une fausse version qui s'arrête aussitôt). Le temps (1 à 2 minutes) est à mesurer sur le serveur.
- [x] `dashu-admin` garde exactement ses six actions ; `status` donne la version du binaire et le mode de mise à jour (vérifié en CI).
- [x] Hermès met à jour dashu comme avant ; son installation lit `/opt/dashu/share` : relancée le 24 novembre depuis l'hôte Proxmox, commande et skill réinstallés, API et `dashu-admin status` vérifiés depuis le compte d'Hermès.
- [x] Le serveur de Rémi est passé aux binaires le 24 novembre : `echo binaire > /etc/dashu/source && dashu-update` a téléchargé `canal-v1` (93d540a), vérifié son empreinte, fait l'instantané de la base (27 notes) et installé le binaire. Avant, deux mises à jour compilées (504a4c5, puis 3316ca1 avec le nouveau `dashu-update`, qui note le commit). Rust et `/opt/dashu/src` restent en place quelques jours, au cas où.
- Trouvé en route : l'ancien `dashu-update` ne vérifiait pas que l'ancienne version répondait après un retour arrière ; c'est corrigé dans les deux modes.

**Étape 4, premier lancement dans le navigateur** (24 novembre 2026). Vérifiée par les tests (`tests/installation.rs`, 7 tests ; tests unitaires de `src/setup.rs`), par la CI de la branche (run 45370406005 : installation, code lu dans ce qu'affiche le script, assistant joué comme un navigateur, relance par re-exec sous systemd, connexion, sur x86_64 et ARM) et dans le navigateur sur une base neuve, sous Windows. **Pas encore essayé avec l'image Docker publiée ni sur une Debian neuve.**

- [x] Sans compte, toute page mène à `/setup` ; l'API, `/mcp` et OAuth répondent 503 en JSON ; sans le bon code, rien ne se crée ; le formulaire final sans code renvoie au code ; les essais sont limités (429 après 5). Tests `sans_compte_tout_mene_au_premier_lancement`, `sans_le_bon_code_rien_ne_se_cree`.
- [x] Le code n'apparaît ni dans la page ni dans une adresse (il se tape dans un formulaire avec jeton CSRF ; `?code=` ne compte pas) ; il change à chaque démarrage (test `le_code_change_a_chaque_demarrage`). Il n'est écrit que dans le journal du service.
- [x] L'assistant crée le compte administrateur, enregistre l'adresse et Google (secret chiffré, vérifié en base), ouvre la session et demande la relance ; après la relance, la configuration vient de la base et le compte se connecte. Test `l_assistant_cree_l_administrateur_et_relance_dashu`. Dans le navigateur : la session a survécu à la relance (tableau de bord de Perso), adresse proposée d'après `location.origin`, adresse de retour Google à jour, aucune erreur de CSP.
- [x] Une fois un compte créé, `/setup` et `/setup/code` répondent 404, l'API reste ouverte. Test `une_fois_un_compte_cree_setup_repond_404`, et en CI.
- [x] Sans `SESSION_SECRET`, le secret est créé une fois (fichier `secret`, 600), gardé au redémarrage ; une variable donnée l'emporte sans toucher au fichier. Test `sans_session_secret_le_secret_est_cree_une_fois`. `dashu-backup` le copie avec la base (relu, pas essayé).
- [x] Une variable d'environnement prime sur la base (adresse, adresses autorisées, Google). Test `l_environnement_prime_sur_la_base`. Tous les tests existants, écrits comme le serveur de Rémi (tout dans l'environnement), passent sans changement.
- [ ] `docker run` de l'image mène au même assistant : à vérifier sur l'image publiée (le binaire est le même que celui testé).
- [x] L'installateur ne pose plus que les questions du système, puis affiche l'adresse et le code (vérifié en CI, le code est lu dans ce qu'il affiche).
- Sous Windows (développement), dashu ne peut pas se remplacer lui-même : après l'assistant, il s'arrête et demande d'être relancé.
- Migration 0021 : `instance_settings`, `allowed_emails`, `users.is_admin` (le plus ancien compte devient administrateur).

**Étape 5, carte Instance dans Réglages** (24 novembre 2026). Vérifiée par les tests (`tests/instance.rs`, 4 tests ; toute la suite passe) et dans le navigateur, sur une instance installée par l'assistant : carte en trois parties, adresse autorisée ajoutée, message « redémarre », liste à jour avant même la relance. **Pas encore sur le serveur.**

- [x] La carte n'apparaît que pour l'administrateur ; ses cinq routes répondent 404 aux autres comptes, sans rien changer. Test `seul_l_administrateur_voit_et_change_l_instance`.
- [x] Adresse, Google et adresses autorisées s'y changent, puis dashu se relance ; après la relance, tout vient de la base et une adresse retirée ne se connecte plus. Une valeur de l'environnement s'affiche verrouillée et ne se change pas (message « défini sur le serveur », rien en base, pas de relance). Tests `adresse_google_et_adresses_se_changent_puis_dashu_se_relance`, `l_environnement_verrouille_ses_reglages`.
- [x] Ni sa propre adresse, ni celle de l'API, ni la dernière ne se retirent ; adresse, email ou client Google invalides sont refusés sans relance. Test `garde_fous`.
- [x] Le secret Google n'est jamais réaffiché, ni dans la carte ni dans la page (vérifié par le test), et il est chiffré en base.
- Sur le serveur de Rémi (tout dans `dashu.env`), la carte est en lecture seule : adresse, Google et adresses « définis sur le serveur ».

**Étape 6, page d'accueil** (24 novembre 2026, PR #78 vers `main`). La section Installer de dashu.dev donne la commande du script et l'image Docker, avec le premier lancement et la licence.

- [x] Publiée sur dashu.dev (vérifié en ligne). La publication échouait depuis le 23 novembre : `deploy-pages` ne retrouvait plus le site envoyé dans le même job (« Found 0 artifact(s) »), alors qu'il était bien dans le run. Corrigé en deux jobs, envoi puis publication, comme le modèle de GitHub (PR #81) ; le droit `actions: read` ajouté avant (PR #80) ne suffisait pas.
- [ ] La commande et l'image de la page marchent telles quelles chez un visiteur : la version stable existe (ce qu'elles téléchargent répond) ; reste un essai sur une machine neuve.

## Outils par espace : avancement

Cahier des charges dans `SPEC.md` (« Outils par espace »), demandé par Rémi le 25 novembre 2026. PR 1 (outils par espace) : branche `outils-par-espace`, partie de `v1`, **pas encore fusionnée ni essayée sur le serveur**. PR 2 (1Life) : branche `outil-1life`, partie de la branche de la PR 1, pas encore fusionnée.

- [x] Registre unique `src/registry.rs` : la barre latérale, `/go`, les réglages et l'API en sont tirés ; la barre du téléphone le suit. Tests unitaires `registre_coherent`, `valeurs_par_defaut_et_reglages`, et test `chaque_outil_du_registre_apparait_partout`, qui parcourt le registre.
- [x] Masquer Fichiers dans École le retire de la barre latérale et de `/go` d'École seulement ; réaffiché, il revient avec ses fichiers. Test `masquer_puis_reafficher_un_outil_sans_rien_perdre`. Notes masqué retire aussi « Nouvelle note » et les raccourcis du téléphone qui en dépendent : `notes_masquees_retirent_aussi_les_raccourcis_qui_en_dependent`.
- [x] Adresse d'un outil masqué : retour à l'accueil de l'espace, message « Cet outil est masqué dans cet espace. » et lien vers les réglages (le message reste 8 secondes au lieu de 4 quand il porte un lien). Test `adresse_d_un_outil_masque_ramene_a_l_accueil`.
- [x] Réglages indépendants d'un espace à l'autre et d'un compte à l'autre, espace partagé compris : `reglages_independants_d_un_espace_et_d_un_compte_a_l_autre`.
- [x] Le dashboard ne se masque pas, un outil inconnu est refusé : `l_accueil_ne_se_masque_pas_et_un_outil_inconnu_est_refuse`. Interrupteur htmx (bloc de l'espace et barre latérale par `hx-swap-oob`) : `interrupteur_htmx_renvoie_le_bloc_et_la_barre_laterale`.
- [x] Migration 0023 (`space_tools`) : elle n'écrit rien, tout reste affiché. Test `la_migration_ne_masque_rien`.
- [x] API : notes créées et listées dans un espace où Notes est masqué ; `GET` et `PUT /api/v1/spaces/{slug}/tools`, refus sans rien écrire. Test `l_api_travaille_sur_un_outil_masque_et_regle_les_outils`. Connecteur Claude : `lire_outils_espace`, `modifier_outils_espace` (62 outils en tout), test `le_connecteur_claude_lit_et_regle_les_outils`. Hermès : `tools`, `tools-set`, skill 1.5.0, vérifiés par une relecture seulement.
- [x] Vu dans le navigateur, sur ordinateur, avec une copie locale du binaire : l'interrupteur s'enregistre, la barre latérale perd Fichiers sans rechargement, `/e/perso/files` ramène à l'accueil avec le message et son lien, aucune erreur dans la console.
- [ ] Sur iPhone, et sur le serveur après `dashu-update`.

**PR 2, 1Life réservé :**

- [x] Compte non autorisé, ou sans session : 404 sur `/1life`, `/1life/cr`, `/1life/audit`, `/1life/clients`, avec ou sans l'espace dans l'adresse, la même page qu'une adresse inconnue ; 1Life absent des réglages, de `/go` et de la barre latérale ; le formulaire des réglages répond 404. Test `un_compte_non_autorise_recoit_404_partout`.
- [x] `dashu tools grant`, `revoke` et `list`, joués avec le vrai binaire sur la base du test ; compte inconnu, outil non réservé ou inconnu refusés. Test `grant_puis_revoke_par_la_commande`.
- [x] Compte autorisé : 1Life masqué par défaut, affiché dans Taff seulement ; l'accueil, ses trois cartes « Bientôt » et les trois pages « Module à venir » s'ouvrent. Test `un_compte_autorise_active_1life_par_espace`.
- [x] Aucune route ne donne l'accès ; l'API traite 1Life comme un outil inconnu pour un compte non autorisé. Test `l_api_ne_donne_pas_l_acces_et_traite_1life_comme_un_outil_inconnu`.
- Migration 0024 (`tool_grants`). Aucune logique métier, aucun nom de client.
- Icône de 1Life (25 novembre 2026) : le logo fourni par Rémi, redessiné comme les icônes Lucide (grille de 24, deux traits ronds sans remplissage), symbole `1life` du sprite, ajouté par `scripts/build-assets.mjs`. Le `npm run build` local s'arrête avant le sprite (paquets ProseMirror absents de `node_modules`) : le symbole a été posé dans `static/icons.svg` à l'identique de ce que produit le script.
- [ ] Sur le serveur : `dashu tools grant` pour le compte de Rémi, après `dashu-update`.

## Écarts par rapport au cahier des charges

- **Déploiement sans Docker.** La spec prévoyait Docker Compose (dashu + `cloudflared`). dashu tourne en binaire natif sous systemd dans un LXC, avec `cloudflared` installé comme service à côté : une couche de moins sur un vieux serveur. Le `Dockerfile` et le `docker-compose.yml` restent maintenus (l'image est construite par la CI) et décrits dans le README comme variante.
- **L'application est sur `app.dashu.dev`**, pas sur `dashu.dev` : `dashu.dev` garde la page d'accueil publique du projet (GitHub Pages).
- **Accès supplémentaire par Tailscale** (`tailscale serve`), qui marche même si Cloudflare tombe.
- **Google publié** (17 novembre 2026, par Rémi) : le projet Google de dashu est passé du mode Test à la production. N'importe quel compte Google passe désormais l'écran de Google, et `ALLOWED_EMAILS` reste la seule barrière (le mode Test en ajoutait une seconde, la liste des utilisateurs tests). C'est le préalable à l'écriture dans Google Agenda, envisagée : en mode Test, le jeton d'accès à l'agenda expirerait au bout de 7 jours. `dashu-compte ajouter` ne demande plus d'ajouter l'adresse aux utilisateurs tests.
- **Ajouts hors spec**, faits à la demande après la v1 de base : API JSON pour un assistant (Hermès) avec une administration SSH limitée à six actions, agenda de Perso qui réunit tous les espaces, événements de plus de 24 h dans le bandeau « journée », « île » de changement d'espace avec balayage, barre de raccourcis sur téléphone, app installable sur l'écran d'accueil.
- **Nom affiché « Hermes Dashboard »** depuis le 12 novembre 2026, avec le logo Hermes (barre latérale, connexion où il se déplie une fois, onglets, icônes, favicon). Seul ce qui est visible a changé : le code, le binaire et les commandes gardent le nom dashu.
  - **Écran de lancement** (12 novembre 2026) : au chargement complet d'une page connectée (ouverture de l'app, adresse tapée, rechargement), fond noir et logo Hermes/ qui se replie en H/, puis l'écran s'efface sur la page en 1,5 s. Rendu par le serveur et animé en CSS, il s'affiche dès le premier rendu sans attendre le JavaScript. Jamais pendant une navigation htmx (en-tête `HX-Request`), caché si le système demande de réduire les animations. Test `ecran_de_lancement_au_chargement_seulement`. Vérifié dans le navigateur, sur ordinateur et en taille téléphone ; pas encore sur le serveur ni sur l'iPhone.
- **Espaces libres** (12 novembre 2026) : on crée un espace (8 au plus, couleur et icône au choix), on le renomme, on le supprime depuis Réglages. Avant la suppression, une page dit ce que l'espace contient (fichiers, notes, tâches, agendas) et propose de tout transférer dans un autre espace ; sinon tout disparaît, fichiers effacés du disque. Le slug d'un espace renommé ne change pas (API, Hermès). Tests `tests/spaces.rs`.
  - La migration 0007 reconstruit la table `spaces` (ses contraintes limitaient le slug et la couleur). SQLite l'exige clés étrangères coupées, sinon tout le contenu partirait en cascade : dashu lance donc ses migrations sur une connexion à part, sans clés étrangères, puis vérifie `foreign_key_check` (`src/db.rs`). Test `migration_0007_garde_tout_le_contenu` sur une base en service simulée.
  - Un espace supprimé ne revient plus à la connexion suivante : les trois espaces de départ ne sont créés que pour un compte qui n'en a aucun.
  - Perso ne se supprime pas (13 novembre 2026) : c'est la vue d'ensemble, son agenda réunit ceux de tous les espaces, sur la page Agenda, dans le widget « Prochains cours » et dans l'API (`space=perso`). Renommé, il garde ce rôle. La migration 0008 le rend, vide, à un compte qui l'avait supprimé.
- **Dashboard modulaire** (12 novembre 2026) : grille bento propre à chaque espace (colonne `spaces.dashboard`, migration 0009). Six widgets : Agenda en grille (calendrier : aujourd'hui, trois jours ou la semaine selon la largeur, ouvert sur l'heure en cours) et en liste (prochains cours), Tâches, Notes, Fichiers, Réglages rapides, en S, M, L ou toute taille jusqu'à 4 × 4 cases ; on les déplace, agrandit, ajoute et retire en mode édition (`static/js/dashboard.js`, sans bibliothèque ni style en ligne : la taille passe par des classes). La disposition est enregistrée à chaque changement (`POST /dashboard/layout`). Le balayage entre espaces est coupé pendant l'édition. Tests `tests/dashboard.rs`.
- **Profil et affichage** (12 novembre 2026) : prénom, nom, photo de profil (initiales sur un fond tiré au hasard sans photo), taille de l'interface (zoom de toute l'app, les unités d'écran divisées par `--zoom`), barre latérale ancrée ou détachée (façon mode compact de Zen : cachée à gauche, visible au survol du bord). Réglages passe en bas de la barre latérale, le thème ne se choisit plus que dans Réglages. Migration 0010, tests `tests/profile.rs`. Les widgets du dashboard ne défilent plus : ce qui dépasse est retiré à l'arrivée ; le widget Calendrier montre les heures qui tiennent autour de l'heure en cours. Corrigé le 15 novembre 2026 : un petit widget qui a son propre compteur (Fichiers récents : « 5 fichiers dans cet espace ») perdait tous ses éléments, le compteur et « + N de plus » prenant la place ; au moins un élément reste maintenant, et le compteur du widget remplace « + N de plus ». Vérifié dans le navigateur (instance locale), sans erreur de console.
- **Réglages communs à tous les espaces** (17 novembre 2026, demandé par Rémi) : dans Réglages (page principale, Membres, suppression d'un espace), plus d'île pour changer d'espace, donc plus de balayage non plus, et un accent neutre (le texte : noir en thème clair, blanc en thème sombre) au lieu de la couleur de l'espace actif. Les espaces gardent leur couleur dans la liste. Test `tests/reglages_neutres.rs` ; les tests de `tests/spaces.rs` vérifient l'espace actif sur le dashboard. Vu dans le navigateur (instance locale), sombre et clair, ordinateur et téléphone.
- **L'île centrée sur la fenêtre** (17 novembre 2026, demandé par Rémi) : avec la barre latérale ancrée, l'île se centrait sur la colonne du contenu, donc 120 px à droite du centre de l'écran ; avec la barre détachée ou sur téléphone, elle était au centre. Elle s'étend maintenant sur les deux colonnes de la grille et se centre sur toute la fenêtre dans tous les cas. Changement de CSS seul, sans test automatique possible (pas de mise en page dans `cargo test`). Mesuré dans le navigateur (instance locale) : écart de 0 px entre le centre de l'île et celui de la fenêtre sur le dashboard, Notes, Fichiers, Agenda et Outils, barre ancrée ou détachée, aux trois tailles d'interface, à 1 280 px, à 900 px (126 px d'écart avec la barre latérale) et sur téléphone ; elle reste collée en haut au défilement.
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
