// Démo statique de dashu.dev/demo (src/demo). Chargé avant htmx et app.js,
// seulement dans les pages produites par `dashu demo-export`.
//
// Les pages sont des fichiers : rien ne s'écrit, rien ne part. Ce script
//   * traduit chaque lecture (lien, fragment htmx, fetch) vers le fichier
//     exporté, avec la même règle que src/demo/export.rs ;
//   * arrête toute écriture (POST, PUT…) et affiche « Disponible dans la
//     version complète » ;
//   * garde le thème et la taille de l'interface dans ce navigateur
//     (localStorage), et sait tout remettre à zéro.

(() => {
  "use strict";

  const MESSAGE = "Disponible dans la version complète";
  // Le préfixe de la démo, lu dans l'adresse de ce script :
  // /demo/static/js/demo.<empreinte>.js → /demo.
  const BASE = (() => {
    const src = document.currentScript ? new URL(document.currentScript.src).pathname : "";
    const at = src.indexOf("/static/");
    return at > 0 ? src.slice(0, at) : "/demo";
  })();
  const STORE = "demo:";

  const $ = (selector, root = document) => root.querySelector(selector);

  function store(key, value) {
    try {
      if (value === null) localStorage.removeItem(STORE + key);
      else localStorage.setItem(STORE + key, value);
    } catch {}
  }

  function stored(key) {
    try {
      return localStorage.getItem(STORE + key);
    } catch {
      return null;
    }
  }

  // ---------------------------------------------------------- Messages

  let lastToast = 0;
  function toast(text = MESSAGE) {
    // Un seul message à la fois, même si plusieurs requêtes sont refusées.
    if (Date.now() - lastToast < 800) return;
    lastToast = Date.now();
    const node = document.createElement("p");
    node.className = "toast toast-ok";
    node.setAttribute("role", "status");
    node.textContent = text;
    ($("#main") || document.body).append(node);
    setTimeout(() => node.classList.add("is-leaving"), 3500);
    setTimeout(() => node.remove(), 3800);
  }

  /** Écriture refusée : le message, et l'éditeur de notes le dit aussi. */
  function refused() {
    toast();
    setTimeout(() => {
      const status = $("#save-status");
      if (status) {
        status.textContent = "Démo : non enregistré";
        status.dataset.state = "demo";
      }
    }, 50);
  }

  // ---------------------------------------------------------- Adresses

  /** Espace de l'onglet, comme l'en-tête X-Espace d'app.js. */
  function currentSpace() {
    try {
      const tab = JSON.parse($("#app")?.getAttribute("hx-headers") || "{}")["X-Espace"];
      if (tab) return tab;
    } catch {}
    const match = location.pathname.slice(BASE.length).match(/^\/e\/([a-z0-9-]+)/);
    return match ? match[1] : "perso";
  }

  /** Paramètres → nom de dossier : vides retirés, triés par nom, puis tout
   *  ce qui n'est ni lettre ASCII, ni chiffre, ni « . _ - » devient « - ». */
  function querySlug(pairs) {
    const kept = pairs.filter(([, value]) => value !== "");
    kept.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
    const joined = kept.map(([k, v]) => `${k}=${v}`).join("&");
    let slug = [...joined].map((c) => (/^[A-Za-z0-9._-]$/.test(c) ? c : "-")).slice(0, 150).join("");
    if (slug.startsWith(".")) slug = "-" + slug.slice(1);
    return slug;
  }

  /** Fichier exporté pour une lecture : `kind` vaut « html » ou « json ». */
  function mapGet(path, extra = [], kind = "html") {
    const url = new URL(path, location.origin);
    let p = url.pathname;
    const pairs = [...url.searchParams, ...extra];
    if (p === BASE || p.startsWith(BASE + "/")) {
      p = p.slice(BASE.length) || "/";
      if (p.startsWith("/static/") || !pairs.some(([, v]) => v !== "")) return BASE + p;
      // Une page déjà exportée, à laquelle s'ajoutent des paramètres.
      p = p.replace(/\/index\.(html|json)$/, "").replace(/\/_q\/[^/]*\/?$/, "");
    } else if (p.startsWith("/static/")) {
      return BASE + p;
    } else if (!/^\/e\/[a-z0-9-]+(\/|$)/.test(p)) {
      p = `/e/${currentSpace()}${p}`;
    }
    p = p.replace(/\/+$/, "");
    const q = querySlug(pairs);
    return BASE + p + (q ? `/_q/${q}` : "") + (kind === "json" ? "/index.json" : "/");
  }

  // ---------------------------------------------------------- htmx

  document.addEventListener("htmx:configRequest", (event) => {
    const detail = event.detail;
    if (detail.verb !== "get") {
      event.preventDefault();
      refused();
      return;
    }
    const data = detail.formData;
    const extra = data ? [...data.entries()].filter(([, v]) => typeof v === "string") : [];
    detail.path = mapGet(detail.path, extra);
    if (data) for (const key of [...data.keys()]) data.delete(key);
  });

  // Une page absente de la démo (404) ne remplace rien.
  document.addEventListener("htmx:beforeSwap", (event) => {
    if (event.detail.xhr && event.detail.xhr.status >= 400) {
      event.detail.shouldSwap = false;
      event.detail.isError = false;
      // Une recherche libre (paramètre q) n'a pas de page exportée.
      const search = /\/_q\/(?:[^/]*-)?q-/.test(event.detail.pathInfo?.requestPath || "");
      toast(search ? "Recherche libre dans la version complète ; ici, les tags fonctionnent" : MESSAGE);
    }
  });
  document.addEventListener("htmx:sendError", () => toast());

  // ---------------------------------------------------------- Clics et formulaires

  document.addEventListener(
    "click",
    (event) => {
      const target = event.target instanceof Element ? event.target : null;
      if (!target) return;
      if (target.closest("[data-demo-reset]")) {
        event.preventDefault();
        reset();
        return;
      }
      // « Envoyer des fichiers », photo de profil : un label autour d'un
      // champ de fichier (désactivé dans la démo, voir applyLook).
      const label = target.closest("label");
      if (target.closest("[data-demo-off]") || (label && label.control?.type === "file")) {
        event.preventDefault();
        event.stopImmediatePropagation();
        toast();
      }
    },
    true,
  );

  const WRITES = ["hx-post", "hx-put", "hx-patch", "hx-delete"];

  document.addEventListener(
    "submit",
    (event) => {
      const form = event.target;
      if (!(form instanceof HTMLFormElement)) return;
      const write = form.method === "post" || WRITES.some((a) => form.hasAttribute(a));
      if (!write) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      const action = new URL(form.getAttribute("action") || location.href, location.origin).pathname;
      const button = event.submitter;
      if (action === "/space") return changeSpace(form);
      if (action === "/settings/size" && button) return setSize(button.value);
      if (action === "/settings/theme" && button) return setTheme(button.value);
      refused();
    },
    true,
  );

  /** L'île (ou le balayage) : la même page dans l'autre espace. */
  function changeSpace(form) {
    const slug = form.elements.slug?.value;
    if (!slug) return;
    const next = (form.elements.next?.value || "/").split("?")[0];
    const page = ["/", "/notes", "/files", "/calendar"].includes(next) ? next : "/";
    const url = mapGet(`/e/${slug}${page}`);
    if (window.htmx) {
      window.htmx.ajax("GET", url, {
        source: "#app",
        target: "body",
        swap: "innerHTML transition:true",
        headers: { "HX-Boosted": "true" },
        push: "true",
      });
    } else {
      location.assign(url);
    }
  }

  // ---------------------------------------------------------- Apparence

  function setSize(value) {
    store("size", value === "normal" ? null : value);
    applyLook();
  }

  function setTheme(value) {
    store("theme", value === "system" ? null : value);
    applyLook();
  }

  /** Thème et taille choisis dans ce navigateur, sur la page affichée. */
  function applyLook() {
    const app = $("#app");
    if (!app) return;
    const size = stored("size");
    const theme = stored("theme");
    if (size) app.dataset.size = size;
    else delete app.dataset.size;
    if (theme) {
      app.dataset.theme = theme;
      document.documentElement.dataset.theme = theme;
    } else {
      app.dataset.theme = "";
      delete document.documentElement.dataset.theme;
    }
    document.querySelectorAll('form[action="/settings/size"] button').forEach((b) => {
      b.setAttribute("aria-pressed", String(b.value === (size || "normal")));
    });
    document.querySelectorAll('form[action="/settings/theme"] button').forEach((b) => {
      b.setAttribute("aria-pressed", String(b.value === (theme || "system")));
    });
    // L'envoi de fichiers reste visible mais ne s'ouvre pas.
    document.querySelectorAll('input[type="file"]').forEach((input) => {
      input.disabled = true;
      input.title = MESSAGE;
    });
  }

  document.addEventListener("DOMContentLoaded", applyLook);
  document.addEventListener("htmx:afterSettle", applyLook);

  // Fichiers glissés sur la page : rien n'est envoyé.
  ["dragover", "drop"].forEach((type) =>
    document.addEventListener(
      type,
      (event) => {
        if (![...(event.dataTransfer?.types || [])].includes("Files")) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        if (type === "drop") toast();
      },
      true,
    ),
  );

  /** « Réinitialiser la démo » : oublie les choix faits ici, revient au début. */
  function reset() {
    try {
      for (const storage of [localStorage, sessionStorage]) {
        Object.keys(storage)
          .filter((key) => key.startsWith(STORE) || key.startsWith("dashu"))
          .forEach((key) => storage.removeItem(key));
      }
    } catch {}
    location.assign(`${BASE}/e/perso/`);
  }

  // ---------------------------------------------------------- fetch et XHR

  const json = (value, status = 200) =>
    Promise.resolve(new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } }));

  const realFetch = window.fetch.bind(window);
  window.fetch = (input, init = {}) => {
    const raw = typeof input === "string" || input instanceof URL ? String(input) : input.url;
    const url = new URL(raw, location.href);
    const method = String(init.method || (input instanceof Request ? input.method : "GET")).toUpperCase();
    if (url.origin !== location.origin) return Promise.reject(new TypeError("hors de la démo"));
    if (method !== "GET" && method !== "HEAD") {
      refused();
      return json({ error: MESSAGE }, 403);
    }
    const p = url.pathname;
    if (p === "/health") return Promise.resolve(new Response("ok"));
    if (p === "/cmd/suggest") return json({ items: [] });
    if (p === BASE || p.startsWith(BASE + "/")) return realFetch(input, init);
    return realFetch(mapGet(p + url.search, [], p === "/cmd/catalogue" ? "json" : "html"), { method });
  };

  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    this.demoRefused = !/^(GET|HEAD)$/i.test(method);
    return open.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (body) {
    if (!this.demoRefused) return send.call(this, body);
    refused();
    setTimeout(() => {
      this.dispatchEvent(new ProgressEvent("error"));
      this.dispatchEvent(new ProgressEvent("loadend"));
    }, 0);
  };

  // Pas de notes en direct dans la démo.
  window.WebSocket = function () {
    throw new Error("pas de connexion en direct dans la démo");
  };
})();
