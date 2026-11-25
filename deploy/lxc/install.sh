#!/usr/bin/env bash
# Installe Hermes Dashboard (dashu) sur Linux, en posant les questions au fur
# et à mesure (SPEC, « Installation simple ») :
#
#   curl -fsSL https://raw.githubusercontent.com/moreauremi/dashu/v1/deploy/lxc/install.sh | sudo bash
#
# Le script télécharge le binaire tout fait de la dernière version, vérifie
# son empreinte SHA-256, crée l'utilisateur système dashu, sa configuration
# et ses services, puis rend dashu joignable : sur le réseau de la maison, par
# Tailscale (privé, ou ouvert à Internet par Funnel), ou par un tunnel
# Cloudflare vers un nom de domaine. Le compte et la connexion Google se
# règlent ensuite dans le navigateur : le script affiche l'adresse et le code
# d'installation (src/setup.rs).
#
# Relancé sur une machine déjà installée, il propose un menu : mise à jour,
# accès, fichiers, Google, mot de passe. La base n'est jamais touchée, et un
# changement de configuration qui empêche dashu de démarrer est annulé.
#
# Sans question (automatisation, tests) :
#
#   bash install.sh --non-interactif --email toi@example.com [options]
#   bash install.sh toi@example.com https://app.example.com [canal]   (ancienne forme)
#
#   --canal stable|v1       suite de versions suivie (stable par défaut)
#   --acces aucun|local     aucun : dashu n'écoute que sur la machine (on le
#                           publie soi-même) ; local : sur le réseau de la maison
#   --adresse <url>         adresse publique (BASE_URL)
#   --email <adresse>       compte à créer tout de suite, mot de passe lu dans
#                           DASHU_MOT_DE_PASSE (sinon : assistant du premier lancement)
#   --fichiers              active les fichiers (dans /srv/dashu)
#   --conversions           active aussi les outils de conversion (ffmpeg, pandoc…)
#   --archive <fichier>     installe cette archive au lieu de la télécharger
#   --depuis-les-sources [branche]   compile le code (Rust, 10 à 20 minutes)
#
# DASHU_TELECHARGEMENT remplace l'adresse du dossier qui contient les
# archives et SHA256SUMS (tests de la CI, tests/deploy/installation.sh).
#
# Tout le script est dans des fonctions, appelées par la dernière ligne :
# avec « curl … | bash », bash lit ainsi tout le script avant d'exécuter quoi
# que ce soit, et aucune commande ne peut avaler la suite du téléchargement.
set -euo pipefail

REPO_URL="https://github.com/moreauremi/dashu"
# Version de Typst, moteur PDF de pandoc (README, « Compression et
# conversions ») ; la CI la lit ici pour ses tests.
TYPST_VERSION=0.11.1
ENV_FILE=/etc/dashu/dashu.env
SHARE=/opt/dashu/share
export DEBIAN_FRONTEND=noninteractive
# Une langue présente sur toute Debian ou Ubuntu : sur une machine neuve, la
# session SSH en annonce souvent une que le système n'a pas (en_US.UTF-8,
# fr_FR.UTF-8), et apt comme perl remplissent alors l'écran d'avertissements
# (« Setting locale failed ») sans conséquence. Les accents restent justes.
export LC_ALL=C.UTF-8 LANG=C.UTF-8
unset LANGUAGE
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH"

# ---------- Affichage ----------

if [ -t 1 ]; then
  GRAS=$'\033[1m' JAUNE=$'\033[33m' ROUGE=$'\033[31m' VERT=$'\033[32m' NORMAL=$'\033[0m'
else
  GRAS="" JAUNE="" ROUGE="" VERT="" NORMAL=""
fi
say() { printf '\n%s==> %s%s\n' "$GRAS" "$*" "$NORMAL"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '%s[!]%s %s\n' "$JAUNE" "$NORMAL" "$*" >&2; }
die() {
  printf '%s[x]%s %s\n' "$ROUGE" "$NORMAL" "$*" >&2
  exit 1
}

# ---------- Questions (elles lisent le terminal, jamais l'entrée du script) ----------

# demander VAR "question" [défaut]
demander() {
  local reponse=""
  printf '%s%s%s : ' "$GRAS" "$2" "${3:+ [$3]}$NORMAL" >/dev/tty
  IFS= read -r reponse </dev/tty || reponse=""
  printf -v "$1" '%s' "${reponse:-${3:-}}"
}

# demander_secret VAR "question" : rien ne s'affiche pendant la frappe.
demander_secret() {
  local reponse=""
  printf '%s%s%s : ' "$GRAS" "$2" "$NORMAL" >/dev/tty
  IFS= read -rs reponse </dev/tty || reponse=""
  printf '\n' >/dev/tty
  printf -v "$1" '%s' "$reponse"
}

# oui_non "question" o|n : vrai si la réponse est oui.
oui_non() {
  local r
  while :; do
    demander r "$1 (o/n)" "$2"
    case "${r,,}" in
      o | oui | y | yes) return 0 ;;
      n | non | no) return 1 ;;
    esac
    printf 'Réponds par o ou n.\n' >/dev/tty
  done
}

# Une adresse simple : lettres, chiffres et . _ % + - avant l'arobase.
email_valide() { [[ "$1" =~ ^[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}$ ]]; }

# Un nom de machine : app.mondomaine.fr
nom_valide() { [[ "$1" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]]; }

demander_email() {
  while :; do
    demander EMAIL "Ton adresse email (elle sert d'identifiant)"
    EMAIL="$(printf '%s' "$EMAIL" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
    email_valide "$EMAIL" && return
    printf 'Adresse invalide.\n' >/dev/tty
  done
}

# Le mot de passe reste dans une variable du script ; il part vers dashu par
# un tube (printf est une commande interne de bash : il n'apparaît jamais
# dans la liste des processus).
demander_mot_de_passe() {
  local encore
  while :; do
    demander_secret MOT_DE_PASSE "Mot de passe (12 caractères au moins)"
    if [ "${#MOT_DE_PASSE}" -lt 12 ]; then
      printf 'Trop court : 12 caractères au moins.\n' >/dev/tty
      continue
    fi
    demander_secret encore "Le même, encore une fois"
    [ "$MOT_DE_PASSE" = "$encore" ] && return
    printf 'Les deux ne sont pas identiques.\n' >/dev/tty
  done
}

# ---------- Machine ----------

verifier_machine() {
  [ "$(id -u)" -eq 0 ] || die "À lancer en root : curl -fsSL … | sudo bash"
  [ "$(uname -s)" = Linux ] || die "Ce script installe dashu sur Linux. Sur macOS ou Windows : Docker (README, « Variante : Docker »)."
  [ -d /run/systemd/system ] || die "systemd est nécessaire et ne tourne pas ici (conteneur Docker, WSL sans systemd…). Docker reste possible (README, « Variante : Docker »)."
  command -v apt-get >/dev/null || die "Ce script vise Debian, Ubuntu et leurs dérivées (apt). Ailleurs : Docker (README, « Variante : Docker »)."
  case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    *) die "Processeur $(uname -m) non pris en charge : x86_64 ou aarch64 (ARM 64 bits) seulement." ;;
  esac
}

# dashu répond-il sur la machine ?
repond() {
  for _ in $(seq 1 20); do
    curl -fsS --max-time 2 http://127.0.0.1:3000/health >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

adresse_ip() { hostname -I 2>/dev/null | awk '{print $1}'; }

paquets_de_base() {
  say "Paquets du système"
  apt-get update -q
  apt-get install -y -q curl ca-certificates sqlite3 rsync jq tar gzip xz-utils >/dev/null
}

# ---------- Version à installer ----------

# Adresse du dossier qui contient les archives du canal.
url_versions() {
  if [ -n "${DASHU_TELECHARGEMENT:-}" ]; then
    printf '%s' "${DASHU_TELECHARGEMENT%/}"
    return
  fi
  case "$CANAL" in
    stable) printf '%s/releases/latest/download' "$REPO_URL" ;;
    v1) printf '%s/releases/download/canal-v1' "$REPO_URL" ;;
    *) die "Canal inconnu : $CANAL (stable ou v1)." ;;
  esac
}

# Télécharge l'archive, la vérifie et l'extrait dans $TRAVAIL/dashu.
obtenir_archive() {
  local nom="dashu-$ARCH-linux.tar.gz" base
  if [ -n "$ARCHIVE" ]; then
    say "Archive locale $ARCHIVE"
    cp "$ARCHIVE" "$TRAVAIL/$nom"
  else
    base="$(url_versions)"
    say "Téléchargement de dashu ($CANAL, $ARCH)"
    curl -fsSL --retry 3 -o "$TRAVAIL/SHA256SUMS" "$base/SHA256SUMS" ||
      die "Téléchargement impossible : $base/SHA256SUMS (pas encore de version publiée sur ce canal ?)."
    curl -fsSL --retry 3 -o "$TRAVAIL/$nom" "$base/$nom" || die "Téléchargement impossible : $base/$nom."
    (cd "$TRAVAIL" && grep "  $nom\$" SHA256SUMS | sha256sum -c --quiet -) ||
      die "L'archive ne correspond pas à son empreinte (SHA256SUMS) : rien n'est installé."
    info "Empreinte SHA-256 vérifiée."
  fi
  tar -xzf "$TRAVAIL/$nom" -C "$TRAVAIL"
  [ -x "$TRAVAIL/dashu/dashu" ] || die "Archive incomplète : le binaire dashu manque."
  info "$("$TRAVAIL/dashu/dashu" version)"
}

# Ancienne façon : le code de la branche, compilé sur place.
compiler() {
  local branche="$1"
  say "Compilation depuis les sources (branche $branche, 10 à 20 minutes)"
  apt-get install -y -q git build-essential pkg-config >/dev/null
  export RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo
  export PATH="/opt/rust/cargo/bin:$PATH"
  command -v cargo >/dev/null ||
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
  install -d /opt/dashu
  [ -d /opt/dashu/src/.git ] || git clone -q --depth 1 --branch "$branche" "$REPO_URL.git" /opt/dashu/src
  (cd /opt/dashu/src && DASHU_COMMIT="$(git rev-parse HEAD)" nice -n 10 cargo build --release --locked)
  mkdir -p "$TRAVAIL/dashu"
  cp /opt/dashu/src/target/release/dashu "$TRAVAIL/dashu/dashu"
  cp -r /opt/dashu/src/deploy /opt/dashu/src/LICENSE "$TRAVAIL/dashu/"
}

# Binaire dans /usr/local/bin, le reste de l'archive dans /opt/dashu/share
# (services, scripts, deploy/hermes), remplacé d'un coup.
installer_archive() {
  install -d /opt/dashu
  install -m 755 "$TRAVAIL/dashu/dashu" /usr/local/bin/.dashu.new
  mv -f /usr/local/bin/.dashu.new /usr/local/bin/dashu
  rm -rf "$SHARE.new" "$SHARE.old"
  cp -a "$TRAVAIL/dashu" "$SHARE.new"
  rm -f "$SHARE.new/dashu"
  [ -d "$SHARE" ] && mv "$SHARE" "$SHARE.old"
  mv "$SHARE.new" "$SHARE"
  rm -rf "$SHARE.old"
  printf '%s\n' "$CANAL" >/etc/dashu/branch
  if [ -n "$SOURCES" ]; then echo sources >/etc/dashu/source; else echo binaire >/etc/dashu/source; fi
  /usr/local/bin/dashu version | awk '{print $3}' >/etc/dashu/deployed-commit
}

# ---------- Accès ----------

choisir_acces() {
  local choix
  ACCES_LOCAL="" ACCES_TS="" ACCES_FUNNEL="" ACCES_CF=""
  cat >/dev/tty <<TEXTE

${GRAS}Comment veux-tu accéder à dashu ?${NORMAL} Un ou plusieurs numéros, séparés par des espaces.

  1) Sur le réseau de la maison seulement : http://$(adresse_ip):3000
     Rien à créer, mais pas de chiffrement : le mot de passe passe en clair sur le réseau.
  2) Tailscale : depuis tes appareils, où que tu sois, en HTTPS.
     Il faut un compte Tailscale (gratuit) et Tailscale sur chaque appareil.
  3) Tailscale Funnel : une adresse publique en HTTPS, sans acheter de nom de domaine.
     Ouverte à tout Internet (la page de connexion aussi).
  4) Ton nom de domaine, par un tunnel Cloudflare : https://app.mondomaine.fr
     Il faut un domaine géré par Cloudflare (offre gratuite) et un tunnel.

TEXTE
  while :; do
    demander choix "Ton choix" 1
    ACCES_LOCAL="" ACCES_TS="" ACCES_FUNNEL="" ACCES_CF=""
    local ok=1 c
    for c in $choix; do
      case "$c" in
        1) ACCES_LOCAL=1 ;;
        2) ACCES_TS=1 ;;
        3) ACCES_TS=1 ACCES_FUNNEL=1 ;;
        4) ACCES_CF=1 ;;
        *) ok="" ;;
      esac
    done
    [ -n "$ok" ] && [ -n "$choix" ] && break
    printf 'Des numéros entre 1 et 4, par exemple « 2 » ou « 1 2 ».\n' >/dev/tty
  done
  if [ -n "$ACCES_CF" ]; then
    while :; do
      demander CF_NOM "Adresse de dashu sur ton domaine (par exemple app.mondomaine.fr)"
      CF_NOM="$(printf '%s' "$CF_NOM" | tr '[:upper:]' '[:lower:]')"
      CF_NOM="${CF_NOM#https://}"
      CF_NOM="${CF_NOM%%/*}"
      nom_valide "$CF_NOM" && break
      printf 'Nom invalide.\n' >/dev/tty
    done
    cat >/dev/tty <<TEXTE

Dans Cloudflare : Zero Trust > Réseaux > Tunnels > Créer un tunnel > Cloudflared.
Choisis Debian : la commande affichée contient le jeton du tunnel (la longue
suite de caractères après « service install »). Copie seulement le jeton.
TEXTE
    while :; do
      demander_secret CF_JETON "Jeton du tunnel (il ne s'affiche pas)"
      CF_JETON="$(printf '%s' "$CF_JETON" | tr -d '[:space:]')"
      [[ "$CF_JETON" =~ ^[A-Za-z0-9=_.+/-]{50,}$ ]] && break
      printf 'Ce n'"'"'est pas un jeton de tunnel (une longue suite de lettres et de chiffres).\n' >/dev/tty
    done
  fi
}

configurer_tailscale() {
  say "Tailscale"
  if [ ! -c /dev/net/tun ]; then
    warn "/dev/net/tun manque : Tailscale ne peut pas tourner ici."
    if [ "$(systemd-detect-virt -c 2>/dev/null || true)" = lxc ]; then
      info "C'est un conteneur LXC. Sur l'hôte Proxmox, ajoute ces deux lignes à /etc/pve/lxc/<numéro>.conf :"
      info "  lxc.cgroup2.devices.allow: c 10:200 rwm"
      info "  lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file"
      info "puis redémarre le conteneur (pct reboot <numéro>) et relance ce script (choix « Changer l'accès »)."
    fi
    return 1
  fi
  if ! command -v tailscale >/dev/null; then
    info "Installation de Tailscale (script officiel de tailscale.com)."
    curl -fsSL https://tailscale.com/install.sh | sh >/dev/null
  fi
  if ! tailscale status >/dev/null 2>&1; then
    # Le nom de la machine (« dashu » sur le serveur, « dashu-test » pour un
    # essai à côté) : deux installations ne se disputent pas le même nom.
    info "Ouvre le lien qui va s'afficher pour relier cette machine ($(hostname -s)) à ton compte Tailscale."
    tailscale up --hostname "$(hostname -s)"
  fi
  TS_NOM="$(tailscale status --json | jq -r '.Self.DNSName // empty' | sed 's/\.$//')"
  [ -n "$TS_NOM" ] || {
    warn "Tailscale ne donne pas de nom à cette machine."
    return 1
  }
  if [ "$(tailscale status --json | jq -r '.CurrentTailnet.MagicDNSEnabled // false')" != true ]; then
    warn "MagicDNS est coupé : active-le dans la console Tailscale (DNS > MagicDNS), sinon $TS_NOM ne sera pas trouvé."
  fi
  info "Si Tailscale affiche un lien pour activer HTTPS$([ -n "$ACCES_FUNNEL" ] && printf ' ou Funnel'), ouvre-le : le script attend."
  if [ -n "$ACCES_FUNNEL" ]; then
    tailscale funnel --bg 3000
  else
    tailscale serve --bg 3000
  fi
  info "dashu est publié sur https://$TS_NOM"
}

configurer_cloudflare() {
  say "Tunnel Cloudflare"
  if ! command -v cloudflared >/dev/null; then
    info "Installation de cloudflared (dépôt officiel de Cloudflare)."
    install -d -m 755 /usr/share/keyrings
    curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg -o /usr/share/keyrings/cloudflare-main.gpg
    echo "deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main" \
      >/etc/apt/sources.list.d/cloudflared.list
    apt-get update -q
    apt-get install -y -q cloudflared >/dev/null
  fi
  # Le jeton reste dans un fichier lisible par root seul, lu par systemd :
  # jamais dans la ligne de commande (visible de tous par ps) ni dans l'unité.
  (umask 077 && printf 'TUNNEL_TOKEN=%s\n' "$CF_JETON" >/etc/dashu/tunnel.env)
  install -m 644 "$SHARE/deploy/lxc/dashu-tunnel.service" /etc/systemd/system/
  systemctl daemon-reload
  systemctl enable dashu-tunnel >/dev/null 2>&1
  systemctl restart dashu-tunnel
  cat >/dev/tty <<TEXTE

Dans Cloudflare, sur la page du tunnel, onglet « Routes publiées » (Public
Hostname), ajoute :  $CF_NOM  →  http://localhost:3000

TEXTE
  local _
  demander _ "Appuie sur Entrée quand c'est fait"
}

# BASE_URL et BIND_ADDR d'après les accès choisis. L'adresse principale est
# la plus ouverte ; les autres servent à se connecter par mot de passe.
adresse_principale() {
  BIND_ADDR=127.0.0.1:3000
  [ -n "$ACCES_LOCAL" ] && BIND_ADDR=0.0.0.0:3000
  if [ -n "$ADRESSE" ]; then
    BASE_URL="${ADRESSE%/}"
  elif [ -n "$ACCES_CF" ]; then
    BASE_URL="https://$CF_NOM"
  elif [ -n "$ACCES_TS" ] && [ -n "${TS_NOM:-}" ]; then
    BASE_URL="https://$TS_NOM"
  elif [ -n "$ACCES_LOCAL" ]; then
    BASE_URL="http://$(adresse_ip):3000"
  else
    BASE_URL="http://localhost:3000"
  fi
}

configurer_acces() {
  TS_NOM=""
  if [ -n "$ACCES_TS" ] && ! configurer_tailscale; then
    warn "Tailscale n'est pas en place : dashu reste joignable par les autres accès choisis."
    ACCES_TS="" ACCES_FUNNEL=""
    [ -n "$ACCES_LOCAL$ACCES_CF" ] || ACCES_LOCAL=1
  fi
  [ -n "$ACCES_CF" ] && configurer_cloudflare
  adresse_principale
}

# ---------- Configuration ----------

aleatoire_hex() { head -c "$1" /dev/urandom | od -An -tx1 | tr -d ' \n'; }

# Valeur d'une variable de dashu.env, sans guillemets.
valeur() {
  local ligne
  ligne="$(grep "^$1=" "$ENV_FILE" 2>/dev/null | tail -1 || true)"
  ligne="${ligne#"$1"=}"
  ligne="${ligne#\"}"
  printf '%s' "${ligne%\"}"
}

utilisateur_et_dossiers() {
  id dashu >/dev/null 2>&1 || useradd --system --home-dir /var/lib/dashu --shell /usr/sbin/nologin dashu
  install -d -o dashu -g dashu -m 750 /var/lib/dashu /var/backups/dashu
  install -d /etc/dashu
}

ecrire_configuration() {
  if [ -f "$ENV_FILE" ]; then
    info "Configuration existante gardée ($ENV_FILE)."
    return
  fi
  (
    umask 027
    {
      echo "SESSION_SECRET=$(aleatoire_hex 32)"
      # Sans email, la liste des adresses autorisées est rangée en base
      # (assistant du premier lancement, puis Réglages › Instance).
      if [ -n "$EMAIL" ]; then echo "ALLOWED_EMAILS=$EMAIL"; fi
      echo "BASE_URL=$BASE_URL"
      echo "DATABASE_URL=sqlite:///var/lib/dashu/dashu.db"
      echo "BIND_ADDR=$BIND_ADDR"
    } >"$ENV_FILE"
  )
  chown root:dashu "$ENV_FILE"
  chmod 640 "$ENV_FILE"
}

# changer_env NOM valeur [NOM valeur…] : remplace (ou ajoute) ces lignes de
# dashu.env, redémarre dashu et vérifie qu'il répond ; sinon remet l'ancien
# fichier, comme dashu-compte. Écriture dans un fichier voisin (droits
# copiés), puis renommage. Les valeurs passent par l'environnement d'awk, qui
# ne les réinterprète pas (contrairement à awk -v).
changer_env() {
  local avant tmp
  avant="$(mktemp /etc/dashu/.dashu.env.avant.XXXXXX)"
  cp -p "$ENV_FILE" "$avant"
  while [ $# -ge 2 ]; do
    tmp="$(mktemp /etc/dashu/.dashu.env.XXXXXX)"
    cp -p "$ENV_FILE" "$tmp"
    CLE="$1" VAL="$2" awk '
      index($0, ENVIRON["CLE"] "=") == 1 { if (!fait) print ENVIRON["CLE"] "=" ENVIRON["VAL"]; fait = 1; next }
      { print }
      END { if (!fait) print ENVIRON["CLE"] "=" ENVIRON["VAL"] }
    ' "$ENV_FILE" >"$tmp"
    mv -f "$tmp" "$ENV_FILE"
    shift 2
  done
  systemctl restart dashu
  if repond; then
    rm -f "$avant"
    return 0
  fi
  warn "dashu ne redémarre pas avec ce changement : l'ancienne configuration est remise (journalctl -u dashu -n 30)."
  mv -f "$avant" "$ENV_FILE"
  systemctl restart dashu || true
  return 1
}

# Fichiers dans /srv/dashu : un dossier fixe, celui que connaît le bac à sable
# du service (deploy/lxc/hardening.conf). Pour un autre disque, on le monte
# sur /srv/dashu avant (README, « Second disque »). Le service de conversion
# (même binaire, son propre compte) en a besoin dès les fichiers : les images
# se convertissent sans paquet de plus.
preparer_fichiers() {
  install -d -m 755 /srv/dashu
  install -d -o dashu -g dashu -m 750 /srv/dashu/fichiers
  getent group dashu-jobs >/dev/null || groupadd --system dashu-jobs
  id dashu-worker >/dev/null 2>&1 ||
    useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin --gid dashu-jobs dashu-worker
  usermod -a -G dashu-jobs dashu
  install -d -o dashu -g dashu-jobs -m 2770 /srv/dashu/tmp /srv/dashu/tmp/jobs
  if ! mountpoint -q /srv/dashu; then
    info "Les fichiers sont rangés dans /srv/dashu, sur le disque du système : la sauvegarde"
    info "des fichiers ne se fait que sur un volume monté sur /var/backups/dashu-fichiers (README)."
  fi
}

installer_conversions() {
  say "Outils de conversion (vidéo, photos HEIC, PDF, documents)"
  apt-get install -y -q ffmpeg ghostscript libheif-examples pandoc fonts-inter >/dev/null
  local actuelle cible="$ARCH-unknown-linux-musl"
  actuelle="$(typst --version 2>/dev/null | awk '{print $2}' || true)"
  if [ "$actuelle" != "$TYPST_VERSION" ]; then
    curl -fsSL "https://github.com/typst/typst/releases/download/v$TYPST_VERSION/typst-$cible.tar.xz" | tar -xJ -C "$TRAVAIL"
    install -m 755 "$TRAVAIL/typst-$cible/typst" /usr/local/bin/typst
  fi
  info "$(typst --version)"
}

# ---------- Services ----------

installer_scripts() {
  local script
  for script in dashu-update dashu-backup dashu-backup-files dashu-restore dashu-admin dashu-compte dashu-telechargeur; do
    [ -f "$SHARE/deploy/lxc/$script" ] || continue
    install -m 755 "$SHARE/deploy/lxc/$script" "/usr/local/sbin/.$script.new"
    mv -f "/usr/local/sbin/.$script.new" "/usr/local/sbin/$script"
  done
}

installer_services() {
  say "Services"
  local d="$SHARE/deploy/lxc"
  install -m 644 "$d/dashu.service" "$d/dashu-backup.service" "$d/dashu-backup.timer" \
    "$d/dashu-worker.service" "$d/dashu-tunnel.service" /etc/systemd/system/
  install -d /etc/systemd/system/dashu.service.d
  install -m 644 "$d/hardening.conf" /etc/systemd/system/dashu.service.d/
  installer_scripts
  systemctl daemon-reload
  systemctl enable dashu dashu-backup.timer >/dev/null 2>&1
  systemctl start dashu-backup.timer
  systemctl restart dashu
  if [ -n "$(valeur WORK_DIR)" ]; then
    systemctl enable dashu-worker >/dev/null 2>&1
    systemctl restart dashu-worker
  fi
  if ! repond; then
    warn "dashu ne démarre pas avec le durcissement systemd : nouvel essai sans."
    rm -f /etc/systemd/system/dashu.service.d/hardening.conf
    systemctl daemon-reload
    systemctl restart dashu
    repond || {
      journalctl -u dashu -n 30 --no-pager
      die "dashu ne démarre pas."
    }
  fi
  info "dashu répond sur la machine (127.0.0.1:3000)."
}

# Crée le compte (ou change son mot de passe) avec le mot de passe donné.
# Ce que dit dashu ne contient jamais le mot de passe : on ne le montre
# qu'en cas d'erreur.
definir_mot_de_passe() {
  local sortie
  if ! sortie="$(printf '%s\n' "$2" | runuser -u dashu -- sh -c 'set -a; . /etc/dashu/dashu.env; exec dashu set-password "$0"' "$1" 2>&1)"; then
    warn "$sortie"
    return 1
  fi
}

configurer_google() {
  local id secret
  case "$BASE_URL" in
    https://* | http://localhost*) ;;
    *)
      info "La connexion avec Google demande une adresse en HTTPS (Tailscale, Funnel ou Cloudflare)."
      return 0
      ;;
  esac
  oui_non "Activer la connexion avec Google (facultatif, réglable plus tard en relançant ce script) ?" n || return 0
  cat >/dev/tty <<TEXTE

Dans la console Google Cloud (https://console.cloud.google.com/apis/credentials) :
  1. Crée un projet, puis l'écran de consentement OAuth (audience « Externe »).
  2. Identifiants > Créer des identifiants > ID client OAuth > Application Web.
  3. URI de redirection autorisé :  $BASE_URL/auth/google/callback
Détails : README, « Créer les identifiants Google ».

TEXTE
  demander id "ID client (il finit par .apps.googleusercontent.com ; Entrée pour passer)"
  id="$(printf '%s' "$id" | tr -d '[:space:]')"
  [ -n "$id" ] || return 0
  demander_secret secret "Code secret du client (il ne s'affiche pas)"
  secret="$(printf '%s' "$secret" | tr -d '[:space:]')"
  [ -n "$secret" ] || return 0
  if changer_env GOOGLE_CLIENT_ID "$id" GOOGLE_CLIENT_SECRET "$secret"; then
    info "Connexion avec Google activée."
  fi
}

verifier_adresse() {
  if curl -fsS --max-time 10 "$BASE_URL/health" >/dev/null 2>&1; then
    info "$BASE_URL répond."
  else
    warn "$BASE_URL ne répond pas depuis cette machine. Ça peut venir du réseau (DNS pas encore à jour, route du tunnel à ajouter) : essaie depuis un autre appareil."
  fi
}

# Code d'installation que dashu vient d'écrire dans son journal (dernier
# démarrage du service seulement).
code_installation() {
  local code="" invocation
  for _ in $(seq 1 10); do
    invocation="$(systemctl show -p InvocationID --value dashu 2>/dev/null || true)"
    code="$(journalctl -o cat --no-pager ${invocation:+_SYSTEMD_INVOCATION_ID=$invocation} -u dashu 2>/dev/null |
      grep -oE '[2-9A-HJ-NP-Z]{4}-[2-9A-HJ-NP-Z]{4}-[2-9A-HJ-NP-Z]{4}' | tail -1 || true)"
    [ -n "$code" ] && break
    sleep 1
  done
  printf '%s' "$code"
}

resume() {
  local code
  say "${VERT}C'est prêt${NORMAL}"
  info "Adresse        : $BASE_URL"
  [ -n "${ACCES_LOCAL:-}" ] && [ "$BASE_URL" != "http://$(adresse_ip):3000" ] && info "Réseau local   : http://$(adresse_ip):3000"
  [ -n "${ACCES_TS:-}" ] && [ -n "${TS_NOM:-}" ] && [ "$BASE_URL" != "https://$TS_NOM" ] && info "Tailscale      : https://$TS_NOM"
  [ -n "${EMAIL:-}" ] && info "Compte         : $EMAIL"
  info "Version        : $(dashu version)"
  echo
  info "Mettre à jour         : sudo dashu-update   (ou relancer ce script)"
  info "Ajouter quelqu'un     : sudo dashu-compte ajouter <email>"
  info "Sauvegarde            : chaque nuit à 3 h 30 dans /var/backups/dashu ; restaurer : sudo dashu-restore"
  info "Journal               : journalctl -u dashu"
  if [ -n "${ACCES_LOCAL:-}" ]; then
    echo
    warn "Accès local : pas de chiffrement, le mot de passe passe en clair sur le réseau de la maison."
  fi
  if [ -n "${ACCES_CF:-}" ]; then
    echo
    info "Cloudflare : désactive l'injection de Web Analytics pour ce domaine (la CSP de dashu bloque"
    info "son script), et ne mets pas de défi anti-robot sur /mcp, /oauth/ et /.well-known/ (README)."
  fi
  if [ -z "$COMPTE_CREE" ]; then
    code="$(code_installation)"
    say "Dernière étape, dans ton navigateur"
    info "Ouvre  $BASE_URL/setup"
    info "et tape le code d'installation :  ${GRAS}${code:-(voir journalctl -u dashu)}${NORMAL}"
    info "Tu y crées ton compte, et la connexion avec Google si tu veux."
    info "Le code change à chaque démarrage de dashu ; journalctl -u dashu le redonne."
  fi
}

# ---------- Installation neuve ----------

installation_neuve() {
  if [ -n "$INTERACTIF" ]; then
    say "Installation de Hermes Dashboard (dashu)"
    info "Quelques questions, puis le script fait le reste (2 à 5 minutes)."
    choisir_acces
    if oui_non "Activer les fichiers (envoi, aperçu, partage par lien, dossiers) ?" o; then
      FICHIERS=oui
      oui_non "Activer aussi les conversions (vidéos, photos HEIC, PDF, documents ; environ 600 Mo de paquets) ?" n &&
        CONVERSIONS=oui
    fi
  else
    case "$ACCES" in
      "" | aucun) ;;
      local) ACCES_LOCAL=1 ;;
      *) die "--acces : aucun ou local sans question (Tailscale et Cloudflare demandent les questions)." ;;
    esac
    [ -z "$EMAIL" ] || email_valide "$EMAIL" || die "Adresse email invalide : $EMAIL"
    MOT_DE_PASSE="${DASHU_MOT_DE_PASSE:-}"
    unset DASHU_MOT_DE_PASSE
  fi

  paquets_de_base
  utilisateur_et_dossiers
  if [ -n "$SOURCES" ]; then compiler "$BRANCHE"; else obtenir_archive; fi
  installer_archive
  configurer_acces
  ecrire_configuration
  if [ "$FICHIERS" = oui ]; then
    preparer_fichiers
    grep -q '^FILES_DIR=' "$ENV_FILE" || echo 'FILES_DIR=/srv/dashu/fichiers' >>"$ENV_FILE"
    grep -q '^WORK_DIR=' "$ENV_FILE" || echo 'WORK_DIR=/srv/dashu/tmp' >>"$ENV_FILE"
  fi
  [ "$CONVERSIONS" = oui ] && installer_conversions
  installer_services

  if [ -n "$EMAIL" ] && [ -n "$MOT_DE_PASSE" ]; then
    definir_mot_de_passe "$EMAIL" "$MOT_DE_PASSE"
    MOT_DE_PASSE=""
    COMPTE_CREE=1
    info "Compte $EMAIL créé."
  fi
  verifier_adresse
  resume
}

# ---------- Machine déjà installée ----------

# Relance sans question : binaire, services et scripts remis à jour ; la
# configuration et la base ne bougent pas (comme l'ancien script).
relance_sans_question() {
  say "dashu est déjà installé : binaire, services et scripts remis à jour, configuration gardée"
  # Le canal suivi ne change que s'il est donné.
  [ -n "$CANAL_DONNE" ] || [ ! -f /etc/dashu/branch ] || CANAL="$(cat /etc/dashu/branch)"
  paquets_de_base
  if [ -n "$SOURCES" ]; then compiler "$BRANCHE"; else obtenir_archive; fi
  installer_archive
  installer_services
}

menu_acces() {
  choisir_acces
  configurer_acces
  if changer_env BASE_URL "$BASE_URL" BIND_ADDR "$BIND_ADDR"; then
    info "Adresse principale : $BASE_URL"
    # Un vrai if : « [ … ] && warn » en dernière ligne rendrait une erreur
    # sans Google, et set -e arrêterait tout le script.
    if [ -n "$(valeur GOOGLE_CLIENT_ID)" ]; then
      warn "Google : l'URI de redirection doit devenir $BASE_URL/auth/google/callback dans la console Google."
    fi
  fi
}

menu_fichiers() {
  CONVERSIONS=""
  if [ -n "$(valeur FILES_DIR)" ]; then
    info "Les fichiers sont déjà activés."
  else
    preparer_fichiers
    changer_env FILES_DIR /srv/dashu/fichiers WORK_DIR /srv/dashu/tmp || return 0
    systemctl enable dashu-worker >/dev/null 2>&1
    systemctl restart dashu-worker
    info "Fichiers activés."
  fi
  if oui_non "Installer les outils de conversion (vidéos, photos HEIC, PDF, documents ; environ 600 Mo) ?" n; then
    installer_conversions
    systemctl restart dashu-worker
  fi
}

menu_mot_de_passe() {
  local autorisees
  demander_email
  # Liste dans dashu.env : on peut y ajouter l'adresse. Sinon, elle est en
  # base et se gère dans Réglages › Instance ; dashu set-password dira si
  # l'adresse n'y est pas.
  if [ -n "$(valeur ALLOWED_EMAILS)" ]; then
    autorisees="$(valeur ALLOWED_EMAILS | tr ',' '\n' | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
    if ! grep -qxF "$EMAIL" <<<"$autorisees"; then
      oui_non "$EMAIL n'est pas autorisée à se connecter. L'ajouter ?" o || return 0
      dashu-compte ajouter "$EMAIL" >/dev/null || return 0
    fi
  fi
  demander_mot_de_passe
  if definir_mot_de_passe "$EMAIL" "$MOT_DE_PASSE"; then
    info "Mot de passe enregistré pour $EMAIL."
  fi
  MOT_DE_PASSE=""
}

menu() {
  local choix
  BASE_URL="$(valeur BASE_URL)"
  say "dashu est déjà installé ($(dashu version 2>/dev/null || echo 'version inconnue'))"
  while :; do
    # À chaque tour : un choix du menu a pu changer l'adresse, ou redémarrer
    # dashu, ce qui change le code d'installation.
    BASE_URL="$(valeur BASE_URL)"
    info "Adresse : $BASE_URL"
    if ! sqlite3 /var/lib/dashu/dashu.db 'SELECT 1 FROM users LIMIT 1' 2>/dev/null | grep -q 1; then
      info "Pas encore de compte : ouvre $BASE_URL/setup et tape le code $(code_installation)"
    fi
    cat >/dev/tty <<TEXTE

  1) Mettre à jour
  2) Changer l'accès (réseau local, Tailscale, Funnel, Cloudflare)
  3) Activer les fichiers et les conversions
  4) Connexion avec Google
  5) Créer un compte ou changer un mot de passe
  6) Quitter

TEXTE
    demander choix "Ton choix" 6
    case "$choix" in
      1) dashu-update || warn "La mise à jour n'a pas abouti (voir ci-dessus)." ;;
      2) menu_acces ;;
      3) menu_fichiers ;;
      4)
        BASE_URL="$(valeur BASE_URL)"
        configurer_google
        ;;
      5) menu_mot_de_passe ;;
      6 | q) return 0 ;;
      *) printf 'Un numéro entre 1 et 6.\n' >/dev/tty ;;
    esac
  done
}

# ---------- Point d'entrée ----------

usage() {
  sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" 2>/dev/null | sed '$d; s/^# \{0,1\}//'
  exit 2
}

main() {
  INTERACTIF=1 CANAL=stable CANAL_DONNE="" ACCES="" ADRESSE="" EMAIL="" FICHIERS="" CONVERSIONS=""
  ARCHIVE="" SOURCES="" BRANCHE="" MOT_DE_PASSE=""
  ACCES_LOCAL="" ACCES_TS="" ACCES_FUNNEL="" ACCES_CF="" CF_NOM="" CF_JETON="" TS_NOM="" COMPTE_CREE=""

  # Ancienne forme : install.sh <email> <adresse> [branche]. Sans question,
  # dashu n'écoute que sur la machine, et les fichiers suivent /srv/dashu
  # s'il est monté, comme avant.
  if [ $# -ge 2 ] && [ "${1#-}" = "$1" ]; then
    INTERACTIF="" EMAIL="$1" ADRESSE="$2" CANAL="${3:-v1}" ACCES=aucun
    [ -n "${3:-}" ] && CANAL_DONNE=1
    mountpoint -q /srv/dashu 2>/dev/null && FICHIERS=oui CONVERSIONS=oui
    case "$CANAL" in stable | v1) ;; *) SOURCES=1 BRANCHE="$CANAL" ;; esac
    shift $#
  fi
  while [ $# -gt 0 ]; do
    case "$1" in
      --non-interactif) INTERACTIF="" ;;
      --canal) CANAL="${2:?--canal stable|v1}" CANAL_DONNE=1 && shift ;;
      --acces) ACCES="${2:?--acces aucun|local}" && shift ;;
      --adresse) ADRESSE="${2:?--adresse <url>}" && shift ;;
      --email) EMAIL="$(printf '%s' "${2:?--email <adresse>}" | tr '[:upper:]' '[:lower:]')" && shift ;;
      --fichiers) FICHIERS=oui ;;
      --conversions) FICHIERS=oui CONVERSIONS=oui ;;
      --archive) ARCHIVE="$(realpath "${2:?--archive <fichier>}")" && shift ;;
      --depuis-les-sources)
        SOURCES=1
        if [ $# -ge 2 ] && [ "${2#-}" = "$2" ]; then
          BRANCHE="$2"
          shift
        fi
        ;;
      -h | --help) usage ;;
      *) die "Option inconnue : $1 (voir --help)." ;;
    esac
    shift
  done
  [ -n "$SOURCES" ] && BRANCHE="${BRANCHE:-v1}" && CANAL="$BRANCHE"

  verifier_machine
  if [ -n "$INTERACTIF" ]; then
    { : </dev/tty; } 2>/dev/null ||
      die "Pas de terminal pour poser les questions : lance le script dans un terminal, ou avec --non-interactif (voir --help)."
    # Les commandes lancées ne lisent plus l'entrée du script : seules les
    # questions lisent le terminal.
    exec </dev/null
  fi

  TRAVAIL="$(mktemp -d)"
  trap 'rm -rf "$TRAVAIL"' EXIT

  if [ -f "$ENV_FILE" ] && [ -x /usr/local/bin/dashu ]; then
    if [ -n "$INTERACTIF" ]; then menu; else relance_sans_question; fi
  else
    installation_neuve
  fi
}

main "$@"
