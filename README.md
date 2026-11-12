<p align="center">
  <img src=".github/logo-hermes.svg" alt="Hermes Dashboard" width="560">
</p>

<p align="center">
  <strong>Hermes Dashboard</strong> · tableau de bord, notes, emploi du temps et fichiers, auto-hébergés
</p>

<p align="center">
  <a href="https://dashu.dev">dashu.dev</a> ·
  <a href="https://github.com/moreauremi/dashu/tree/v1">le code (branche v1)</a> ·
  <a href="https://github.com/moreauremi/dashu/blob/v1/README.md">la documentation</a>
</p>

---

Hermes Dashboard réunit un tableau de bord, des notes Markdown, l'emploi du temps Pronote et des fichiers, rangés par espaces derrière une seule connexion. Il tient dans **un seul binaire Rust**, hébergé à la maison.

Le nom de code du projet reste **dashu** : c'est le nom du dépôt, du binaire et des commandes (`dashu set-password`, `dashu-update`…).

## Où en est le projet

- **v1 terminée et en service.** Tous les critères d'acceptation sont vérifiés, la plupart sur le serveur réel. L'application sert au quotidien, sur ordinateur et sur iPhone.
- **v2 en cours : les fichiers.** L'envoi, la compression et l'aperçu sont en service depuis le 11 novembre 2026. La suite : tags et recherche, partage par lien temporaire, images dans les notes, onglet Outils.

Le détail, critère par critère, est dans [STATUS.md](https://github.com/moreauremi/dashu/blob/v1/STATUS.md). Le cahier des charges est dans [SPEC.md](https://github.com/moreauremi/dashu/blob/v1/SPEC.md).

## Ce que ça fait

| Module | Contenu |
| --- | --- |
| Espaces | Le contenu est rangé par espaces (par exemple Perso, École, Entreprise), chacun avec sa couleur et son icône, jusqu'à 8 en tout. On passe de l'un à l'autre en un clic, ou par balayage sur téléphone. Perso sert de vue d'ensemble : son agenda réunit ceux de tous les espaces. |
| Dashboard | Prochains cours, tâches du jour, notes récentes. Chaque widget se charge seul : un widget lent ne retarde pas les autres. |
| Notes | Éditeur Markdown avec aperçu en direct, sauvegarde automatique, tags, épinglage, recherche plein texte qui ignore les accents. |
| Agenda | Flux iCal de Pronote importé par son lien, resynchronisé toutes les heures, en vue jour ou semaine. |
| Fichiers (v2) | Envoi par glisser-déposer, jusqu'à 2 Go par fichier. Vignettes et aperçu des photos (HEIC comprises), vidéos, PDF et textes. Compression des vidéos, photos et PDF en une copie plus légère. |
| Connexion | Mot de passe ou Google, sur le même compte. Pas d'inscription publique : seuls les emails autorisés entrent. |
| API | Une API JSON, désactivée par défaut et protégée par une clé, pour qu'un assistant gère les tâches et les notes et lise l'agenda. |

`Ctrl+K` ouvre une palette de commandes qui cherche dans les notes et mène à chaque page. Sur iPhone, l'application s'installe sur l'écran d'accueil, comme une app.

## Sous le capot

Rust (axum), SQLite, htmx et askama. Les pages sont rendues par le serveur et htmx s'occupe de l'interactivité. Pas de framework JavaScript : htmx et l'éditeur CodeMirror, rien d'autre.

Mesures de la v1 sur le serveur réel : Lighthouse mobile de 99 à 100, 7,6 Mo de mémoire, 0,5 ms de réponse serveur, image Docker de 7 Mo.

**Sécurité.** Mots de passe en Argon2id, connexion Google par OpenID Connect avec PKCE, protection CSRF, limite de tentatives de connexion, Content-Security-Policy stricte (aucun script en ligne). Chaque requête SQL filtre sur l'utilisateur connecté, et des tests vérifient qu'un second compte ne voit rien du premier. Un fichier envoyé n'est jamais exécuté dans la page. Les conversions tournent dans un service à part, sans réseau, qui ne voit ni la base ni les secrets.

**Hébergement.** Un conteneur LXC Debian sur un serveur Proxmox, le binaire sous systemd. On y accède par un tunnel Cloudflare et par Tailscale, sans ouvrir aucun port sur la box. La base est sauvegardée et vérifiée chaque nuit, et la restauration a été testée sur le serveur.

## Installer

Le code de l'application est sur la branche **`v1`** (la branche `main` ne contient que cette page et le site). Il faut [Rust](https://rustup.rs) 1.85 ou plus récent et un compilateur C.

```bash
git clone -b v1 https://github.com/moreauremi/dashu.git
```

```bash
cd dashu
```

```bash
cp .env.example .env
```

Renseigner `SESSION_SECRET`, `ALLOWED_EMAILS` et `BASE_URL` dans `.env`, créer le compte, puis lancer le serveur, qui répond sur <http://localhost:3000> :

```bash
cargo run -- set-password toi@example.com
```

```bash
cargo run
```

Le [README de la branche v1](https://github.com/moreauremi/dashu/blob/v1/README.md) détaille tout le reste : les variables d'environnement, le déploiement sur un serveur (LXC, systemd, Tailscale, tunnel Cloudflare), la variante Docker, les sauvegardes et la restauration, la connexion Google et l'API.

## Feuille de route

Une version à la fois, utilisée au quotidien avant de passer à la suivante.

| Version | Contenu | État |
| --- | --- | --- |
| v1 | Espaces, dashboard, notes Markdown, agenda Pronote, API pour un assistant | En service |
| v2 | Fichiers par espace, partage par lien temporaire, onglet Outils (conversions) | En cours |
| v3 | OCR : texte des images et des PDF scannés | Prévue |
| v4 | Médias : retouche et détourage de photos dans le navigateur | Prévue |
| v5 | Audio : lecture des notes à voix haute, transcription des vocaux et des cours | Prévue |
| v6 | IA : un assistant qui cherche dans les notes et les fichiers | Prévue |

## Branches du dépôt

| Branche | Contenu |
| --- | --- |
| [`v1`](https://github.com/moreauremi/dashu/tree/v1) | L'application, ses tests et sa documentation. Elle reçoit toutes les évolutions, v2 comprise. |
| `main` | Cette page et `website/`, la page d'accueil de [dashu.dev](https://dashu.dev), publiée par GitHub Pages à chaque push. |

Pour voir la page d'accueil en local :

```bash
python3 -m http.server -d website 8000
```

Puis ouvrir <http://localhost:8000>.

## Licence

À choisir avant la première version publique (MIT ou AGPL).
