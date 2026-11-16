//! Notes en direct (SPEC, « Notes en temps réel ») : plusieurs personnes
//! écrivent dans la même note et voient les autres taper, avec leur curseur.
//!
//! L'éditeur utilise le module `prosemirror-collab` ; ce fichier en est
//! l'**arbitre**. Pour chaque note ouverte, une « session » (`Room`) garde en
//! mémoire la liste numérotée des modifications (les « étapes » de
//! ProseMirror) depuis l'ouverture, et la renvoie dans le même ordre à tous
//! ceux qui ont la note ouverte. Une étape envoyée sur une version dépassée
//! est ignorée : son auteur reçoit d'abord celles qui lui manquaient, recale
//! les siennes par-dessus, puis les renvoie.
//!
//! Le serveur ne lit jamais le contenu des étapes. Le Markdown reste la seule
//! vérité : la page qui vient d'écrire envoie le Markdown de la note à une
//! version confirmée, et on l'enregistre si cette version est toujours la
//! dernière, avec le même garde-fou de version (`notes.revision`) que
//! l'enregistrement classique.
//!
//! Une session vit tant que quelqu'un a la note ouverte, plus une minute de
//! grâce (une reconnexion la retrouve). Rien n'est écrit sur le disque en
//! dehors du Markdown.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::mpsc;

use super::{TITLE_MAX_CHARS, normalize_newlines};
use crate::AppState;
use crate::auth::session::Ctx;
use crate::time;
use crate::web::{AppError, AppResult};

/// Taille maximale d'un message reçu (le Markdown d'une longue note y tient).
const MAX_MESSAGE: usize = 1 << 20;
/// Étapes au plus par envoi.
const MAX_STEPS_PER_MESSAGE: usize = 1000;
/// Au-delà, la session repart de la version enregistrée.
const MAX_STEPS: usize = 100_000;
/// Personnes au plus sur une même note.
const MAX_PEERS: usize = 20;
/// Durée de vie d'une session après le départ de la dernière personne.
const GRACE: Duration = Duration::from_secs(60);
/// Signe de vie et nouvelle vérification de l'accès. Cloudflare coupe une
/// connexion muette au bout de 100 secondes.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Une position de curseur tient en quelques nombres.
const MAX_CURSOR_CHARS: usize = 200;

/// Les sessions de toutes les notes ouvertes (dans `AppState`).
#[derive(Default)]
pub struct Live {
    rooms: Mutex<HashMap<i64, Room>>,
    next_id: AtomicU64,
}

/// La session d'une note.
struct Room {
    /// Identifie cette session : une page qui se reconnecte sait si elle
    /// retrouve la même (et peut renvoyer ce qui n'était pas parti).
    epoch: String,
    /// Le Markdown de la note à l'ouverture de la session : la version 0.
    base_md: String,
    /// Étapes depuis la version 0, au format JSON de ProseMirror. La version
    /// courante est leur nombre.
    steps: Vec<Value>,
    /// Identifiant de la page qui a envoyé chaque étape.
    client_ids: Vec<Value>,
    title: String,
    /// `notes.revision` après le dernier enregistrement de la session.
    revision: i64,
    saved_md: String,
    saved_title: String,
    peers: HashMap<u64, Peer>,
    /// Change à chaque arrivée et à chaque départ : la minute de grâce ne
    /// ferme la session que si personne n'est revenu entre-temps.
    generation: u64,
    /// Un enregistrement à la fois : deux enregistrements simultanés ne se
    /// prennent pas l'un l'autre pour une modification venue d'ailleurs.
    save_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Room {
    fn new(note: &Loaded) -> Self {
        Self {
            epoch: epoch(),
            base_md: note.body_md.clone(),
            steps: Vec::new(),
            client_ids: Vec::new(),
            title: note.title.clone(),
            revision: note.revision,
            saved_md: note.body_md.clone(),
            saved_title: note.title.clone(),
            peers: HashMap::new(),
            generation: 0,
            save_lock: Arc::default(),
        }
    }

    fn version(&self) -> usize {
        self.steps.len()
    }

    /// Les présents, tels que les pages les affichent.
    fn peers_json(&self) -> Value {
        let mut peers: Vec<_> = self.peers.iter().collect();
        peers.sort_by_key(|(id, _)| **id);
        Value::Array(
            peers
                .into_iter()
                .map(|(id, p)| {
                    json!({
                        "id": id,
                        "name": p.name,
                        "initials": p.initials,
                        "color": p.color,
                        "cursor": p.cursor,
                    })
                })
                .collect(),
        )
    }

    fn send_all(&self, message: &Value) {
        let text = Utf8Bytes::from(message.to_string());
        for peer in self.peers.values() {
            let _ = peer.tx.send(text.clone());
        }
    }

    fn send_others(&self, conn: u64, message: &Value) {
        let text = Utf8Bytes::from(message.to_string());
        for (id, peer) in &self.peers {
            if *id != conn {
                let _ = peer.tx.send(text.clone());
            }
        }
    }
}

/// Une page connectée. Le nom et la couleur viennent du compte, jamais de ce
/// qu'envoie la page.
struct Peer {
    name: String,
    initials: String,
    color: String,
    tx: mpsc::UnboundedSender<Utf8Bytes>,
    cursor: Option<Value>,
}

/// Qui se connecte.
struct Who {
    user_id: i64,
    name: String,
    initials: String,
    color: String,
}

/// La note telle qu'elle est en base.
struct Loaded {
    title: String,
    body_md: String,
    revision: i64,
}

/// Messages envoyés par l'éditeur.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Incoming {
    /// Des étapes faites sur la version `version`. Avec `doc` et `title` (en
    /// quittant la page), c'est aussi un enregistrement de la version obtenue.
    Steps {
        version: usize,
        steps: Vec<Value>,
        #[serde(rename = "clientID")]
        client_id: Value,
        doc: Option<String>,
        title: Option<String>,
    },
    /// Position du curseur (`{anchor, head}`), relayée telle quelle.
    Cursor { cursor: Value },
    /// Le titre, à mesure qu'on le tape.
    Title { title: String },
    /// Le Markdown et le titre de la note à la version `version`.
    Save {
        version: usize,
        doc: String,
        title: String,
    },
}

/// Suite de la connexion après un message.
#[derive(PartialEq)]
enum Flow {
    Continue,
    Close,
}

impl Live {
    fn rooms(&self) -> MutexGuard<'_, HashMap<i64, Room>> {
        // Un verrou empoisonné (panique d'une autre tâche) garde des données
        // cohérentes : chaque section critique est courte et sans `await`.
        self.rooms.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Entre dans la session de la note (créée au besoin). Renvoie
    /// l'identifiant de la connexion et le message `init`, ou `None` si la
    /// note a déjà trop de monde.
    fn join(
        &self,
        note: i64,
        who: &Who,
        tx: mpsc::UnboundedSender<Utf8Bytes>,
        loaded: &Loaded,
    ) -> Option<(u64, Utf8Bytes)> {
        let mut rooms = self.rooms();
        let room = rooms.entry(note).or_insert_with(|| Room::new(loaded));
        // Session vide (minute de grâce) mais note modifiée ailleurs depuis :
        // elle repart de la base.
        if room.peers.is_empty() && room.revision != loaded.revision {
            *room = Room::new(loaded);
        }
        if room.peers.len() >= MAX_PEERS {
            return None;
        }
        let conn = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        room.peers.insert(
            conn,
            Peer {
                name: who.name.clone(),
                initials: who.initials.clone(),
                color: who.color.clone(),
                tx,
                cursor: None,
            },
        );
        room.generation += 1;
        let peers = room.peers_json();
        room.send_others(conn, &json!({ "type": "peers", "peers": peers }));
        let init = json!({
            "type": "init",
            "epoch": room.epoch,
            "version": room.version(),
            "doc": room.base_md,
            "steps": room.steps,
            "clientIDs": room.client_ids,
            "title": room.title,
            "revision": room.revision,
            "me": conn,
            "peers": peers,
        });
        Some((conn, Utf8Bytes::from(init.to_string())))
    }

    /// Quitte la session. La dernière personne partie, la session reste une
    /// minute, le temps d'une reconnexion.
    fn leave(self: &Arc<Self>, note: i64, conn: u64) {
        let mut rooms = self.rooms();
        let Some(room) = rooms.get_mut(&note) else {
            return;
        };
        if room.peers.remove(&conn).is_none() {
            return;
        }
        room.generation += 1;
        if !room.peers.is_empty() {
            let peers = room.peers_json();
            room.send_all(&json!({ "type": "peers", "peers": peers }));
            return;
        }
        let generation = room.generation;
        let live = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(GRACE).await;
            let mut rooms = live.rooms();
            if rooms
                .get(&note)
                .is_some_and(|r| r.peers.is_empty() && r.generation == generation)
            {
                rooms.remove(&note);
            }
        });
    }

    /// Ferme la session pour tout le monde après lui avoir envoyé `message`.
    /// Les connexions se terminent une fois le message lu.
    fn close_room(&self, note: i64, epoch: &str, message: &Value) {
        let mut rooms = self.rooms();
        if rooms.get(&note).is_some_and(|r| r.epoch == epoch)
            && let Some(room) = rooms.remove(&note)
        {
            room.send_all(message);
        }
    }

    /// Nombre de sessions ouvertes (pour les tests).
    pub fn open_rooms(&self) -> usize {
        self.rooms().len()
    }
}

/// `GET /notes/{id}/live` : ouvre le canal en direct de la note.
pub async fn socket(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    // Sans ce contrôle, une page d'un autre site (même sous dashu.dev)
    // pourrait ouvrir un canal avec le cookie de l'utilisateur.
    if !same_origin(&headers, &state.config.base_url) {
        return Err(AppError::Forbidden);
    }
    let note = load(&state.db, ctx.user.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let first = ctx.user.first_name.trim();
    let who = Who {
        user_id: ctx.user.id,
        name: if first.is_empty() {
            ctx.user.display_name()
        } else {
            first.to_string()
        },
        initials: ctx.user.initials(),
        color: ctx.user.avatar_color.clone(),
    };
    Ok(ws
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(socket, state, id, who, note)))
}

/// L'origine de la page (`Origin`) doit être dashu lui-même : l'hôte de la
/// requête, celui que transmet un proxy, ou celui de `BASE_URL`.
fn same_origin(headers: &HeaderMap, base_url: &str) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(host_of)
    else {
        return false;
    };
    let header_host = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    [
        header_host(header::HOST.as_str()),
        header_host("x-forwarded-host"),
        host_of(base_url),
    ]
    .into_iter()
    .flatten()
    .any(|host| host.eq_ignore_ascii_case(&origin))
}

/// `https://app.dashu.dev` → `app.dashu.dev` ; `http://localhost:3000` →
/// `localhost:3000`.
fn host_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

async fn load(db: &SqlitePool, user: i64, id: i64) -> AppResult<Option<Loaded>> {
    Ok(sqlx::query!(
        r#"SELECT title, body_md, revision AS "revision!: i64" FROM notes
           WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .map(|row| Loaded {
        title: row.title,
        body_md: row.body_md,
        revision: row.revision,
    }))
}

/// L'accès est revérifié en cours de route : un membre retiré de l'espace, ou
/// une note rangée hors de ses espaces, ferme le canal.
async fn has_access(db: &SqlitePool, user: i64, id: i64) -> bool {
    sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM notes
           WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
        id,
        user
    )
    .fetch_optional(db)
    .await
    .is_ok_and(|row| row.is_some())
}

fn text(message: &Value) -> Message {
    Message::Text(Utf8Bytes::from(message.to_string()))
}

async fn run(mut socket: WebSocket, state: AppState, note: i64, who: Who, loaded: Loaded) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let Some((conn, init)) = state.live.join(note, &who, tx, &loaded) else {
        let _ = socket.send(text(&json!({ "type": "full" }))).await;
        return;
    };
    if socket.send(Message::Text(init)).await.is_ok() {
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    let Some(Ok(message)) = incoming else { break };
                    match message {
                        Message::Text(body) => {
                            let flow = handle(&state, note, conn, &who, body.as_str()).await;
                            if flow == Flow::Close {
                                // Laisse partir ce que la session a encore à dire.
                                while let Ok(pending) = rx.try_recv() {
                                    let _ = socket.send(Message::Text(pending)).await;
                                }
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                outgoing = rx.recv() => {
                    // `None` : la session a été fermée (conflit, note
                    // supprimée), après avoir envoyé son dernier message.
                    let Some(body) = outgoing else { break };
                    if socket.send(Message::Text(body)).await.is_err() {
                        break;
                    }
                }
                _ = heartbeat.tick() => {
                    if !has_access(&state.db, who.user_id, note).await {
                        let _ = socket.send(text(&json!({ "type": "gone" }))).await;
                        break;
                    }
                    if socket.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    state.live.leave(note, conn);
    let _ = socket.send(Message::Close(None)).await;
}

async fn handle(state: &AppState, note: i64, conn: u64, who: &Who, body: &str) -> Flow {
    // Un message illisible est ignoré : il ne casse pas la session des autres.
    let Ok(message) = serde_json::from_str::<Incoming>(body) else {
        return Flow::Continue;
    };
    match message {
        Incoming::Steps {
            version,
            steps,
            client_id,
            doc,
            title,
        } => {
            if steps.is_empty() || steps.len() > MAX_STEPS_PER_MESSAGE {
                return Flow::Continue;
            }
            if !has_access(&state.db, who.user_id, note).await {
                return gone(state, note, conn);
            }
            let accepted = {
                let mut rooms = state.live.rooms();
                let Some(room) = rooms.get_mut(&note) else {
                    return Flow::Close;
                };
                if version != room.version() {
                    // Version dépassée : la page recevra les étapes qui lui
                    // manquent, recalera les siennes et les renverra.
                    None
                } else if room.steps.len() + steps.len() > MAX_STEPS {
                    let epoch = room.epoch.clone();
                    drop(rooms);
                    state
                        .live
                        .close_room(note, &epoch, &json!({ "type": "reset" }));
                    return Flow::Continue;
                } else {
                    let count = steps.len();
                    room.client_ids
                        .extend(std::iter::repeat_n(client_id.clone(), count));
                    room.steps.extend(steps.iter().cloned());
                    room.send_all(&json!({
                        "type": "steps",
                        "from": version,
                        "steps": steps,
                        "clientIDs": vec![client_id; count],
                    }));
                    Some(room.version())
                }
            };
            match (accepted, doc) {
                (Some(version), Some(doc)) => {
                    let title = title.unwrap_or_default();
                    save(state, note, who, version, doc, title).await
                }
                _ => Flow::Continue,
            }
        }
        Incoming::Cursor { cursor } => {
            if !cursor.is_object() || cursor.to_string().len() > MAX_CURSOR_CHARS {
                return Flow::Continue;
            }
            let mut rooms = state.live.rooms();
            if let Some(room) = rooms.get_mut(&note) {
                if let Some(peer) = room.peers.get_mut(&conn) {
                    peer.cursor = Some(cursor.clone());
                }
                room.send_others(
                    conn,
                    &json!({ "type": "cursor", "peer": conn, "cursor": cursor }),
                );
            }
            Flow::Continue
        }
        Incoming::Title { title } => {
            if !has_access(&state.db, who.user_id, note).await {
                return gone(state, note, conn);
            }
            let title: String = title.trim().chars().take(TITLE_MAX_CHARS).collect();
            let mut rooms = state.live.rooms();
            if let Some(room) = rooms.get_mut(&note) {
                room.title = title.clone();
                room.send_others(conn, &json!({ "type": "title", "title": title }));
            }
            Flow::Continue
        }
        Incoming::Save {
            version,
            doc,
            title,
        } => save(state, note, who, version, doc, title).await,
    }
}

/// Plus d'accès à la note : la page est prévenue et le canal se ferme.
fn gone(state: &AppState, note: i64, conn: u64) -> Flow {
    let rooms = state.live.rooms();
    if let Some(peer) = rooms.get(&note).and_then(|r| r.peers.get(&conn)) {
        let _ = peer
            .tx
            .send(Utf8Bytes::from(json!({ "type": "gone" }).to_string()));
    }
    Flow::Close
}

/// Enregistre le Markdown de la note à la version `version`, si c'est
/// toujours la dernière. Tout le monde reçoit « saved ».
async fn save(
    state: &AppState,
    note: i64,
    who: &Who,
    version: usize,
    doc: String,
    title: String,
) -> Flow {
    let doc = normalize_newlines(&doc);
    let title: String = title.trim().chars().take(TITLE_MAX_CHARS).collect();

    let (save_lock, epoch) = {
        let rooms = state.live.rooms();
        let Some(room) = rooms.get(&note) else {
            return Flow::Close;
        };
        (Arc::clone(&room.save_lock), room.epoch.clone())
    };
    // Un enregistrement à la fois par note, jusqu'à l'écriture en base.
    let _saving = save_lock.lock().await;

    let expected = {
        let rooms = state.live.rooms();
        let Some(room) = rooms.get(&note).filter(|r| r.epoch == epoch) else {
            return Flow::Close;
        };
        if version != room.version() {
            // Une version plus récente existe : son auteur l'enregistrera.
            return Flow::Continue;
        }
        if doc == room.saved_md && title == room.saved_title {
            return Flow::Continue;
        }
        room.revision
    };

    let now = time::now();
    let written = sqlx::query_scalar!(
        r#"UPDATE notes SET title = ?, body_md = ?, updated_at = ?, revision = revision + 1
           WHERE id = ? AND revision = ?
             AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)
           RETURNING revision AS "revision!: i64""#,
        title,
        doc,
        now,
        note,
        expected,
        who.user_id
    )
    .fetch_optional(&state.db)
    .await;

    match written {
        Ok(Some(revision)) => {
            let mut rooms = state.live.rooms();
            if let Some(room) = rooms.get_mut(&note).filter(|r| r.epoch == epoch) {
                room.revision = revision;
                room.saved_md = doc;
                room.saved_title = title;
                room.send_all(&json!({
                    "type": "saved",
                    "version": version,
                    "revision": revision,
                    "at": time::hm(now),
                }));
            }
            Flow::Continue
        }
        Ok(None) => {
            if has_access(&state.db, who.user_id, note).await {
                // La note a changé hors de la session (API, Hermès, éditeur
                // sans connexion en direct) : rien n'est écrasé.
                state
                    .live
                    .close_room(note, &epoch, &json!({ "type": "conflict" }));
            } else {
                state
                    .live
                    .close_room(note, &epoch, &json!({ "type": "gone" }));
            }
            Flow::Continue
        }
        Err(error) => {
            tracing::error!(%error, note, "enregistrement en direct");
            Flow::Continue
        }
    }
}

/// Identifiant aléatoire d'une session (16 caractères hexadécimaux).
fn epoch() -> String {
    let bytes: [u8; 8] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn origine_de_dashu_seulement() {
        let base = "https://app.dashu.dev";
        assert!(same_origin(
            &headers(&[
                ("origin", "https://app.dashu.dev"),
                ("host", "app.dashu.dev")
            ]),
            base
        ));
        // Tailscale : l'hôte de la requête.
        assert!(same_origin(
            &headers(&[
                ("origin", "https://dashu.example.ts.net"),
                ("host", "dashu.example.ts.net")
            ]),
            base
        ));
        // Proxy qui réécrit Host mais transmet l'hôte d'origine.
        assert!(same_origin(
            &headers(&[
                ("origin", "https://dashu.example.ts.net"),
                ("host", "127.0.0.1:3000"),
                ("x-forwarded-host", "dashu.example.ts.net")
            ]),
            base
        ));
        assert!(same_origin(
            &headers(&[
                ("origin", "http://localhost:3000"),
                ("host", "localhost:3000")
            ]),
            "http://localhost:3000"
        ));
        // La page d'accueil (même site, autre origine), un autre site, rien.
        assert!(!same_origin(
            &headers(&[("origin", "https://dashu.dev"), ("host", "app.dashu.dev")]),
            base
        ));
        assert!(!same_origin(
            &headers(&[
                ("origin", "https://evil.example"),
                ("host", "app.dashu.dev")
            ]),
            base
        ));
        assert!(!same_origin(&headers(&[("host", "app.dashu.dev")]), base));
        assert!(!same_origin(
            &headers(&[("origin", "null"), ("host", "app.dashu.dev")]),
            base
        ));
    }
}
