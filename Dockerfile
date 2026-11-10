# syntax=docker/dockerfile:1

# Image de dashu en deux étapes :
#   1. compilation du binaire, lié statiquement (musl) ;
#   2. image finale vide (scratch) qui ne contient que ce binaire.
# Pas de shell, pas de gestionnaire de paquets, pas de bibliothèque système :
# rien d'autre à mettre à jour ni à attaquer que dashu lui-même.

# ---------- Étape 1 : compilation ----------
FROM rust:1-alpine AS build

# gcc et musl-dev : SQLite (embarqué par sqlx) et ring (TLS) contiennent du C.
RUN apk add --no-cache gcc musl-dev

WORKDIR /src

# Les requêtes SQL sont vérifiées à la compilation à partir du dossier .sqlx,
# sans base de données sous la main.
ENV SQLX_OFFLINE=true

COPY . .

# Les deux caches accélèrent les reconstructions sur le serveur
# (`docker compose up -d --build`) : seules les crates modifiées recompilent.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/dashu /dashu \
    # Dossier de données appartenant à l'utilisateur non privilégié de l'image finale.
    && mkdir /data && chown 10001:10001 /data

# ---------- Étape 2 : image finale ----------
FROM scratch

COPY --from=build /dashu /dashu
COPY --from=build --chown=10001:10001 /data /data

# Utilisateur sans privilège (identifiant numérique : scratch n'a pas de /etc/passwd).
USER 10001:10001

ENV DATABASE_URL=sqlite:///data/dashu.db \
    BIND_ADDR=0.0.0.0:3000

VOLUME /data
EXPOSE 3000

# Pas de curl dans l'image : le binaire sait interroger son propre /health.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["/dashu", "healthcheck"]

ENTRYPOINT ["/dashu"]
