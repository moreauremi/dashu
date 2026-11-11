#!/bin/sh
# Sauvegarde nocturne de la base SQLite de dashu.
#
# Fait une copie à chaud (commande SQLite `VACUUM INTO`, lancée par
# `dashu backup` dans le conteneur) vers le second disque, la vérifie, puis
# supprime les copies de plus de 14 jours. Le serveur n'est pas arrêté.
#
# Installation, une fois, sur la machine qui héberge Docker :
#
#   1. Dans .env, pointer BACKUP_DIR vers le second disque :
#        BACKUP_DIR=/mnt/backup/dashu
#   2. Créer ce dossier et le donner à l'utilisateur du conteneur (10001) :
#        mkdir -p /mnt/backup/dashu && chown 10001:10001 /mnt/backup/dashu
#   3. Relancer :  docker compose up -d
#   4. Planifier, avec `crontab -e` (tous les jours à 3 h 30) :
#        30 3 * * * /opt/dashu/scripts/backup.sh >> /var/log/dashu-backup.log 2>&1
#
# Restauration (le nom du projet Compose est celui du dossier, ici `dashu`) :
#
#   cd /opt/dashu && docker compose stop dashu
#   docker run --rm -v dashu_data:/data -v /mnt/backup/dashu:/backups:ro alpine \
#     sh -c "rm -f /data/dashu.db-wal /data/dashu.db-shm \
#            && cp /backups/dashu-AAAA-MM-JJ_HHMMSS.db /data/dashu.db \
#            && chown 10001:10001 /data/dashu.db"
#   docker compose start dashu
#
# (Sans Docker, dans le conteneur LXC : dashu-restore, voir le README.)

set -eu

# Dossier du dépôt (celui qui contient docker-compose.yml et .env).
APP_DIR=${APP_DIR:-$(cd "$(dirname "$0")/.." && pwd)}
KEEP_DAYS=${KEEP_DAYS:-14}

cd "$APP_DIR"

# BACKUP_DIR vient du .env, comme pour docker compose.
if [ -z "${BACKUP_DIR:-}" ] && [ -f .env ]; then
    BACKUP_DIR=$(sed -n 's/^BACKUP_DIR=//p' .env | tail -n 1)
fi
BACKUP_DIR=${BACKUP_DIR:-$APP_DIR/backups}

if [ ! -d "$BACKUP_DIR" ]; then
    echo "$(date '+%F %T') ERREUR : le dossier de sauvegarde $BACKUP_DIR n'existe pas" >&2
    exit 1
fi

name="dashu-$(date +%Y-%m-%d_%H%M%S).db"

# Dans le conteneur, BACKUP_DIR est monté sur /backups.
# -T : pas de terminal, indispensable sous cron.
if ! docker compose exec -T dashu /dashu backup "/backups/$name"; then
    echo "$(date '+%F %T') ERREUR : la sauvegarde a échoué" >&2
    exit 1
fi

# Ménage : on ne garde que les KEEP_DAYS derniers jours.
find "$BACKUP_DIR" -maxdepth 1 -name 'dashu-*.db' -type f -mtime +"$KEEP_DAYS" -delete

count=$(find "$BACKUP_DIR" -maxdepth 1 -name 'dashu-*.db' -type f | wc -l)
echo "$(date '+%F %T') OK : $BACKUP_DIR/$name ($count sauvegardes conservées)"
