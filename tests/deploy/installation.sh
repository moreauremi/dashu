#!/usr/bin/env bash
# Test de deploy/lxc/install.sh et de dashu-update, lancé par la CI sur une
# machine jetable, en root, avec l'archive que la CI vient de construire :
#
#   sudo bash tests/deploy/installation.sh dashu-x86_64-linux.tar.gz
#
# 1. Installation avec les questions (dans un terminal simulé par `script`) :
#    accès local, fichiers, compte. Le mot de passe ne s'affiche pas.
# 2. Connexion avec ce compte ; ancienne forme du script relancée : la
#    configuration et la base ne bougent pas.
# 3. dashu-update : « déjà à jour », archive abîmée refusée, nouvelle version
#    installée, version qui ne démarre pas remplacée par la précédente.
# 4. Menu du script relancé : changer le mot de passe.
#
# Les versions sont servies par un petit serveur HTTP local
# (DASHU_TELECHARGEMENT), comme les publie la CI : archive et SHA256SUMS.
# À ne pas lancer sur une vraie machine : le test installe dashu pour de bon.
set -euo pipefail

ICI="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALL="$ICI/deploy/lxc/install.sh"
ARCHIVE="$(realpath "${1:?usage : installation.sh <archive dashu-<arch>-linux.tar.gz>}")"
[ "$(id -u)" -eq 0 ] || {
  echo "à lancer en root, sur une machine jetable" >&2
  exit 1
}
case "$(uname -m)" in x86_64) ARCH=x86_64 ;; *) ARCH=aarch64 ;; esac
NOM="dashu-$ARCH-linux.tar.gz"
EMAIL=essai@example.test
MDP="un mot de passe d'essai"
MDP2="un autre mot de passe"

TMP="$(mktemp -d)"
DEPOT="$TMP/depot"
mkdir -p "$DEPOT"
python3 -m http.server 8765 --bind 127.0.0.1 --directory "$DEPOT" >/dev/null 2>&1 &
SERVEUR=$!
trap 'kill $SERVEUR 2>/dev/null || true; rm -rf "$TMP"' EXIT
export DASHU_TELECHARGEMENT=http://127.0.0.1:8765

fail() {
  echo "ÉCHEC : $*" >&2
  journalctl -u dashu -n 20 --no-pager >&2 || true
  exit 1
}
etape() { printf '\n----- %s\n' "$*"; }

# Publie une archive dans le faux dépôt, avec son empreinte.
publier() {
  cp "$1" "$DEPOT/$NOM"
  (cd "$DEPOT" && sha256sum "$NOM" >SHA256SUMS)
}

# Archive dont le binaire est un petit script : il répond « dashu <version> »
# à `dashu version` et, sauf s'il doit échouer, lance le vrai binaire.
archive_modifiee() { # archive_modifiee <version> <démarre: oui|non> <sortie>
  local d="$TMP/modif"
  rm -rf "$d" && mkdir -p "$d"
  tar -xzf "$ARCHIVE" -C "$d"
  # Une seule fois : ensuite, ce binaire tourne (on ne remplace pas un
  # programme en cours d'exécution).
  [ -x /opt/dashu-essai-reel ] || install -m 755 "$d/dashu/dashu" /opt/dashu-essai-reel
  if [ "$2" = oui ]; then
    printf '#!/bin/sh\n[ "$1" = version ] && { echo "dashu %s"; exit 0; }\nexec /opt/dashu-essai-reel "$@"\n' "$1" >"$d/dashu/dashu"
  else
    printf '#!/bin/sh\n[ "$1" = version ] && { echo "dashu %s"; exit 0; }\n[ "$1" = backup ] && exec /opt/dashu-essai-reel "$@"\nexit 1\n' "$1" >"$d/dashu/dashu"
  fi
  chmod 755 "$d/dashu/dashu"
  tar -czf "$3" -C "$d" dashu
}

# Connexion par le formulaire, avec son jeton CSRF : 303 vers / si c'est bon.
connexion() { # connexion <email> <mot de passe>
  local jar="$TMP/cookies" csrf code
  rm -f "$jar"
  csrf="$(curl -fsS -c "$jar" http://127.0.0.1:3000/login | grep -o 'name="_csrf" value="[^"]*"' | cut -d'"' -f4)"
  [ -n "$csrf" ] || fail "pas de jeton CSRF sur /login"
  code="$(curl -s -o /dev/null -w '%{http_code}' -b "$jar" -c "$jar" \
    --data-urlencode "_csrf=$csrf" --data-urlencode "email=$1" --data-urlencode "password=$2" \
    http://127.0.0.1:3000/login)"
  [ "$code" = 303 ]
}

# Répond aux questions du script comme une personne au clavier : chaque
# réponse part quand sa question est affichée (le mot de passe, donc, une
# fois l'écho coupé). Le script tourne dans un terminal simulé
# (tests/deploy/repondre.py) ; une question imprévue fait échouer le test.
repondre() { # repondre <journal> <réponses…>
  local journal="$1" code=0
  shift
  python3 "$ICI/tests/deploy/repondre.py" "$journal" "$@" -- bash "$INSTALL" || code=$?
  if [ "$code" != 0 ]; then
    sed 's/\x1b\[[0-9;]*m//g' "$journal" | tail -40
    fail "le script s'est arrêté en erreur ($code)"
  fi
}

publier "$ARCHIVE"
VERSION="$(tar -xzOf "$ARCHIVE" dashu/dashu >"$TMP/bin" && chmod +x "$TMP/bin" && "$TMP/bin" version)"
echo "Version essayée : $VERSION"

etape "1. Installation avec les questions"
# Accès 1 (réseau local), fichiers oui, conversions non, email, mot de passe
# deux fois. Pas de question Google : l'adresse locale n'est pas en HTTPS.
repondre "$TMP/installation.log" 1 o n "$EMAIL" "$MDP" "$MDP"
sed 's/\x1b\[[0-9;]*m//g' "$TMP/installation.log" | tail -25
grep -q "C'est prêt" "$TMP/installation.log" || fail "pas de résumé final"
if grep -qF "$MDP" "$TMP/installation.log"; then fail "le mot de passe s'est affiché"; fi
curl -fsS http://127.0.0.1:3000/health | grep -qx ok || fail "/health"
[ "$(dashu version)" = "$VERSION" ] || fail "version installée : $(dashu version)"
[ "$(stat -c '%a %U %G' /etc/dashu/dashu.env)" = "640 root dashu" ] || fail "droits de dashu.env"
grep -qx 'BIND_ADDR=0.0.0.0:3000' /etc/dashu/dashu.env || fail "accès local : BIND_ADDR"
grep -qx "ALLOWED_EMAILS=$EMAIL" /etc/dashu/dashu.env || fail "ALLOWED_EMAILS"
grep -qx 'FILES_DIR=/srv/dashu/fichiers' /etc/dashu/dashu.env || fail "fichiers non activés"
systemctl is-active --quiet dashu-worker || fail "service de conversion arrêté"
systemctl is-enabled --quiet dashu-backup.timer || fail "sauvegarde nocturne non programmée"
[ "$(cat /etc/dashu/source)" = binaire ] || fail "mode de mise à jour"
for s in dashu-update dashu-backup dashu-restore dashu-admin dashu-compte; do
  [ -x "/usr/local/sbin/$s" ] || fail "$s non installé"
done
[ -f /opt/dashu/share/deploy/hermes/SKILL.md ] || fail "fichiers d'Hermès absents de /opt/dashu/share"
dashu-admin status | grep -q "version : ${VERSION#dashu }" || fail "dashu-admin status"

etape "2. Connexion, puis relance sans question"
connexion "$EMAIL" "$MDP" || fail "connexion avec le compte créé"
if connexion "$EMAIL" "mauvais mot de passe"; then fail "un mauvais mot de passe passe"; fi
AVANT="$(sha256sum /etc/dashu/dashu.env)"
bash "$INSTALL" "$EMAIL" http://exemple.invalid >/dev/null
[ "$(sha256sum /etc/dashu/dashu.env)" = "$AVANT" ] || fail "la relance a modifié dashu.env"
connexion "$EMAIL" "$MDP" || fail "connexion après la relance"

etape "3. Mises à jour"
dashu-update | tee "$TMP/maj.log"
grep -q "Déjà à jour" "$TMP/maj.log" || fail "même version : « déjà à jour » attendu"

# Archive abîmée (empreinte d'une autre) : refusée, rien ne change.
archive_modifiee "9.9.9 essai" oui "$TMP/nouvelle.tar.gz"
publier "$ARCHIVE"
cp "$TMP/nouvelle.tar.gz" "$DEPOT/$NOM"
if dashu-update >"$TMP/maj.log" 2>&1; then fail "une archive abîmée est passée"; fi
grep -q "ne correspond pas à son empreinte" "$TMP/maj.log" || fail "message de l'archive abîmée"
[ "$(dashu version)" = "$VERSION" ] || fail "la version a changé malgré l'archive abîmée"

# Nouvelle version : installée, notée, dashu répond.
publier "$TMP/nouvelle.tar.gz"
dashu-update | tee "$TMP/maj.log"
grep -q "Mise à jour terminée" "$TMP/maj.log" || fail "mise à jour"
[ "$(dashu version)" = "dashu 9.9.9 essai" ] || fail "nouvelle version non installée"
[ "$(cat /etc/dashu/deployed-commit)" = essai ] || fail "deployed-commit"
connexion "$EMAIL" "$MDP" || fail "connexion après la mise à jour"

# Version qui ne démarre pas : retour à la précédente, base comprise.
archive_modifiee "9.9.10 casse" non "$TMP/casse.tar.gz"
publier "$TMP/casse.tar.gz"
if dashu-update >"$TMP/maj.log" 2>&1; then fail "une version qui ne démarre pas est restée"; fi
grep -q "retour à la précédente" "$TMP/maj.log" || fail "pas de retour arrière"
[ "$(dashu version)" = "dashu 9.9.9 essai" ] || fail "ancienne version non remise"
curl -fsS http://127.0.0.1:3000/health | grep -qx ok || fail "/health après le retour arrière"
connexion "$EMAIL" "$MDP" || fail "connexion après le retour arrière"

etape "4. Menu : changer le mot de passe"
repondre "$TMP/menu.log" 5 "$EMAIL" "$MDP2" "$MDP2" 6
if grep -qF "$MDP2" "$TMP/menu.log"; then fail "le nouveau mot de passe s'est affiché"; fi
connexion "$EMAIL" "$MDP2" || fail "connexion avec le nouveau mot de passe"
if connexion "$EMAIL" "$MDP"; then fail "l'ancien mot de passe marche encore"; fi

echo
echo "Installation et mises à jour : tout est bon."
