/* Horizon : le haut de la page d'accueil et les vignettes des cartes.
   Une courbe d'horizon reste toujours au même endroit ; ce qu'il y a dessous
   change toutes les 400 ms (aube, orbite, aurore, dune, cahier, quadrillage,
   et les captures de l'application posées comme une planète). Le raccord se
   fait sur la forme : le contenu change, la courbe reste.
   2,5 changements par seconde au plus (sous le seuil de 3 par seconde pour
   les personnes photosensibles), pause possible, arrêt si le système demande
   moins d'animations, rien ne tourne hors de l'écran. Aucune ressource externe. */
(() => {
  const CADENCE = 400;
  const calme = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Géométrie commune : sommet de la courbe à 58 % de la hauteur, rayon 1,35 × la largeur
  // (assez grand pour une courbe douce, assez petit pour qu'elle se voie sur téléphone).
  const courbe = (L, H) => {
    const R = Math.max(L * 1.35, H * 0.9);
    return { cx: L / 2, cy: H * 0.58 + R, R, sommet: H * 0.58 };
  };
  const hauteurSur = (g, x) => g.cy - Math.sqrt(Math.max(0, g.R * g.R - (x - g.cx) ** 2));

  // Générateur pseudo-aléatoire stable : les étoiles ne bougent pas d'une image à l'autre.
  const graine = (n) => () => ((n = (n * 16807) % 2147483647) / 2147483647);

  const disque = (ctx, g) => { ctx.beginPath(); ctx.arc(g.cx, g.cy, g.R, 0, Math.PI * 2); };
  const ciel = (ctx, L, H, haut, bas) => {
    const d = ctx.createLinearGradient(0, 0, 0, H);
    d.addColorStop(0, haut); d.addColorStop(0.62, bas);
    ctx.fillStyle = d; ctx.fillRect(0, 0, L, H);
  };
  const halo = (ctx, g, couleur, largeur, alpha) => {
    for (let i = 4; i >= 1; i--) {
      ctx.save(); disque(ctx, g);
      ctx.strokeStyle = couleur; ctx.globalAlpha = alpha / i; ctx.lineWidth = largeur * i * i;
      ctx.stroke(); ctx.restore();
    }
  };
  const arcsParalleles = (ctx, g, pas, couleur, n, depart = 0) => {
    ctx.save(); disque(ctx, g); ctx.clip();
    ctx.strokeStyle = couleur; ctx.lineWidth = 1;
    for (let i = 1; i <= n; i++) {
      ctx.beginPath(); ctx.arc(g.cx, g.cy, g.R - depart - i * pas, 0, Math.PI * 2); ctx.stroke();
    }
    ctx.restore();
  };

  const images = {};
  const charge = (src) => {
    if (images[src]) return images[src];
    const im = new Image(); im.decoding = "async"; im.src = src;
    images[src] = im; return im;
  };
  const pret = (im) => im && im.complete && im.naturalWidth > 0;

  // Chaque scène : (contexte, largeur, hauteur, géométrie) → dessine tout le cadre.
  // `encre` = couleur de la petite barre qui marche sur l'horizon.
  const SCENES = {
    aube: { encre: "#14202c", dessine(ctx, L, H, g) {
      ciel(ctx, L, H, "#e8dccd", "#f0b98e");
      const soleil = ctx.createRadialGradient(g.cx, g.sommet, 0, g.cx, g.sommet, L * 0.35);
      soleil.addColorStop(0, "rgba(255,236,200,.95)"); soleil.addColorStop(1, "rgba(255,236,200,0)");
      ctx.fillStyle = soleil; ctx.fillRect(0, 0, L, H);
      ctx.save(); disque(ctx, g);
      const mer = ctx.createLinearGradient(0, g.sommet, 0, H);
      mer.addColorStop(0, "#3d6f93"); mer.addColorStop(1, "#173a57");
      ctx.fillStyle = mer; ctx.fill(); ctx.clip();
      const reflet = ctx.createLinearGradient(g.cx - L * 0.06, 0, g.cx + L * 0.06, 0);
      reflet.addColorStop(0, "rgba(255,220,180,0)"); reflet.addColorStop(0.5, "rgba(255,220,180,.35)"); reflet.addColorStop(1, "rgba(255,220,180,0)");
      ctx.fillStyle = reflet; ctx.fillRect(g.cx - L * 0.06, g.sommet, L * 0.12, H);
      ctx.restore();
      arcsParalleles(ctx, g, H * 0.035, "rgba(255,255,255,.07)", 12, H * 0.01);
    } },
    orbite: { encre: "#f4f2ee", dessine(ctx, L, H, g) {
      ctx.fillStyle = "#050507"; ctx.fillRect(0, 0, L, H);
      const r = graine(7);
      for (let i = 0; i < 140; i++) {
        ctx.fillStyle = `rgba(255,255,255,${0.2 + r() * 0.6})`;
        ctx.fillRect(r() * L, r() * g.sommet, r() < 0.9 ? 1 : 2, 1);
      }
      ctx.save(); disque(ctx, g); ctx.fillStyle = "#07070c"; ctx.fill(); ctx.restore();
      halo(ctx, g, "#7f8cff", 1.2, 0.55);
      ctx.save(); disque(ctx, g); ctx.strokeStyle = "#dfe3ff"; ctx.lineWidth = 1.2; ctx.stroke(); ctx.restore();
      const x = g.cx - L * 0.28, y = hauteurSur(g, x);
      const eclat = ctx.createRadialGradient(x, y, 0, x, y, L * 0.12);
      eclat.addColorStop(0, "rgba(255,245,230,1)"); eclat.addColorStop(0.08, "rgba(255,200,150,.6)"); eclat.addColorStop(1, "rgba(255,160,100,0)");
      ctx.fillStyle = eclat; ctx.fillRect(0, 0, L, H);
    } },
    aurore: { encre: "#e8fff1", dessine(ctx, L, H, g) {
      ctx.fillStyle = "#04070a"; ctx.fillRect(0, 0, L, H);
      const r = graine(19);
      for (let i = 0; i < 90; i++) { ctx.fillStyle = `rgba(255,255,255,${r() * 0.5})`; ctx.fillRect(r() * L, r() * g.sommet * 0.8, 1, 1); }
      halo(ctx, g, "#3dff8f", 2.2, 0.5);
      halo(ctx, g, "#c04cff", 5, 0.12);
      ctx.save(); disque(ctx, g);
      const sol = ctx.createLinearGradient(0, g.sommet, 0, H);
      sol.addColorStop(0, "#0d2a1b"); sol.addColorStop(1, "#040b07");
      ctx.fillStyle = sol; ctx.fill(); ctx.restore();
      ctx.save(); disque(ctx, g); ctx.strokeStyle = "#9dffc4"; ctx.lineWidth = 1.5; ctx.stroke(); ctx.restore();
    } },
    dune: { encre: "#2b1a0c", dessine(ctx, L, H, g) {
      ciel(ctx, L, H, "#efe7da", "#f3dcc0");
      ctx.save(); disque(ctx, g);
      const sable = ctx.createLinearGradient(0, g.sommet, 0, H);
      sable.addColorStop(0, "#d39a5f"); sable.addColorStop(1, "#7d4f27");
      ctx.fillStyle = sable; ctx.fill(); ctx.restore();
      arcsParalleles(ctx, g, H * 0.022, "rgba(80,40,10,.12)", 22, H * 0.006);
    } },
    cahier: { encre: "#1b1b19", dessine(ctx, L, H, g) {
      ctx.fillStyle = "#f6f4ee"; ctx.fillRect(0, 0, L, H);
      ctx.strokeStyle = "rgba(60,90,160,.16)"; ctx.lineWidth = 1;
      for (let y = H * 0.08; y < g.sommet; y += H * 0.045) { ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(L, y); ctx.stroke(); }
      ctx.save(); disque(ctx, g); ctx.fillStyle = "#ebe6da"; ctx.fill(); ctx.restore();
      arcsParalleles(ctx, g, H * 0.045, "rgba(60,90,160,.22)", 14);
      ctx.save(); disque(ctx, g); ctx.strokeStyle = "#c2452f"; ctx.lineWidth = 1.5; ctx.stroke(); ctx.restore();
    } },
    quadrillage: { encre: "#10306b", dessine(ctx, L, H, g) {
      ctx.fillStyle = "#f3f5f8"; ctx.fillRect(0, 0, L, H);
      const pas = Math.max(14, L / 48);
      ctx.strokeStyle = "rgba(30,86,196,.10)"; ctx.lineWidth = 1;
      for (let x = 0; x < L; x += pas) { ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, H); ctx.stroke(); }
      for (let y = 0; y < H; y += pas) { ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(L, y); ctx.stroke(); }
      ctx.save(); disque(ctx, g); ctx.fillStyle = "rgba(30,86,196,.06)"; ctx.fill(); ctx.strokeStyle = "#1e56c4"; ctx.lineWidth = 1.5; ctx.stroke(); ctx.restore();
      ctx.strokeStyle = "#1e56c4"; ctx.lineWidth = 1;
      for (const f of [-0.3, 0, 0.3]) {
        const x = g.cx + f * L, y = hauteurSur(g, x);
        ctx.beginPath(); ctx.moveTo(x - 8, y); ctx.lineTo(x + 8, y); ctx.moveTo(x, y - 8); ctx.lineTo(x, y + 8); ctx.stroke();
      }
    } },
  };
  // Les captures de l'application, posées sous la courbe comme une planète.
  const capture = (src, cielHaut, cielBas, encre, cadrage = 0.18) => ({ encre, image: src, dessine(ctx, L, H, g) {
    ciel(ctx, L, H, cielHaut, cielBas);
    const im = images[src];
    ctx.save(); disque(ctx, g); ctx.fillStyle = "#151514"; ctx.fill(); ctx.clip();
    if (pret(im)) {
      const l = L * 1.08, h = l * im.naturalHeight / im.naturalWidth;
      ctx.drawImage(im, (L - l) / 2, g.sommet - h * cadrage, l, h);
    }
    ctx.restore();
    halo(ctx, g, "#ffffff", 0.8, 0.35);
  } });
  SCENES.agenda = capture("captures/agenda.webp", "#0d0d0c", "#2a2621", "#f4f2ee", 0.12);
  SCENES.dashboard = capture("captures/dashboard.webp", "#141414", "#36302a", "#f4f2ee", 0.16);
  SCENES.notes = capture("captures/notes-a-plusieurs.webp", "#e9e6df", "#cfc7bb", "#1b1b19", 0.2);
  SCENES.fichiers = capture("captures/fichiers.webp", "#0f1013", "#26303c", "#f4f2ee", 0.14);

  const ORDRE = ["aube", "agenda", "orbite", "cahier", "dashboard", "aurore", "quadrillage", "notes", "dune", "fichiers"];

  const prepare = (canvas) => {
    const ctx = canvas.getContext("2d");
    const taille = () => {
      const r = canvas.getBoundingClientRect(), d = Math.min(devicePixelRatio || 1, 2);
      canvas.width = Math.round(r.width * d); canvas.height = Math.round(r.height * d);
      ctx.setTransform(d, 0, 0, d, 0, 0);
      return { L: r.width, H: r.height };
    };
    return { ctx, taille };
  };
  const dessine = (ctx, L, H, nom) => {
    const s = SCENES[nom];
    ctx.clearRect(0, 0, L, H);
    s.dessine(ctx, L, H, courbe(L, H));
    return s;
  };

  // Vignettes des cartes : une scène fixe chacune, même courbe.
  for (const c of document.querySelectorAll("canvas[data-scene]")) {
    const { ctx, taille } = prepare(c);
    const nom = c.dataset.scene;
    const peins = () => { const { L, H } = taille(); dessine(ctx, L, H, nom); };
    if (SCENES[nom].image) charge(SCENES[nom].image).addEventListener("load", peins, { once: true });
    new ResizeObserver(peins).observe(c);
  }

  // Le cadre du haut de page.
  const cadre = document.querySelector(".horizon");
  if (!cadre) return;
  const canvas = cadre.querySelector("canvas");
  const marcheur = cadre.querySelector(".marcheur");
  const bouton = cadre.querySelector(".pause");
  const { ctx, taille } = prepare(canvas);
  for (const n of ORDRE) if (SCENES[n].image) charge(SCENES[n].image);

  let L = 0, H = 0, i = 0, enPause = calme, visible = true, minuteur = null, depart = performance.now();
  const peins = () => {
    // On saute une capture pas encore chargée plutôt que d'afficher un trou.
    for (let k = 0; k < ORDRE.length && SCENES[ORDRE[i]].image && !pret(images[SCENES[ORDRE[i]].image]); k++) i = (i + 1) % ORDRE.length;
    const s = dessine(ctx, L, H, ORDRE[i]);
    marcheur.style.color = s.encre;
  };
  const redimensionne = () => { ({ L, H } = taille()); peins(); place(performance.now()); };
  // La petite barre avance le long de la courbe en 14 s, d'un bord à l'autre, sans s'arrêter aux coupes.
  const place = (t) => {
    const p = enPause ? 0.42 : (((t - depart) / 14000) % 1);
    const g = courbe(L, H), x = L * (0.08 + p * 0.84), y = hauteurSur(g, x);
    const pente = Math.atan2(x - g.cx, Math.sqrt(Math.max(1, g.R * g.R - (x - g.cx) ** 2)));
    marcheur.style.transform = `translate(${x}px, ${y}px) rotate(${pente}rad)`;
  };
  const boucle = (t) => { if (!enPause && visible) { place(t); requestAnimationFrame(boucle); } };
  const avance = () => { i = (i + 1) % ORDRE.length; peins(); };
  const lance = () => {
    clearInterval(minuteur);
    if (enPause || !visible || document.hidden) return;
    minuteur = setInterval(avance, CADENCE);
    requestAnimationFrame(boucle);
  };

  new ResizeObserver(redimensionne).observe(canvas);
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; lance(); }).observe(cadre);
  document.addEventListener("visibilitychange", lance);
  if (calme) bouton.hidden = true;
  bouton.addEventListener("click", () => {
    enPause = !enPause;
    bouton.setAttribute("aria-pressed", String(enPause));
    bouton.querySelector("span").textContent = enPause ? "Lecture" : "Pause";
    if (!enPause) depart = performance.now() - 0.42 * 14000;
    place(performance.now());
    lance();
  });
  lance();
})();
