#!/usr/bin/env bash
# Test de deploy/lxc/dashu-backup-files, lancé par la CI (et à la main) :
# deux nuits de sauvegarde, puis la restauration d'un fichier supprimé.
# Tout se passe dans un dossier temporaire ; rien ne touche au système.
set -euo pipefail

SCRIPT="$(cd "$(dirname "$0")/../.." && pwd)/deploy/lxc/dashu-backup-files"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export DASHU_FILES_DIR="$TMP/fichiers" DASHU_FILES_BACKUP_DIR="$TMP/sauvegardes"
mkdir -p "$DASHU_FILES_DIR/ab" "$DASHU_FILES_DIR/.envois" "$DASHU_FILES_BACKUP_DIR"

fail() { echo "ÉCHEC : $*" >&2; exit 1; }
inode() { ls -i "$1" | awk '{print $1}'; }

echo "cours" > "$DASHU_FILES_DIR/ab/abcd"
echo "partiel" > "$DASHU_FILES_DIR/.envois/xyz.part"

# Première nuit : une copie complète, sans les envois en cours.
"$SCRIPT" 2026-11-15_033000 >/dev/null
N1="$DASHU_FILES_BACKUP_DIR/fichiers-2026-11-15_033000"
[ -f "$N1/ab/abcd" ] || fail "le fichier n'est pas dans le premier instantané"
[ ! -e "$N1/.envois" ] || fail "les envois en cours ont été copiés"

# Deuxième nuit : le fichier inchangé est relié, pas recopié ; le nouveau est copié.
echo "tp" > "$DASHU_FILES_DIR/ab/abef"
"$SCRIPT" 2026-11-16_033000 >/dev/null
N2="$DASHU_FILES_BACKUP_DIR/fichiers-2026-11-16_033000"
[ "$(inode "$N1/ab/abcd")" = "$(inode "$N2/ab/abcd")" ] || fail "le fichier inchangé a été recopié (pas de lien dur)"
[ -f "$N2/ab/abef" ] || fail "le nouveau fichier manque dans le deuxième instantané"
[ ! -f "$N1/ab/abef" ] || fail "le premier instantané a changé"
[ -z "$(find "$DASHU_FILES_BACKUP_DIR" -name '*.partiel')" ] || fail "un instantané partiel est resté"

# Restauration : le fichier supprimé revient, rien d'autre ne bouge.
rm "$DASHU_FILES_DIR/ab/abcd"
echo "arrivé après" > "$DASHU_FILES_DIR/ab/abzz"
OUT="$("$SCRIPT" restore 2026-11-16_033000)"
[ "$(cat "$DASHU_FILES_DIR/ab/abcd")" = "cours" ] || fail "le fichier supprimé n'est pas revenu"
[ -f "$DASHU_FILES_DIR/ab/abzz" ] || fail "un fichier arrivé depuis a été effacé"
case "$OUT" in "1 fichier(s) remis en place"*) ;; *) fail "message inattendu : $OUT" ;; esac

# Purge : un instantané de plus de 14 jours disparaît, les récents restent.
touch -t 202601010000 "$N1"
"$SCRIPT" 2026-11-17_033000 >/dev/null
[ ! -d "$N1" ] || fail "le vieil instantané n'a pas été effacé"
[ -d "$N2" ] || fail "un instantané récent a été effacé"

# Entrées refusées : horodatage invalide, instantané absent.
if "$SCRIPT" "../../etc" 2>/dev/null; then fail "un horodatage invalide est accepté"; fi
if "$SCRIPT" restore 2026-01-01_000000 2>/dev/null; then fail "une restauration sans instantané réussit"; fi

echo "sauvegarde des fichiers : ok"
