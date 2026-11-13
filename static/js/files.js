// Envoi des fichiers : sur les pages Fichiers et Outils (zone de dépôt ou
// bouton), mais aussi partout ailleurs (un fichier lâché sur n'importe quelle
// page part dans l'espace de la page) et depuis l'éditeur de notes (image
// collée, voir frontend/editor.js et `window.dashuUpload`). Plusieurs
// fichiers, une barre de progression par fichier.
//
// Chaque fichier part en morceaux de 50 Mo : Cloudflare refuse toute requête
// de plus de 100 Mo. POST /files/uploads ouvre l'envoi, puis chaque morceau
// part dans un PUT /files/uploads/{id}?offset=…, avec le jeton CSRF de la
// page dans l'en-tête, comme le fait htmx. Un morceau raté est renvoyé (trois
// essais) ; si le serveur attend un autre morceau, il dit lequel (409) et on
// reprend de là. On passe par XMLHttpRequest plutôt que fetch : lui seul donne
// la progression d'un envoi. Quand tout est parti, htmx recharge la liste.
//
// Ce script est chargé une fois par base.html ; les écouteurs sont posés sur
// `document`, et survivent donc aux navigations htmx.

(() => {
  "use strict";
  if (window.dashuFiles) return;
  window.dashuFiles = true;

  /** Envois simultanés au plus : le serveur est un vieux portable. */
  const PARALLEL = 2;
  /** Taille d'un morceau : 50 Mo, sous la limite de Cloudflare. */
  const CHUNK = 50 * 1024 * 1024;
  const TRIES = 3;
  const waiting = [];
  let running = 0;
  let received = 0;

  const $ = (selector) => document.querySelector(selector);

  function csrfToken() {
    try {
      return JSON.parse($("[hx-headers]").getAttribute("hx-headers"))["X-CSRF-Token"] || "";
    } catch {
      return "";
    }
  }

  function tabSpace() {
    try {
      return JSON.parse($("[hx-headers]").getAttribute("hx-headers"))["X-Espace"] || "";
    } catch {
      return "";
    }
  }

  /** Une ligne dans la file d'envoi : nom, barre, état. */
  function addRow(file) {
    const row = document.createElement("li");
    row.className = "upload-item";
    const name = document.createElement("span");
    name.className = "upload-name";
    name.textContent = file.name;
    const bar = document.createElement("progress");
    bar.max = 100;
    bar.value = 0;
    const status = document.createElement("span");
    status.className = "upload-status";
    status.textContent = "En attente";
    row.append(name, bar, status);
    queue().append(row);
    return { row, bar, status };
  }

  /** La file d'envoi de la page, ou une file flottante (dépôt hors des pages
   *  Fichiers et Outils), posée sur `body` : elle survit aux navigations. */
  function queue() {
    let list = $("[data-upload-queue]");
    if (!list) {
      list = document.createElement("ul");
      list.className = "upload-queue is-floating";
      list.setAttribute("role", "list");
      list.setAttribute("aria-live", "polite");
      list.dataset.uploadQueue = "";
      document.body.append(list);
    }
    return list;
  }

  function fail(job, text) {
    job.row.classList.add("is-error");
    job.bar.remove();
    job.status.textContent = text;
  }

  /** Met des fichiers dans la file d'envoi. `where` : un espace et un
   *  dossier précis (fichiers lâchés sur un dossier de l'onglet Fichiers) ;
   *  sans lui, ce que dit la page. */
  function addFiles(files, where) {
    const zone = $("[data-dropzone]");
    const max = Number(zone?.dataset.max) || 0;
    for (const file of files) {
      const job = { file, where, ...addRow(file) };
      if (max && file.size > max) fail(job, `Trop volumineux : ${zone.dataset.maxLabel} au plus`);
      else waiting.push(job);
    }
    pump();
  }

  /** Envoie un fichier dans l'espace de la page et rend son identifiant
   *  (éditeur de notes : image collée). `name` remplace le nom du fichier. */
  window.dashuUpload = (file, name) =>
    new Promise((resolve, reject) => {
      waiting.push({ file, name, resolve, reject, ...addRow({ name: name || file.name }) });
      pump();
    });

  function pump() {
    while (running < PARALLEL && waiting.length) send(waiting.shift());
    if (running === 0 && waiting.length === 0 && received > 0) {
      received = 0;
      refreshList();
    }
  }

  /** Une requête, sous forme de promesse. `onProgress` reçoit les octets partis. */
  function request(method, url, body, onProgress) {
    return new Promise((resolve) => {
      const xhr = new XMLHttpRequest();
      xhr.open(method, url);
      xhr.setRequestHeader("X-CSRF-Token", csrfToken());
      // L'espace de l'onglet, comme pour une requête htmx.
      xhr.setRequestHeader("X-Espace", tabSpace());
      // Les erreurs reviennent alors en une ligne, comme pour un fragment htmx.
      xhr.setRequestHeader("HX-Request", "true");
      if (body) xhr.setRequestHeader("Content-Type", "application/octet-stream");
      if (onProgress) {
        xhr.upload.addEventListener("progress", (event) => onProgress(event.loaded));
      }
      xhr.addEventListener("load", () => resolve(xhr));
      xhr.addEventListener("error", () => resolve(null));
      xhr.send(body);
    });
  }

  async function send(job) {
    running += 1;
    try {
      const id = await upload(job);
      job.bar.value = 100;
      job.row.classList.add("is-done");
      // Hors des pages Fichiers et Outils : un lien vers le fichier arrivé.
      if (id && !$("#file-results, #tool-results")) {
        const link = document.createElement("a");
        link.href = `/files/${id}`;
        link.textContent = "Envoyé · voir";
        job.status.replaceChildren(link);
      } else {
        job.status.textContent = "Envoyé";
      }
      received += 1;
      job.resolve?.(id);
    } catch (error) {
      fail(job, error.message);
      job.reject?.(error);
    }
    finished();
  }

  async function upload(job) {
    const { file } = job;
    const name = encodeURIComponent(job.name || file.name);
    // Espace choisi au-dessus de la zone de dépôt (l'espace actif par défaut),
    // ou dossier ouvert dans l'onglet Fichiers (il donne son espace) ;
    // onglet Outils : un fichier pas encore rangé.
    const zone = document.querySelector("[data-dropzone]");
    const folder = job.where ? job.where.folder : zone?.dataset.folder;
    const space = job.where
      ? job.where.space
      : folder
        ? ""
        : document.querySelector("[data-upload-space]:checked")?.value;
    let where = space ? `&space=${encodeURIComponent(space)}` : "";
    if (folder) where += `&folder=${encodeURIComponent(folder)}`;
    if (zone?.hasAttribute("data-tools")) where += "&tools=1";
    const opened = await request("POST", `/files/uploads?name=${name}&size=${file.size}${where}`);
    if (!opened) throw new Error("Connexion perdue pendant l'envoi");
    if (opened.status !== 201) throw new Error(errorText(opened));
    const id = opened.responseText.trim();

    const show = (sent) => {
      const percent = file.size ? Math.floor((sent / file.size) * 100) : 100;
      job.bar.value = percent;
      job.status.textContent = `${percent} %`;
    };
    show(0);

    let offset = 0;
    let tries = 0;
    for (;;) {
      const chunk = file.slice(offset, offset + CHUNK);
      const xhr = await request("PUT", `/files/uploads/${id}?offset=${offset}`, chunk, (sent) =>
        show(offset + sent),
      );
      // Dernier morceau : le serveur donne l'identifiant du fichier rangé.
      if (xhr && xhr.status === 201) return xhr.getResponseHeader("x-file-id");
      if (xhr && xhr.status === 200) {
        offset = Number(xhr.getResponseHeader("x-upload-received"));
        tries = 0;
        continue;
      }
      if (xhr && xhr.status === 409) {
        // Le serveur attend un autre morceau : on reprend où il en est.
        offset = Number(xhr.getResponseHeader("x-upload-received"));
        continue;
      }
      // Coupure ou erreur passagère : on renvoie le même morceau.
      tries += 1;
      if (tries < TRIES && (!xhr || xhr.status >= 500 || xhr.status === 400)) {
        job.status.textContent = "Nouvel essai…";
        await new Promise((resolve) => setTimeout(resolve, 2000 * tries));
        continue;
      }
      throw new Error(xhr ? errorText(xhr) : "Connexion perdue pendant l'envoi");
    }
  }

  function finished() {
    running -= 1;
    pump();
  }

  /** Message à afficher pour un envoi refusé. */
  function errorText(xhr) {
    // Nos refus arrivent en <p class="frag-error">…</p>. DOMParser lit le
    // texte sans rien exécuter.
    const doc = new DOMParser().parseFromString(xhr.responseText || "", "text/html");
    const message = doc.querySelector(".frag-error")?.textContent;
    if (message) return message;
    if (xhr.status === 403) return "Page expirée : recharge-la et réessaie";
    return `Échec de l'envoi (erreur ${xhr.status})`;
  }

  /** Recharge la liste de la page (Fichiers ou Outils). */
  function refreshList() {
    const results = $("#file-results, #tool-results");
    if (!window.htmx || !results) return;
    const target = `#${results.id}`;
    // L'adresse entière : dans l'onglet Fichiers, ?dossier=… dit quel dossier.
    window.htmx.ajax("GET", location.pathname + location.search, { target, select: target, swap: "outerHTML" });
  }

  // --- Bouton : sélecteur de fichiers
  document.addEventListener("change", (event) => {
    const input = event.target;
    if (!(input instanceof HTMLInputElement) || !input.matches("[data-upload-input]")) return;
    addFiles(input.files);
    input.value = "";
  });

  // --- Glisser-déposer : sur les pages Fichiers et Outils, la zone de dépôt
  // s'allume ; ailleurs, un voile dit dans quel espace le fichier va partir.
  // L'éditeur de notes gère lui-même ce qu'on y lâche (image insérée).
  /** Ligne de l'onglet Fichiers en train d'être glissée (voir « Ranger en
   *  glissant ») : ce n'est pas un envoi, même si Chrome joint sa vignette
   *  comme un fichier. */
  let dragged = null;
  const carriesFiles = (event) =>
    !dragged && event.dataTransfer && [...event.dataTransfer.types].includes("Files");
  const inEditor = (event) => event.target instanceof Element && !!event.target.closest(".ProseMirror");

  /** Voile du dépôt hors des pages Fichiers et Outils. */
  function veil(show) {
    let el = $(".drop-veil");
    if (!show) {
      el?.classList.remove("is-visible");
      return;
    }
    if (!el) {
      el = document.createElement("div");
      el.className = "drop-veil";
      el.setAttribute("aria-hidden", "true");
      el.append(document.createElement("p"));
      document.body.append(el);
    }
    const space = $("[data-island] .isl-btn[aria-current='true'] .isl-label")?.textContent?.trim();
    el.firstChild.textContent = space ? `Lâcher pour envoyer dans ${space}` : "Lâcher pour envoyer";
    el.classList.add("is-visible");
  }

  document.addEventListener("dragover", (event) => {
    if (!carriesFiles(event) || !$("#app") || inEditor(event)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    const zone = $("[data-dropzone]");
    // Au-dessus d'un dossier, c'est lui qui s'allume (voir « Ranger en glissant »).
    if (zone) zone.classList.toggle("is-over", !dropTarget(event));
    else veil(true);
  });

  document.addEventListener("dragleave", (event) => {
    // relatedTarget vide : le pointeur a quitté la fenêtre.
    if (event.relatedTarget !== null) return;
    $("[data-dropzone]")?.classList.remove("is-over");
    veil(false);
    mark(null);
  });

  document.addEventListener("drop", (event) => {
    if (!carriesFiles(event) || !$("#app") || event.defaultPrevented || inEditor(event)) return;
    event.preventDefault();
    $("[data-dropzone]")?.classList.remove("is-over");
    veil(false);
    // Lâchés sur un dossier de l'onglet Fichiers : ils y arrivent.
    const target = dropTarget(event);
    mark(null);
    addFiles(
      event.dataTransfer.files,
      target ? { space: target.dataset.dropSpace, folder: target.dataset.dropFolder } : undefined,
    );
  });

  // --- Ranger en glissant (onglet Fichiers, SPEC « Dossiers ») : un fichier,
  // une note ou un dossier lâché sur un dossier (une ligne, le dossier d'un
  // espace, un pas du fil d'Ariane) y est rangé. La ligne porte l'adresse de
  // son rangement (data-drag-move), le dossier son espace et son numéro
  // (data-drop-space, data-drop-folder : vide pour la racine de l'espace).
  // Le serveur vérifie tout (niveaux, homonymes) et dit ce qu'il a fait : la
  // page est rechargée avec son message. La fenêtre « Ranger » reste le
  // chemin au clavier et au doigt.

  /** Dossier sous le pointeur, s'il en est un. */
  function dropTarget(event) {
    return event.target instanceof Element ? event.target.closest("[data-drop-space]") : null;
  }

  /** Allume un seul dossier : celui où l'on va lâcher. */
  function mark(target) {
    document.querySelectorAll(".is-drop-target").forEach((el) => {
      if (el !== target) el.classList.remove("is-drop-target");
    });
    target?.classList.add("is-drop-target");
  }

  /** Un dossier ne se lâche pas sur lui-même. */
  const accepts = (target) =>
    !!target && !(dragged?.folder && target.dataset.dropFolder === dragged.folder);

  document.addEventListener("dragstart", (event) => {
    const row = event.target instanceof Element ? event.target.closest("[data-drag-move]") : null;
    if (!row) return;
    dragged = { url: row.dataset.dragMove, folder: row.dataset.dragFolder || "" };
    event.dataTransfer.effectAllowed = "move";
    // Firefox ne commence le glissement qu'avec une donnée.
    event.dataTransfer.setData("text/plain", row.querySelector("a")?.textContent?.trim() || "");
    row.classList.add("is-dragging");
  });

  document.addEventListener("dragend", () => {
    document.querySelector(".is-dragging")?.classList.remove("is-dragging");
    mark(null);
    dragged = null;
  });

  document.addEventListener("dragover", (event) => {
    const target = dropTarget(event);
    const fromComputer = carriesFiles(event);
    if (!dragged && !fromComputer) return;
    mark(accepts(target) ? target : null);
    if (dragged && accepts(target)) {
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
    }
  });

  document.addEventListener("drop", (event) => {
    if (!dragged) return;
    event.preventDefault();
    const target = dropTarget(event);
    const move = dragged;
    dragged = null;
    mark(null);
    if (accepts(target)) rangeTo(move.url, target.dataset.dropSpace, target.dataset.dropFolder);
  });

  /** Envoie le rangement (le formulaire de la fenêtre « Ranger »), puis
   *  recharge la page : le serveur y met son message (rangé, ou refusé). */
  async function rangeTo(url, space, folder) {
    const here = location.pathname + location.search;
    const body = new URLSearchParams({ space_id: space, folder_id: folder || "", next: here });
    try {
      await fetch(url, {
        method: "POST",
        headers: {
          "X-CSRF-Token": csrfToken(),
          "X-Espace": tabSpace(),
          "Content-Type": "application/x-www-form-urlencoded",
        },
        body,
        // La redirection n'est pas suivie : c'est la page rechargée qui lit
        // le message laissé par le serveur.
        redirect: "manual",
      });
    } catch {
      // Connexion perdue : la page rechargée montre que rien n'a bougé.
    }
    window.htmx?.ajax("GET", here, { target: "body", swap: "innerHTML transition:true" });
  }
})();
