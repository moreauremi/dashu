// Envoi des fichiers, sur la page Fichiers : glisser-déposer ou bouton,
// plusieurs fichiers, une barre de progression par fichier.
//
// Chaque fichier part en morceaux de 50 Mo : Cloudflare refuse toute requête
// de plus de 100 Mo. POST /files/uploads ouvre l'envoi, puis chaque morceau
// part dans un PUT /files/uploads/{id}?offset=…, avec le jeton CSRF de la
// page dans l'en-tête, comme le fait htmx. Un morceau raté est renvoyé (trois
// essais) ; si le serveur attend un autre morceau, il dit lequel (409) et on
// reprend de là. On passe par XMLHttpRequest plutôt que fetch : lui seul donne
// la progression d'un envoi. Quand tout est parti, htmx recharge la liste.
//
// Ce script est chargé par la page à chaque visite (hx-boost) ; les écouteurs
// sont posés sur `document` une seule fois.

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
    $("[data-upload-queue]").append(row);
    return { row, bar, status };
  }

  function fail(job, text) {
    job.row.classList.add("is-error");
    job.bar.remove();
    job.status.textContent = text;
  }

  function addFiles(files) {
    const zone = $("[data-dropzone]");
    if (!zone || !$("[data-upload-queue]")) return;
    const max = Number(zone.dataset.max) || 0;
    for (const file of files) {
      const job = { file, ...addRow(file) };
      if (max && file.size > max) fail(job, `Trop volumineux : ${zone.dataset.maxLabel} au plus`);
      else waiting.push(job);
    }
    pump();
  }

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
      await upload(job);
      job.bar.value = 100;
      job.status.textContent = "Envoyé";
      job.row.classList.add("is-done");
      received += 1;
    } catch (error) {
      fail(job, error.message);
    }
    finished();
  }

  async function upload(job) {
    const { file } = job;
    const name = encodeURIComponent(file.name);
    const opened = await request("POST", `/files/uploads?name=${name}&size=${file.size}`);
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
      if (xhr && xhr.status === 201) return;
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

  function refreshList() {
    if (!window.htmx || !$("#file-results")) return;
    window.htmx.ajax("GET", "/files", {
      target: "#file-results",
      select: "#file-results",
      swap: "outerHTML",
    });
  }

  // --- Bouton : sélecteur de fichiers
  document.addEventListener("change", (event) => {
    const input = event.target;
    if (!(input instanceof HTMLInputElement) || !input.matches("[data-upload-input]")) return;
    addFiles(input.files);
    input.value = "";
  });

  // --- Glisser-déposer : partout sur la page Fichiers
  const carriesFiles = (event) => event.dataTransfer && [...event.dataTransfer.types].includes("Files");

  document.addEventListener("dragover", (event) => {
    const zone = $("[data-dropzone]");
    if (!zone || !carriesFiles(event)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    zone.classList.add("is-over");
  });

  document.addEventListener("dragleave", (event) => {
    // relatedTarget vide : le pointeur a quitté la fenêtre.
    if (event.relatedTarget === null) $("[data-dropzone]")?.classList.remove("is-over");
  });

  document.addEventListener("drop", (event) => {
    const zone = $("[data-dropzone]");
    if (!zone || !carriesFiles(event)) return;
    event.preventDefault();
    zone.classList.remove("is-over");
    addFiles(event.dataTransfer.files);
  });
})();
