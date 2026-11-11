#!/usr/bin/env bash
# Installe dashu sur une Debian (conteneur LXC Proxmox ou machine), sans Docker :
# binaire compilé sur place, service systemd, sauvegarde chaque nuit, scripts
# d'exploitation dans /usr/local/sbin. À lancer en root :
#
#   bash install.sh <email autorisé> <adresse publique> [branche]
#   bash install.sh toi@example.com https://app.dashu.dev
#
# Relançable : la configuration et la base existantes ne sont jamais écrasées.
# Rien n'est exposé à l'extérieur : dashu écoute sur 127.0.0.1:3000, c'est
# Tailscale ou le tunnel Cloudflare qui le rendent joignable (voir le README).
set -euo pipefail

EMAIL="${1:?usage : install.sh <email autorisé> <adresse publique> [branche]}"
BASE_URL="${2:?usage : install.sh <email autorisé> <adresse publique> [branche]}"
BRANCH="${3:-v1}"
REPO="https://github.com/moreauremi/dashu.git"

export DEBIAN_FRONTEND=noninteractive
export RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo
export PATH="/opt/rust/cargo/bin:$PATH"
say() { printf '\n-> %s\n' "$*"; }
healthy() {
  for _ in $(seq 1 20); do
    curl -fsS --max-time 2 http://127.0.0.1:3000/health >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

say "Paquets système"
apt-get update -q
apt-get -y -q install curl git ca-certificates build-essential pkg-config openssl sqlite3

say "Rust (installé pour la machine, dans /opt/rust)"
command -v cargo >/dev/null ||
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path

say "Code (branche $BRANCH)"
install -d /opt/dashu /etc/dashu
printf '%s\n' "$BRANCH" > /etc/dashu/branch
[ -d /opt/dashu/src/.git ] || git clone -q --depth 1 --branch "$BRANCH" "$REPO" /opt/dashu/src
cd /opt/dashu/src

say "Compilation (10 à 20 min sur un vieux processeur, c'est normal)"
nice -n 10 cargo build --release --locked
install -m 755 target/release/dashu /usr/local/bin/dashu
git rev-parse HEAD > /etc/dashu/deployed-commit

say "Utilisateur, dossiers et configuration"
id dashu >/dev/null 2>&1 || useradd --system --home-dir /var/lib/dashu --shell /usr/sbin/nologin dashu
install -d -o dashu -g dashu -m 750 /var/lib/dashu /var/backups/dashu
if [ ! -f /etc/dashu/dashu.env ]; then
  umask 027
  cat > /etc/dashu/dashu.env <<ENV
SESSION_SECRET=$(openssl rand -hex 32)
ALLOWED_EMAILS=$EMAIL
BASE_URL=$BASE_URL
DATABASE_URL=sqlite:///var/lib/dashu/dashu.db
BIND_ADDR=127.0.0.1:3000
ENV
  umask 022
fi
chown root:dashu /etc/dashu/dashu.env
chmod 640 /etc/dashu/dashu.env
# Fichiers (v2) : seulement si un second disque est monté sur /srv/dashu
# (README, « Second disque »). Sans lui, la page Fichiers reste désactivée.
if mountpoint -q /srv/dashu; then
  install -d -o dashu -g dashu -m 750 /srv/dashu/fichiers
  grep -q '^FILES_DIR=' /etc/dashu/dashu.env || echo 'FILES_DIR=/srv/dashu/fichiers' >> /etc/dashu/dashu.env
fi

say "Services et scripts"
install -m 644 deploy/lxc/dashu.service deploy/lxc/dashu-backup.service deploy/lxc/dashu-backup.timer /etc/systemd/system/
install -d /etc/systemd/system/dashu.service.d
install -m 644 deploy/lxc/hardening.conf /etc/systemd/system/dashu.service.d/
for script in dashu-update dashu-backup dashu-restore dashu-admin; do
  install -m 755 "deploy/lxc/$script" /usr/local/sbin/
done
systemctl daemon-reload
systemctl enable --now dashu dashu-backup.timer
if ! healthy; then
  echo "dashu ne démarre pas avec le durcissement systemd : nouvel essai sans."
  rm -f /etc/systemd/system/dashu.service.d/hardening.conf
  systemctl daemon-reload
  systemctl restart dashu
  healthy || { journalctl -u dashu -n 30 --no-pager; echo "dashu ne démarre pas."; exit 1; }
fi

say "dashu répond sur 127.0.0.1:3000"
cat <<NEXT

Il reste à créer le compte (le mot de passe est demandé au clavier) :

  runuser -u dashu -- sh -c 'set -a; . /etc/dashu/dashu.env; exec dashu set-password "\$0"' $EMAIL

puis à rendre dashu joignable : Tailscale et/ou tunnel Cloudflare (README,
section « Déploiement »).
NEXT
