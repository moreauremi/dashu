/* Horizon : la vidéo du haut de la page d'accueil.
   12 plans de la Terre filmés depuis la Station spatiale internationale (NASA, domaine
   public), de 0,8 s chacun, recadrés pour que le bord de la planète tombe toujours sur
   la même courbe : le monde change, la courbe reste (liste des plans dans
   horizon/plans.json). La vidéo ne se charge qu'une fois la page affichée ; en attendant,
   on voit sa première image. Elle s'arrête hors de l'écran, quand l'onglet est caché,
   avec le bouton Pause, et ne démarre pas si le système demande moins d'animations. */
(() => {
  const cadre = document.querySelector(".horizon");
  if (!cadre) return;
  const video = cadre.querySelector("video");
  const bouton = cadre.querySelector(".pause");
  // Cadre plus haut que large (téléphone, tablette debout) : version verticale, plus légère
  // et plus nette (720 × 1200, environ 320 Ko au lieu de 740), et son image d'attente.
  const vertical = () => cadre.clientHeight > cadre.clientWidth;
  if (vertical()) video.poster = "horizon/attente-mobile.webp";
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) { bouton.hidden = true; return; }

  let enPause = false, visible = true, prete = false;
  const regle = () => {
    if (!prete) return;
    if (!enPause && visible && !document.hidden) video.play().catch(() => {});
    else video.pause();
  };
  const demarre = () => {
    if (vertical()) {
      const [webm, mp4] = video.querySelectorAll("source");
      webm.src = "horizon/horizon-mobile.webm"; mp4.src = "horizon/horizon-mobile.mp4";
      video.load();
    }
    video.preload = "auto"; prete = true; regle();
  };
  if (document.readyState === "complete") demarre(); else addEventListener("load", demarre, { once: true });

  new IntersectionObserver(([e]) => { visible = e.isIntersecting; regle(); }).observe(cadre);
  document.addEventListener("visibilitychange", regle);
  bouton.addEventListener("click", () => {
    enPause = !enPause;
    bouton.setAttribute("aria-pressed", String(enPause));
    bouton.querySelector("span").textContent = enPause ? "Lecture" : "Pause";
    regle();
  });
})();
