/* Horizon : le haut de la page d'accueil.
   Des photos de la Terre prises depuis l'orbite (NASA, domaine public), toutes
   recadrées pour que le bord de la planète tombe sur la même courbe, au même
   endroit du cadre. Elles changent toutes les 400 ms : le monde change, la
   courbe reste. Ordre choisi par luminosité, pour éviter les sauts du noir au
   blanc ; 2,5 changements par seconde au plus (sous le seuil de 3 par seconde
   pour les personnes photosensibles). Bouton Pause, arrêt hors de l'écran et
   quand le système demande moins d'animations. La première photo est dans le
   HTML ; les autres se chargent après la page. */
(() => {
  const CADENCE = 400;
  const NOMBRE = 11;
  const cadre = document.querySelector(".horizon");
  if (!cadre) return;
  const calme = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const bouton = cadre.querySelector(".pause");
  const premiere = cadre.querySelector("img");
  if (calme) { bouton.hidden = true; return; }

  const photos = [premiere];
  let i = 0, enPause = false, visible = true, minuteur = null;

  const montre = (n) => photos.forEach((p, k) => p.classList.toggle("vue", k === n));
  const avance = () => {
    // On ne passe qu'aux photos déjà chargées.
    const prete = photos.filter((p) => p.complete && p.naturalWidth);
    if (prete.length < 2) return;
    i = (i + 1) % photos.length;
    while (!(photos[i].complete && photos[i].naturalWidth)) i = (i + 1) % photos.length;
    montre(i);
  };
  const lance = () => {
    clearInterval(minuteur);
    if (!enPause && visible && !document.hidden) minuteur = setInterval(avance, CADENCE);
  };

  const charge = () => {
    for (let n = 2; n <= NOMBRE; n++) {
      const p = new Image();
      p.alt = ""; p.decoding = "async"; p.src = `horizon/${String(n).padStart(2, "0")}.webp`;
      cadre.insertBefore(p, bouton);
      photos.push(p);
    }
  };
  if (document.readyState === "complete") charge(); else addEventListener("load", charge, { once: true });

  new IntersectionObserver(([e]) => { visible = e.isIntersecting; lance(); }).observe(cadre);
  document.addEventListener("visibilitychange", lance);
  bouton.addEventListener("click", () => {
    enPause = !enPause;
    bouton.setAttribute("aria-pressed", String(enPause));
    bouton.querySelector("span").textContent = enPause ? "Lecture" : "Pause";
    lance();
  });
  lance();
})();
