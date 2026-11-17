/* En-tête : le logo se replie en H/ dès que la page défile (voir .compact). */
(() => {
  const bar = document.querySelector(".bar");
  const logo = bar.querySelector(".logo");
  const update = () => {
    const compact = scrollY > 24;
    // Le dépliage du chargement ne sert qu'une fois : sans cela, il se
    // rejouerait à chaque retour en haut de page.
    if (compact) logo.classList.remove("logo-anim");
    bar.classList.toggle("compact", compact);
  };
  addEventListener("scroll", update, { passive: true });
  update();
})();
