#!/usr/bin/env bash
# Branche Hermès (l'assistant) à dashu, ou met à jour sa commande `dashu`
# et son skill.
#
# À lancer en root sur l'hôte Proxmox, après un dashu-update :
#   bash <(pct exec <n° du conteneur dashu> -- cat /opt/dashu/src/deploy/hermes/install.sh)
#
# Les fichiers viennent de la copie du dépôt dans le conteneur dashu
# (/opt/dashu/src/deploy/hermes), ou du dossier indiqué par SRC=.
# Sans danger à relancer : ce qui est déjà en place n'est pas touché, et la
# commande comme le skill ne sont remplacés que s'ils ont changé.
#
# Réglages possibles (variables d'environnement) :
#   DASHU_CT, HERMES_CT  numéros des conteneurs (par défaut : trouvés par leur nom)
#   DASHU_URL            adresse de dashu vue par Hermès (par défaut : son nom Tailscale)
#   SRC                  dossier qui contient dashu et SKILL.md
set -euo pipefail

say()  { printf '\n==> %s\n' "$*"; }
warn() { printf '[!] %s\n' "$*" >&2; }
die()  { printf '[x] %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "Lance ce script en root."
command -v pct >/dev/null || die "pct introuvable : ce script tourne sur l'hôte Proxmox."

by_name() { pct list | awk -v n="$1" 'NR>1 && $NF==n {print $1; exit}'; }
CTID="${DASHU_CT:-$(by_name dashu)}"
HID="${HERMES_CT:-$(by_name hermes)}"
[ -n "$CTID" ] || die "Conteneur « dashu » introuvable : indique son numéro avec DASHU_CT=<n>."
[ -n "$HID" ] || die "Conteneur « hermes » introuvable : indique son numéro avec HERMES_CT=<n>."

H=/home/hermes
SKILL_DIR="$H/.hermes/skills/productivity/dashu"
# Commande lancée dans le conteneur d'Hermès, sous son compte
HX() { pct exec "$HID" -- runuser -u hermes -- env HOME="$H" "$@"; }

WORK="$(mktemp -d)"
chmod 700 "$WORK"
trap 'rm -rf "$WORK"' EXIT

# Copie un fichier dans le conteneur d'Hermès s'il diffère de celui en place.
# Renvoie 0 s'il a été remplacé, 1 s'il était déjà à jour.
push_if_changed() {
  local src="$1" dst="$2" mode="$3" owner="$4"
  if pct exec "$HID" -- cat "$dst" 2>/dev/null | cmp -s - "$src"; then
    echo "  $dst : déjà à jour"
    return 1
  fi
  pct push "$HID" "$src" "$dst" --perms "$mode" --user "$owner" --group "$owner"
  echo "  $dst : installé"
}

# Ajoute une ligne (ou un bloc) à la fin d'un fichier d'Hermès, sous son compte.
append_as_hermes() {
  pct push "$HID" "$1" /tmp/dashu-ajout --perms 644 --user hermes --group hermes
  HX sh -c 'cat /tmp/dashu-ajout >> "$0" && chmod 600 "$0"; rm -f /tmp/dashu-ajout' "$2"
}

say "Fichiers versionnés"
if [ -n "${SRC:-}" ]; then
  cp "$SRC/dashu" "$SRC/SKILL.md" "$WORK/"
else
  for f in dashu SKILL.md; do
    pct exec "$CTID" -- cat "/opt/dashu/src/deploy/hermes/$f" > "$WORK/$f" \
      || die "/opt/dashu/src/deploy/hermes/$f absent du conteneur $CTID : lance d'abord dashu-update."
  done
fi

say "Commande et skill dans le conteneur $HID"
pct exec "$HID" -- sh -c 'command -v python3 >/dev/null' \
  || die "python3 manque dans le conteneur d'Hermès : apt-get install python3"
pct exec "$HID" -- install -d -o hermes -g hermes -m 700 "$H/.config" "$H/.config/dashu" "$H/.ssh"
pct exec "$HID" -- install -d -o hermes -g hermes "$H/.hermes/skills" "$H/.hermes/skills/productivity" "$SKILL_DIR"
push_if_changed "$WORK/dashu" /usr/local/bin/dashu 755 root || true
restart=0
push_if_changed "$WORK/SKILL.md" "$SKILL_DIR/SKILL.md" 644 hermes && restart=1

say "Adresse et jeton de l'API"
if [ -n "${DASHU_URL:-}" ]; then
  FQDN="${DASHU_URL#https://}"
else
  FQDN="$(pct exec "$CTID" -- tailscale status --json | grep -m1 '"DNSName"' | sed -E 's/.*"DNSName": *"([^"]+)\.?".*/\1/; s/\.$//')"
fi
[ -n "$FQDN" ] || die "Nom Tailscale de dashu introuvable : indique DASHU_URL=https://…"
if pct exec "$HID" -- test -s "$H/.config/dashu/url"; then
  echo "  url : déjà en place"
else
  printf 'https://%s\n' "$FQDN" > "$WORK/url"
  pct push "$HID" "$WORK/url" "$H/.config/dashu/url" --perms 600 --user hermes --group hermes
  echo "  url : https://$FQDN"
fi
if pct exec "$HID" -- test -s "$H/.config/dashu/token"; then
  echo "  jeton : déjà en place"
else
  # Le jeton va d'un conteneur à l'autre sans jamais être affiché.
  pct exec "$CTID" -- sed -n 's/^API_TOKEN=//p' /etc/dashu/dashu.env | tr -d "\"'" > "$WORK/token"
  [ -s "$WORK/token" ] || die "API_TOKEN est vide dans /etc/dashu/dashu.env : l'API est désactivée."
  pct push "$HID" "$WORK/token" "$H/.config/dashu/token" --perms 600 --user hermes --group hermes
  echo "  jeton : copié depuis /etc/dashu/dashu.env"
fi

say "Accès d'admin limité (clé SSH, six actions de dashu-admin)"
if pct exec "$HID" -- test -f "$H/.ssh/dashu_admin"; then
  echo "  clé d'Hermès : déjà en place"
else
  HX /usr/bin/ssh-keygen -q -t ed25519 -N "" -C hermes-dashu-admin -f "$H/.ssh/dashu_admin"
  echo "  clé d'Hermès : créée"
fi
PUBKEY="$(pct exec "$HID" -- cat "$H/.ssh/dashu_admin.pub")"
if pct exec "$CTID" -- grep -qF "$PUBKEY" /root/.ssh/authorized_keys 2>/dev/null; then
  echo "  autorisation dans dashu : déjà en place"
else
  printf 'restrict,command="/usr/local/sbin/dashu-admin" %s\n' "$PUBKEY" > "$WORK/ak"
  pct exec "$CTID" -- install -d -m 700 /root/.ssh
  pct push "$CTID" "$WORK/ak" /root/dashu-ak --perms 600
  pct exec "$CTID" -- sh -c 'cat /root/dashu-ak >> /root/.ssh/authorized_keys; chmod 600 /root/.ssh/authorized_keys; rm -f /root/dashu-ak'
  pct exec "$CTID" -- systemctl enable --now ssh >/dev/null 2>&1 || warn "le serveur SSH du conteneur dashu ne démarre pas"
  echo "  autorisation dans dashu : ajoutée"
fi
if HX /usr/bin/ssh-keygen -F "$FQDN" -f "$H/.ssh/known_hosts" >/dev/null 2>&1; then
  echo "  clé d'hôte de dashu : déjà connue"
else
  TSIP="$(pct exec "$CTID" -- tailscale ip -4 | head -1)"
  HOSTKEY="$(pct exec "$CTID" -- cat /etc/ssh/ssh_host_ed25519_key.pub | awk '{print $1" "$2}')"
  printf '%s,%s %s\n' "$FQDN" "$TSIP" "$HOSTKEY" > "$WORK/known_host"
  append_as_hermes "$WORK/known_host" "$H/.ssh/known_hosts"
  echo "  clé d'hôte de dashu : ajoutée"
fi
if pct exec "$HID" -- grep -q '^Host dashu-admin$' "$H/.ssh/config" 2>/dev/null; then
  echo "  ~/.ssh/config : déjà en place"
else
  cat > "$WORK/ssh_config" <<EOF

Host dashu-admin
  HostName $FQDN
  User root
  IdentityFile ~/.ssh/dashu_admin
  IdentitiesOnly yes
  StrictHostKeyChecking yes
  BatchMode yes
  ConnectTimeout 10
EOF
  append_as_hermes "$WORK/ssh_config" "$H/.ssh/config"
  echo "  ~/.ssh/config : hôte dashu-admin ajouté"
fi

say "Consigne dans SOUL.md"
if pct exec "$HID" -- grep -q "skill dashu" "$H/.hermes/SOUL.md" 2>/dev/null; then
  echo "  déjà en place"
else
  printf '\nPour les tâches, les notes et l’emploi du temps de l’école de Rémi, utilise la commande `dashu` (skill dashu).\n' > "$WORK/soul_line"
  pct push "$HID" "$WORK/soul_line" /tmp/dashu-soul --perms 644 --user hermes --group hermes
  HX sh -c 'cat /tmp/dashu-soul >> "$HOME/.hermes/SOUL.md"; rm -f /tmp/dashu-soul'
  echo "  ajoutée"
  restart=1
fi

say "Tests depuis le compte d'Hermès"
# Chemin complet : runuser ne met pas /usr/local/bin dans le PATH.
echec=0
HX /usr/local/bin/dashu spaces || { warn "dashu spaces échoue : l'API ne répond pas à Hermès."; echec=1; }
HX /usr/local/bin/dashu admin status || { warn "L'admin à distance ne répond pas."; echec=1; }

if [ "$restart" -eq 1 ]; then
  say "Redémarrage d'Hermès, pour qu'il relise son skill et SOUL.md"
  pct exec "$HID" -- systemctl restart hermes-gateway \
    && echo "  hermes-gateway redémarré" \
    || warn "hermes-gateway introuvable : redémarre Hermès à la main (hermes gateway restart)."
fi
if [ "$echec" -eq 1 ]; then
  die "Installation faite, mais un test a échoué (voir plus haut)."
fi
say "Terminé"
