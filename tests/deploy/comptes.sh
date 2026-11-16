#!/usr/bin/env bash
# Test de deploy/lxc/dashu-compte, lancé par la CI (et à la main) : ajouter,
# lister, retirer une adresse, les refus, et le retour à l'ancien fichier si
# dashu ne repart pas. Tout se passe dans un dossier temporaire : le
# redémarrage est simulé, rien ne touche au système.
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")/../.." && pwd)/deploy/lxc/dashu-compte"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export DASHU_ENV_FILE="$TMP/dashu.env"
export DASHU_RESTART="echo redémarrage >> '$TMP/redemarrages'"
export DASHU_CHECK=true

fail() { echo "ÉCHEC : $*" >&2; exit 1; }
restarts() { wc -l < "$TMP/redemarrages" 2>/dev/null | tr -d ' ' || echo 0; }
line() { grep '^ALLOWED_EMAILS=' "$DASHU_ENV_FILE"; }
mode() { ls -l "$DASHU_ENV_FILE" | cut -c1-10; }

cat > "$DASHU_ENV_FILE" <<'ENV'
SESSION_SECRET=ne-pas-toucher
ALLOWED_EMAILS="remi@example.com"
BASE_URL=https://app.dashu.dev
API_TOKEN=0123456789abcdef0123456789abcdef
API_USER_EMAIL=remi@example.com
ENV
chmod 640 "$DASHU_ENV_FILE"
others() { grep -v '^ALLOWED_EMAILS=' "$DASHU_ENV_FILE"; }
BEFORE_OTHERS="$(others)"

# Ajouter : la liste grandit (en minuscules, guillemets retirés), le reste
# du fichier et ses droits ne bougent pas, dashu redémarre une fois.
OUT="$("$SCRIPT" ajouter ' Killian@Example.com ')"
[ "$(line)" = "ALLOWED_EMAILS=remi@example.com,killian@example.com" ] || fail "liste inattendue : $(line)"
[ "$(others)" = "$BEFORE_OTHERS" ] || fail "le reste du fichier a changé"
[ "$(mode)" = "-rw-r-----" ] || fail "droits changés : $(mode)"
[ "$(restarts)" = 1 ] || fail "dashu devait redémarrer une fois"
case "$OUT" in *"dashu-compte mot-de-passe killian@example.com"*) ;; *) fail "pas d'aide pour créer le compte : $OUT" ;; esac
[ -z "$(find "$TMP" -name '.dashu.env*')" ] || fail "un fichier temporaire est resté"

# Déjà là : rien ne change, pas de redémarrage.
"$SCRIPT" ajouter killian@example.com | grep -q "déjà autorisée" || fail "le doublon n'est pas signalé"
[ "$(restarts)" = 1 ] || fail "redémarrage inutile"

# Une adresse invalide est refusée, le fichier ne change pas.
before="$(line)"
for bad in "pas-une-adresse" "a@b" "x@y.fr,z@w.fr" 'x@y.fr;reboot'; do
  if "$SCRIPT" ajouter "$bad" 2>/dev/null; then fail "adresse invalide acceptée : $bad"; fi
done
[ "$(line)" = "$before" ] || fail "une adresse invalide a modifié le fichier"

# Liste : les deux, l'adresse de l'API signalée.
LIST="$("$SCRIPT" liste)"
grep -qx "remi@example.com (compte de l'API, Hermès)" <<<"$LIST" || fail "liste : $LIST"
grep -qx "killian@example.com" <<<"$LIST" || fail "liste : $LIST"

# Retirer : l'adresse de l'API est refusée ; une autre part.
if "$SCRIPT" retirer remi@example.com 2>/dev/null; then fail "l'adresse de l'API a été retirée"; fi
"$SCRIPT" retirer KILLIAN@example.com >/dev/null
[ "$(line)" = "ALLOWED_EMAILS=remi@example.com" ] || fail "retrait inattendu : $(line)"
[ "$(restarts)" = 2 ] || fail "dashu devait redémarrer après le retrait"
"$SCRIPT" retirer absent@example.com | grep -q "rien à faire" || fail "retrait d'une absente"

# Sans API : la dernière adresse ne se retire pas non plus.
sed -i.bak '/^API_/d' "$DASHU_ENV_FILE" && rm -f "$DASHU_ENV_FILE.bak"
if "$SCRIPT" retirer remi@example.com 2>/dev/null; then fail "la dernière adresse a été retirée"; fi

# dashu ne repart pas : l'ancien fichier revient, tel quel.
cp -p "$DASHU_ENV_FILE" "$TMP/avant"
if DASHU_CHECK=false "$SCRIPT" ajouter casse@example.com 2>/dev/null; then fail "l'échec n'est pas signalé"; fi
cmp -s "$DASHU_ENV_FILE" "$TMP/avant" || fail "l'ancien fichier n'a pas été remis"
[ "$(mode)" = "-rw-r-----" ] || fail "droits changés après le retour : $(mode)"
[ -z "$(find "$TMP" -name '.dashu.env*')" ] || fail "un fichier temporaire est resté après le retour"

# Mot de passe : seulement pour une adresse autorisée.
if "$SCRIPT" mot-de-passe inconnu@example.com 2>/dev/null; then fail "mot de passe pour une adresse non autorisée"; fi
# Commande inconnue : l'aide, et un échec.
if "$SCRIPT" supprimer-tout 2>/dev/null; then fail "commande inconnue acceptée"; fi

echo "comptes autorisés : ok"
