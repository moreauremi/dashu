// Envoi des fichiers, sur la page Fichiers : glisser-déposer ou bouton,
// plusieurs fichiers, une barre de progression par fichier.
//
// Chaque fichier part seul, tel quel, dans le corps d'un POST /files?name=…
// avec le jeton CSRF de la page dans l'en-tête, comme le fait htmx. On passe
// par XMLHttpRequest plutôt que fetch : lui seul donne la progression d'un
// envoi. Quand tout est parti, la liste est rechargée par htmx.
//
// Ce script est chargé par la page à chaque visite (hx-boost) ; les écouteurs
// sont posés sur `document` une seule fois.

(() => {
  "use strict";
  if (window.dashuFiles) return;
  window.dashuFiles = true;

  /** Envois simultanés au plus : le serveur est un vieux portable. */
  const PARALLEL = 2;
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

  function send(job) {
    running += 1;
    job.status.textContent = "0 %";
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `/files?name=${encodeURIComponent(job.file.name)}`);
    xhr.setRequestHeader("X-CSRF-Token", csrfToken());
    // Les erreurs reviennent alors en une ligne, comme pour un fragment htmx.
    xhr.setRequestHeader("HX-Request", "true");
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.upload.addEventListener("progress", (event) => {
      if (!event.lengthComputable) return;
      job.bar.value = Math.round((event.loaded / event.total) * 100);
      job.status.textContent = `${job.bar.value} %`;
    });
    xhr.addEventListener("load", () => {
      if (xhr.status === 201) {
        job.bar.value = 100;
        job.status.textContent = "Envoyé";
        job.row.classList.add("is-done");
        received += 1;
      } else {
        fail(job, errorText(xhr));
      }
      finished();
    });
    xhr.addEventListener("error", () => {
      fail(job, "Connexion perdue pendant l'envoi");
      finished();
    });
    xhr.send(job.file);
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
    // Une 413 sans notre message vient de Cloudflare : 100 Mo par requête au
    // plus sur l'offre gratuite.
    if (xhr.status === 413) return "Trop volumineux pour Cloudflare (100 Mo) : passe par l'adresse Tailscale";
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
