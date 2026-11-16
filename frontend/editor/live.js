// Notes en temps réel (SPEC, « Notes en temps réel ») : la note se connecte
// au serveur par une WebSocket (/notes/{id}/live) et s'édite à plusieurs.
//
// Le module officiel prosemirror-collab fait le travail délicat : il garde
// les modifications locales pas encore confirmées par le serveur et les
// recale par-dessus celles des autres. Ce fichier s'occupe du reste :
//   * la connexion, la reconnexion après une coupure, le repli sur
//     l'enregistrement classique quand le direct n'est pas possible ;
//   * l'envoi des étapes (une seule fois par version, renvoi si rien ne
//     revient), la réception de celles des autres ;
//   * l'enregistrement : une seconde après une modification confirmée, le
//     Markdown de la note à sa version confirmée part au serveur ;
//   * le titre, les présents et les curseurs des autres.
//
// Le serveur ne lit pas les étapes et sert d'arbitre (src/notes/live.rs).

import { Plugin, PluginKey } from "prosemirror-state";
import { Decoration, DecorationSet } from "prosemirror-view";
import { Step } from "prosemirror-transform";
import { receiveTransaction, sendableSteps, getVersion } from "prosemirror-collab";

import { schema } from "./format.js";

/** Méta que prosemirror-collab pose sur une transaction venue du serveur. */
const REMOTE = "collab$";

/** Vrai pour une transaction qui applique les modifications d'un autre. */
export const isRemote = (tr) => Boolean(tr.getMeta(REMOTE));

const SAVE_DELAY = 1000; // enregistrement, après la dernière modification confirmée
const CURSOR_DELAY = 80; // envoi de la position du curseur
const TITLE_DELAY = 250; // envoi du titre pendant la frappe
const FIRST_TIMEOUT = 4000; // sans réponse : enregistrement classique
const RESEND_DELAY = 2500; // étapes envoyées sans retour : renvoi
const MAX_RETRY_DELAY = 15000; // reconnexion : 1 s, 2 s, 4 s… jusqu'à 15 s
const MAX_BAD_STEPS = 3; // étapes illisibles reçues : on abandonne le direct

// ------------------------------------------------------------- Curseurs

const cursorsKey = new PluginKey("liveCursors");

/** Curseurs et sélections des autres personnes présentes. État : une Map
 *  id → { name, color, anchor, head }, recalée à chaque modification. */
export function liveCursors() {
  return new Plugin({
    key: cursorsKey,
    state: {
      init: () => new Map(),
      apply(tr, peers) {
        let next = peers;
        if (tr.docChanged && peers.size) {
          next = new Map();
          for (const [id, p] of peers) {
            next.set(id, p.head == null ? p : { ...p, anchor: tr.mapping.map(p.anchor), head: tr.mapping.map(p.head) });
          }
        }
        const meta = tr.getMeta(cursorsKey);
        if (meta?.peers) {
          // Liste des présents : on garde la position de ceux qui restent.
          const kept = new Map();
          for (const p of meta.peers) {
            const old = next.get(p.id);
            const cursor = p.cursor ?? old;
            kept.set(p.id, { name: p.name, color: p.color, anchor: cursor?.anchor ?? null, head: cursor?.head ?? null });
          }
          next = kept;
        } else if (meta?.cursor) {
          const { id, anchor, head } = meta.cursor;
          const p = next.get(id);
          if (p) {
            next = new Map(next);
            next.set(id, { ...p, anchor, head });
          }
        }
        return next;
      },
    },
    props: {
      decorations(state) {
        const peers = cursorsKey.getState(state);
        if (!peers.size) return null;
        const size = state.doc.content.size;
        const clamp = (n) => Math.max(0, Math.min(size, Number(n) || 0));
        const decorations = [];
        for (const [id, p] of peers) {
          if (p.head == null) continue;
          const head = clamp(p.head);
          const anchor = clamp(p.anchor ?? p.head);
          const from = Math.min(head, anchor);
          const to = Math.max(head, anchor);
          if (from < to) decorations.push(Decoration.inline(from, to, { class: "live-sel", "data-accent": p.color }));
          decorations.push(Decoration.widget(head, () => caret(p), { key: `live-${id}-${p.color}-${p.name}`, side: -1 }));
        }
        return DecorationSet.create(state.doc, decorations);
      },
    },
  });
}

/** Trait du curseur d'un autre, avec son prénom au-dessus. Couleur : celle de
 *  son avatar, par l'attribut data-accent (pas de style en ligne : CSP). */
function caret(peer) {
  const el = document.createElement("span");
  el.className = "live-caret";
  el.dataset.accent = peer.color;
  // Les présents sont annoncés par leurs avatars, en haut de la note.
  el.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.className = "live-name";
  label.textContent = peer.name;
  el.append(label);
  return el;
}

// ------------------------------------------------------------- Session

/**
 * Ouvre la note en direct. `hooks` :
 *   url            adresse du canal (/notes/{id}/live)
 *   title          le champ du titre
 *   revisionField  le champ caché de la version (garde-fou classique)
 *   peersBox       où afficher les avatars des présents
 *   setStatus(state, text)  indicateur d'état
 *   buildDoc(markdown, steps)  document à la version donnée (lève une erreur si une étape ne s'applique pas)
 *   createState(doc, version, clientID)  état de l'éditeur avec collab
 *   ready(state)   crée la vue (ou y met l'état) et la renvoie
 *   fallback(dirty)  repli sur l'enregistrement classique
 *   readOnly()     plus d'écriture possible (conflit, plus d'accès)
 *   toMarkdown(doc)
 */
export function startLive(hooks) {
  const { url, title, revisionField, setStatus } = hooks;
  const clientID = randomId();
  let ws = null;
  let view = null;
  let epoch = null;
  let me = null;
  let ended = false;
  let attempts = 0;
  let badSteps = 0;
  let inflight = null;
  let dirty = false;
  let lastCursor = "";
  let titlePending = false; // titre tapé, pas encore envoyé
  const timers = {};

  const open = () => ws && ws.readyState === WebSocket.OPEN;
  const send = (message) => {
    if (open()) ws.send(JSON.stringify(message));
  };
  const later = (name, delay, fn) => {
    clearTimeout(timers[name]);
    timers[name] = setTimeout(fn, delay);
  };
  const clearTimers = () => Object.values(timers).forEach(clearTimeout);

  function connect() {
    if (ended) return;
    const scheme = location.protocol === "https:" ? "wss:" : "ws:";
    try {
      ws = new WebSocket(`${scheme}//${location.host}${url}`);
    } catch {
      giveUp();
      return;
    }
    const socket = ws;
    socket.onmessage = (event) => {
      if (socket !== ws) return;
      let message;
      try {
        message = JSON.parse(event.data);
      } catch {
        return;
      }
      receive(message);
    };
    socket.onclose = () => {
      if (socket !== ws) return;
      ws = null;
      inflight = null;
      if (ended) return;
      // Jamais connecté : enregistrement classique.
      if (!view) {
        giveUp();
        return;
      }
      setStatus("dirty", "Reconnexion…");
      attempts += 1;
      later("retry", Math.min(MAX_RETRY_DELAY, 1000 * 2 ** (attempts - 1)), connect);
    };
  }

  /** Plus de direct : l'éditeur enregistre comme avant (garde-fou de version). */
  function giveUp() {
    if (ended) return;
    ended = true;
    clearTimers();
    detachGlobal();
    const socket = ws;
    ws = null;
    socket?.close();
    hooks.peersBox?.replaceChildren();
    if (view) view.dispatch(view.state.tr.setMeta(cursorsKey, { peers: [] }));
    hooks.fallback(dirty);
  }

  /** Fin du direct sans repli : la note est en lecture seule. */
  function stop(text) {
    ended = true;
    clearTimers();
    detachGlobal();
    ws?.close();
    ws = null;
    setStatus("error", text);
    hooks.peersBox?.replaceChildren();
    if (view) {
      view.dispatch(view.state.tr.setMeta(cursorsKey, { peers: [] }));
      hooks.readOnly();
    }
  }

  function receive(message) {
    switch (message.type) {
      case "init":
        return onInit(message);
      case "steps":
        return onSteps(message);
      case "cursor":
        if (view) view.dispatch(view.state.tr.setMeta(cursorsKey, { cursor: { id: message.peer, ...message.cursor } }));
        return;
      case "peers":
        return onPeers(message.peers);
      case "title":
        // Celui qui tape son titre garde le sien.
        if (document.activeElement !== title) title.value = message.title;
        return;
      case "saved":
        return onSaved(message);
      case "conflict":
        return stop("Modifiée ailleurs entre-temps : copie tes changements, puis recharge la note");
      case "gone":
        return stop("Tu n'as plus accès à cette note");
      case "full":
        return giveUp();
      default:
        // « reset » : la session repart de zéro, la reconnexion s'en charge.
        return;
    }
  }

  function onInit(message) {
    clearTimeout(timers.first);
    attempts = 0;
    me = message.me;
    setRevision(message.revision);
    if (!view) {
      let doc;
      try {
        doc = hooks.buildDoc(message.doc, message.steps);
      } catch {
        giveUp();
        return;
      }
      epoch = message.epoch;
      // Un titre déjà tapé en attendant la session n'est pas écrasé.
      if (!dirty) title.value = message.title;
      view = hooks.ready(hooks.createState(doc, message.version, clientID));
    } else if (message.epoch === epoch) {
      // Reconnexion à la même session : les étapes manquées, puis les nôtres.
      const local = getVersion(view.state);
      if (message.version < local) {
        giveUp();
        return;
      }
      apply(message.steps.slice(local), message.clientIDs.slice(local));
      setStatus(dirty ? "dirty" : "saved", dirty ? "Modifications en cours…" : "Reconnecté");
    } else if (!dirty && !sendableSteps(view.state)) {
      // Nouvelle session et rien en attente chez nous : on repart d'elle.
      let doc;
      try {
        doc = hooks.buildDoc(message.doc, message.steps);
      } catch {
        giveUp();
        return;
      }
      epoch = message.epoch;
      if (document.activeElement !== title) title.value = message.title;
      view = hooks.ready(hooks.createState(doc, message.version, clientID));
      setStatus("saved", "Reconnecté");
    } else {
      // Nouvelle session, mais des modifications n'étaient pas parties : on
      // ne peut pas les recaler. Elles s'enregistrent comme avant, avec le
      // garde-fou de version.
      giveUp();
      return;
    }
    onPeers(message.peers);
    maybeSend();
    sendCursor();
    if (dirty) later("save", SAVE_DELAY, saveNow);
  }

  function onSteps(message) {
    if (!view) return;
    const local = getVersion(view.state);
    if (message.from > local) {
      // Des étapes nous ont échappé : la reconnexion les rattrape.
      resync();
      return;
    }
    const skip = local - message.from; // déjà reçues
    if (skip >= message.steps.length) return;
    const ids = message.clientIDs.slice(skip);
    if (!apply(message.steps.slice(skip), ids)) return;
    inflight = null;
    clearTimeout(timers.resend);
    if (ids.includes(clientID)) later("save", SAVE_DELAY, saveNow);
    maybeSend();
    later("cursor", CURSOR_DELAY, sendCursor);
  }

  function apply(json, clientIDs) {
    if (!json.length) return true;
    try {
      const steps = json.map((s) => Step.fromJSON(schema, s));
      view.dispatch(receiveTransaction(view.state, steps, clientIDs, { mapSelectionBackward: true }));
      badSteps = 0;
      return true;
    } catch {
      badSteps += 1;
      if (badSteps >= MAX_BAD_STEPS) giveUp();
      else resync();
      return false;
    }
  }

  /** Recommence la connexion : le message init rattrape ce qui manque. */
  function resync() {
    ws?.close();
  }

  function onPeers(peers) {
    const others = (peers || []).filter((p) => p.id !== me);
    if (view) view.dispatch(view.state.tr.setMeta(cursorsKey, { peers: others }));
    const box = hooks.peersBox;
    if (!box) return;
    box.replaceChildren(
      ...others.map((p) => {
        const avatar = document.createElement("span");
        avatar.className = "avatar";
        avatar.dataset.accent = p.color;
        avatar.textContent = p.initials;
        avatar.title = `${p.name} est sur la note`;
        return avatar;
      }),
    );
    box.hidden = others.length === 0;
    box.setAttribute("aria-label", others.length ? `Sur la note : ${others.map((p) => p.name).join(", ")}` : "");
  }

  function onSaved(message) {
    setRevision(message.revision);
    if (!view) return;
    if (message.version === getVersion(view.state) && !sendableSteps(view.state) && !titlePending) {
      dirty = false;
    }
    if (!dirty) setStatus("saved", `Enregistré à ${message.at}`);
  }

  function setRevision(revision) {
    if (revisionField && revision != null) revisionField.value = revision;
  }

  /** Envoie les étapes pas encore confirmées, une fois par version. */
  function maybeSend() {
    if (!view || !open()) return;
    const sendable = sendableSteps(view.state);
    if (!sendable || inflight === sendable.version) return;
    send({ type: "steps", version: sendable.version, steps: sendable.steps.map((s) => s.toJSON()), clientID });
    inflight = sendable.version;
    later("resend", RESEND_DELAY, () => {
      inflight = null;
      maybeSend();
    });
  }

  /** Enregistre le Markdown de la version confirmée (rien en attente). */
  function saveNow() {
    if (!view || !open() || !dirty) return;
    if (sendableSteps(view.state)) {
      later("save", SAVE_DELAY, saveNow);
      return;
    }
    send({ type: "save", version: getVersion(view.state), doc: hooks.toMarkdown(view.state.doc), title: title.value });
    setStatus("saving", "Enregistrement…");
  }

  function sendCursor() {
    if (!view || !open() || sendableSteps(view.state)) return;
    const { anchor, head } = view.state.selection;
    const key = `${getVersion(view.state)}:${anchor}:${head}`;
    if (key === lastCursor) return;
    lastCursor = key;
    send({ type: "cursor", cursor: { anchor, head } });
  }

  const onTitle = () => {
    dirty = true;
    setStatus("dirty", "Modifications en cours…");
    titlePending = true;
    later("title", TITLE_DELAY, () => {
      titlePending = false;
      send({ type: "title", title: title.value });
    });
    later("save", SAVE_DELAY, saveNow);
  };
  // Retour sur l'onglet, réseau revenu : on n'attend pas le prochain essai.
  const onWake = () => {
    if (ended || ws || document.visibilityState === "hidden") return;
    clearTimeout(timers.retry);
    connect();
  };
  title.addEventListener("input", onTitle);
  document.addEventListener("visibilitychange", onWake);
  window.addEventListener("online", onWake);
  function detachGlobal() {
    title.removeEventListener("input", onTitle);
    document.removeEventListener("visibilitychange", onWake);
    window.removeEventListener("online", onWake);
  }

  timers.first = setTimeout(() => {
    if (!view) giveUp();
  }, FIRST_TIMEOUT);
  connect();

  return {
    /** Une transaction vient d'être appliquée à la vue. */
    afterTransaction(tr) {
      if (tr.docChanged && !isRemote(tr)) {
        dirty = true;
        setStatus("dirty", "Modifications en cours…");
        maybeSend();
      }
      if (tr.selectionSet || tr.docChanged) later("cursor", CURSOR_DELAY, sendCursor);
    },
    /** Ctrl+S. */
    saveNow,
    /** Des modifications ne sont pas encore enregistrées. */
    unsaved: () => dirty,
    /** On quitte la page : ce qui n'est pas parti part maintenant, avec le
     *  Markdown obtenu (le serveur l'enregistre si nos étapes passent). Rend
     *  faux si le canal est fermé : à l'appelant d'enregistrer autrement. */
    leaving() {
      if (!view) return true;
      if (!open()) return !dirty;
      const sendable = sendableSteps(view.state);
      if (sendable) {
        send({
          type: "steps",
          version: sendable.version,
          steps: sendable.steps.map((s) => s.toJSON()),
          clientID,
          doc: hooks.toMarkdown(view.state.doc),
          title: title.value,
        });
      } else if (dirty) {
        send({ type: "save", version: getVersion(view.state), doc: hooks.toMarkdown(view.state.doc), title: title.value });
      }
      return true;
    },
    /** La page est remplacée : fin de la session. */
    close() {
      this.leaving();
      ended = true;
      clearTimers();
      detachGlobal();
      ws?.close();
      ws = null;
    },
  };
}

function randomId() {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}
